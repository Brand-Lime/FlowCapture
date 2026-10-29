use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use parking_lot::Mutex;
use uuid::Uuid;

use crate::events::{is_flowcapture_app, should_trigger_screenshot, EventCollector};
use crate::platform::{preflight_recording_start, PlatformServices, SharedScreenshotCapturer};
use crate::recorder::RecorderEngine;
use crate::screenshots::{
    capture_frame, run_capture, supports_fast_sampling, CaptureJob, PostClickSampler, ScreenshotEngine,
    ScreenshotWriter,
};
use crate::storage::Database;
use crate::storage::models::{Session, SessionStatus, StoredEvent};
use crate::thread_util::join_thread_with_timeout;

// Covers flushing the background screenshot writer (PNG encoding of queued frames).
const COLLECTOR_JOIN_TIMEOUT: Duration = Duration::from_secs(20);
/// Upper bound for writing the screenshots still queued when the recording stops. The session
/// is marked ready only afterwards, so documentation never starts without its last shots.
const SCREENSHOT_FLUSH_TIMEOUT: Duration = Duration::from_secs(180);
const COLLECTOR_TICK: Duration = Duration::from_millis(60);
const PLATFORM_STOP_TIMEOUT: Duration = Duration::from_secs(3);

use std::collections::HashMap;

pub struct AppState {
    pub db: Arc<Database>,
    pub platform: Arc<Mutex<PlatformServices>>,
    pub recorder: Mutex<RecorderEngine>,
    pub event_collector: Mutex<EventCollector>,
    pub screenshot_engine: Arc<Mutex<ScreenshotEngine>>,
    pub active_session: Mutex<Option<ActiveRecording>>,
    pub active_ai_cancellations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub browser_bridge: crate::platform::browser_bridge::BrowserBridgeState,
    /// Keeps the optional "sessions folder" copy of every session up to date.
    pub mirror: crate::storage::mirror::MirrorWorker,
}

pub struct ActiveRecording {
    pub session: Session,
    pub started_at: Instant,
    pub monitor_id: Option<String>,
    pub is_paused: Arc<AtomicBool>,
    /// Closed pause intervals as epoch-ms `(start, end)`, plus the start of the current pause.
    /// The microphone track stops during a pause, so these are needed to map event times onto
    /// the audio timeline and to report the effective duration.
    pauses: Vec<(i64, i64)>,
    paused_since: Option<i64>,
    collector_stop: Arc<AtomicBool>,
    collector_handle: JoinHandle<()>,
    /// Screenshots captured but not yet written by the background writer.
    pending_screenshots: Arc<AtomicUsize>,
    full_video_recorder: Option<crate::recorder::full_video::FullVideoRecorderHandle>,
}

impl AppState {
    pub fn new(db: Arc<Database>, platform: PlatformServices) -> Result<Self> {
        let browser_bridge = crate::platform::browser_bridge::BrowserBridgeState::new();
        crate::platform::browser_bridge::start_browser_bridge_server(
            browser_bridge.clone(),
            crate::platform::browser_bridge::DEFAULT_BRIDGE_PORT,
        );

        let mirror = crate::storage::mirror::MirrorWorker::spawn(db.clone());
        let notifier = mirror.clone();
        db.set_change_listener(Box::new(move |session_id| notifier.request(session_id)));

        Ok(Self {
            mirror,
            db: db.clone(),
            platform: Arc::new(Mutex::new(platform)),
            recorder: Mutex::new(RecorderEngine::new()),
            event_collector: Mutex::new(EventCollector::new()),
            screenshot_engine: Arc::new(Mutex::new(ScreenshotEngine::new(db.clone()))),
            active_session: Mutex::new(None),
            active_ai_cancellations: Mutex::new(HashMap::new()),
            browser_bridge,
        })
    }

    pub fn register_ai_cancellation(&self, session_id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.active_ai_cancellations
            .lock()
            .insert(session_id.to_string(), flag.clone());
        flag
    }

    pub fn cancel_ai_job(&self, session_id: &str) -> bool {
        let map = self.active_ai_cancellations.lock();
        if let Some(flag) = map.get(session_id) {
            flag.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn remove_ai_cancellation(&self, session_id: &str) {
        self.active_ai_cancellations.lock().remove(session_id);
    }

    pub fn cleanup_stale_recordings(&self) -> Result<()> {
        for session in self.db.list_sessions()? {
            if session.status == SessionStatus::Recording.as_str() {
                self.db.update_session_status(&session.id, SessionStatus::Ready)?;
            }
        }
        Ok(())
    }

    pub fn start_recording(&self, title: Option<String>, monitor_id: Option<String>) -> Result<Session> {
        let mut active = self.active_session.lock();
        if active.is_some() {
            anyhow::bail!("è già in corso una registrazione");
        }

        preflight_recording_start()?;

        let effective_monitor_id = monitor_id
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.db.get_setting("selected_monitor_id").ok().flatten());

        if let Some(ref mid) = effective_monitor_id {
            let _ = self.db.set_setting("selected_monitor_id", mid);
        }

        let session = self.db.create_session(title)?;
        let session_id = session.id.clone();
        let session_dir = self.db.session_dir(&session_id);

        let mon_id = effective_monitor_id.clone();
        let collector_paused = Arc::new(AtomicBool::new(false));
        let collector_paused_clone = collector_paused.clone();
        self.browser_bridge.set_recording(true);
        let browser_bridge_clone = self.browser_bridge.clone();

        let record_full_video = self
            .db
            .get_setting("record_full_video")
            .ok()
            .flatten()
            .map(|v| v != "false")
            .unwrap_or(true);

        let full_video_fps: u32 = self
            .db
            .get_setting("full_video_fps")
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);

        let full_video_recorder = if record_full_video {
            match crate::recorder::full_video::FullVideoRecorderHandle::start(
                session_dir.clone(),
                crate::recorder::full_video::FullVideoConfig {
                    fps: full_video_fps,
                    monitor_id: mon_id.clone(),
                    quality_crf: 23,
                },
            ) {
                Ok(handle) => Some(handle),
                Err(err) => {
                    eprintln!("failed to start full video recorder: {err:#}");
                    None
                }
            }
        } else {
            None
        };

        let pending_screenshots = Arc::new(AtomicUsize::new(0));
        let start_result = (|| -> Result<(JoinHandle<()>, Arc<AtomicBool>)> {
            let mut platform = self.platform.lock();
            platform.input.start()?;
            platform.windows.start()?;
            // The low-fps "slideshow" (a PNG every 500 ms in video/frames) is only a fallback
            // for when the full HD recording is off or could not start: running both captures
            // the screen twice and fills the disk with frames.
            if full_video_recorder.is_none() {
                self.recorder
                    .lock()
                    .start_with_platform(session_dir.clone(), &mut platform, mon_id.clone())?;
            }
            self.event_collector.lock().start(&session_id)?;
            self.screenshot_engine
                .lock()
                .start(&session_id, session_dir.clone(), mon_id)?;

            let collector_stop = Arc::new(AtomicBool::new(false));
            let handle = spawn_recording_collector(
                collector_stop.clone(),
                collector_paused_clone,
                self.db.clone(),
                self.platform.clone(),
                self.screenshot_engine.clone(),
                browser_bridge_clone,
                session_id.clone(),
                session_dir,
                pending_screenshots.clone(),
            );

            Ok((handle, collector_stop))
        })();

        match start_result {
            Ok((collector_handle, collector_stop)) => {
                *active = Some(ActiveRecording {
                    session: session.clone(),
                    started_at: Instant::now(),
                    monitor_id: effective_monitor_id,
                    is_paused: collector_paused,
                    pauses: Vec::new(),
                    paused_since: None,
                    collector_stop,
                    collector_handle,
                    pending_screenshots,
                    full_video_recorder,
                });
                Ok(session)
            }
            Err(err) => {
                if let Some(fvr) = full_video_recorder {
                    let _ = fvr.stop();
                }
                self.browser_bridge.set_recording(false);
                let _ = self.rollback_failed_start(&session_id);
                Err(err)
            }
        }
    }

    pub fn stop_recording(&self) -> Result<Session> {
        let mut active = {
            let mut guard = self.active_session.lock();
            guard
                .take()
                .ok_or_else(|| anyhow::anyhow!("nessuna registrazione in corso"))?
        };

        if let Some(since) = active.paused_since.take() {
            active.pauses.push((since, epoch_ms()));
        }
        let paused_ms: i64 = active.pauses.iter().map(|(start, end)| (end - start).max(0)).sum();
        let duration = (active.started_at.elapsed().as_millis() as i64 - paused_ms).max(0) / 1000;
        let session_id = active.session.id.clone();

        active.collector_stop.store(true, Ordering::SeqCst);
        if let Some(fvr) = active.full_video_recorder.take() {
            if let Err(err) = fvr.stop() {
                eprintln!("FlowCapture: full video recorder stop failed: {err:#}");
            }
        }
        self.screenshot_engine.lock().stop(&session_id)?;
        signal_platform_stop(self);
        self.browser_bridge.set_recording(false);

        if !join_thread_with_timeout(active.collector_handle, COLLECTOR_JOIN_TIMEOUT) {
            eprintln!("FlowCapture: recording collector did not stop within timeout");
        }
        // The collector hands its queue to the writer and exits; wait for the queue itself.
        let flush_deadline = Instant::now() + SCREENSHOT_FLUSH_TIMEOUT;
        while active.pending_screenshots.load(Ordering::SeqCst) > 0 && Instant::now() < flush_deadline {
            thread::sleep(Duration::from_millis(50));
        }
        let unsaved = active.pending_screenshots.load(Ordering::SeqCst);
        if unsaved > 0 {
            crate::logger::write_entry(
                "WARN",
                "recording",
                &format!("{unsaved} screenshot(s) still being written after {}s", SCREENSHOT_FLUSH_TIMEOUT.as_secs()),
            );
        }
        let _ = stop_platform_services(self);

        let mut pending_events = Vec::new();
        if let Some(mut platform) = self.platform.try_lock() {
            for input in platform.input.drain_events() {
                if should_persist_event(&input.event_type) {
                    pending_events.push(stored_from_input(&session_id, input, &self.browser_bridge));
                }
            }
            for window in platform.windows.drain_events() {
                pending_events.push(stored_from_window(&session_id, window));
            }
        }

        self.event_collector.lock().stop(&session_id)?;
        if !pending_events.is_empty() {
            self.db.insert_events(&pending_events)?;
        }

        self.db
            .finish_session(&session_id, None, duration)?;
        if !active.pauses.is_empty() {
            let _ = self
                .db
                .set_session_pauses(&session_id, &serde_json::to_string(&active.pauses)?);
        }

        self.db
            .get_session(&session_id)?
            .ok_or_else(|| anyhow::anyhow!("sessione non trovata dopo la fine della registrazione"))
    }

    pub fn capture_manual_screenshot(&self) -> Result<()> {
        let (session_id, monitor_id) = {
            let active = self.active_session.lock();
            let recording = active.as_ref().ok_or_else(|| anyhow::anyhow!("nessuna registrazione in corso"))?;
            (recording.session.id.clone(), recording.monitor_id.clone())
        };

        let session_dir = self.db.session_dir(&session_id);
        let mut pending = self
            .screenshot_engine
            .lock()
            .prepare_manual(&session_id, session_dir)?;
        if pending.monitor_id.is_none() {
            pending.monitor_id = monitor_id;
        }
        let capturer = SharedScreenshotCapturer;
        run_capture(&capturer, &pending)?;
        self.screenshot_engine.lock().finish_capture(pending)?;
        Ok(())
    }

    pub fn pause_recording(&self) -> Result<()> {
        let mut guard = self.active_session.lock();
        let active = guard.as_mut().ok_or_else(|| anyhow::anyhow!("nessuna registrazione in corso"))?;
        if active.paused_since.is_none() {
            active.paused_since = Some(epoch_ms());
        }
        active.is_paused.store(true, Ordering::SeqCst);
        if let Some(ref fvr) = active.full_video_recorder {
            fvr.pause();
        }
        if let Some(platform) = self.platform.try_lock() {
            self.recorder.lock().pause_with_platform(&platform);
        }
        Ok(())
    }

    pub fn resume_recording(&self) -> Result<()> {
        let mut guard = self.active_session.lock();
        let active = guard.as_mut().ok_or_else(|| anyhow::anyhow!("nessuna registrazione in corso"))?;
        if let Some(since) = active.paused_since.take() {
            active.pauses.push((since, epoch_ms()));
        }
        active.is_paused.store(false, Ordering::SeqCst);
        if let Some(ref fvr) = active.full_video_recorder {
            fvr.resume();
        }
        if let Some(platform) = self.platform.try_lock() {
            self.recorder.lock().resume_with_platform(&platform);
        }
        Ok(())
    }

    pub fn switch_recording_monitor(&self, monitor_id: String) -> Result<()> {
        let mut guard = self.active_session.lock();
        let active = guard.as_mut().ok_or_else(|| anyhow::anyhow!("nessuna registrazione in corso"))?;
        active.monitor_id = Some(monitor_id.clone());
        self.screenshot_engine.lock().set_monitor_id(Some(monitor_id.clone()));
        if let Some(platform) = self.platform.try_lock() {
            let _ = self.recorder.lock().switch_monitor_with_platform(&platform, Some(monitor_id.clone()));
        }
        let _ = self.db.set_setting("selected_monitor_id", &monitor_id);
        Ok(())
    }

    fn rollback_failed_start(&self, session_id: &str) -> Result<()> {
        let _ = self.screenshot_engine.lock().stop(session_id);
        let _ = self.event_collector.lock().stop(session_id);
        if let Some(mut platform) = self.platform.try_lock() {
            let _ = platform.input.stop();
            let _ = platform.windows.stop();
            let _ = self.recorder.lock().stop_with_platform(&mut platform);
        }
        let _ = self.db.delete_session(session_id);
        Ok(())
    }
}

fn epoch_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn signal_platform_stop(state: &AppState) {
    for _ in 0..100 {
        if let Some(mut platform) = state.platform.try_lock() {
            platform.recorder.request_stop();
            platform.input.request_stop();
            platform.windows.request_stop();
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn stop_platform_services(state: &AppState) -> Result<Option<String>> {
    let deadline = Instant::now() + PLATFORM_STOP_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(mut platform) = state.platform.try_lock() {
            platform.input.stop()?;
            platform.windows.stop()?;
            let video_path = state.recorder.lock().stop_with_platform(&mut platform)?;
            return Ok(video_path);
        }
        thread::sleep(Duration::from_millis(5));
    }

    anyhow::bail!("tempo scaduto durante l'arresto della registrazione")
}

#[allow(clippy::too_many_arguments)]
fn spawn_recording_collector(
    stop_flag: Arc<AtomicBool>,
    is_paused: Arc<AtomicBool>,
    db: Arc<Database>,
    platform: Arc<Mutex<PlatformServices>>,
    screenshot_engine: Arc<Mutex<ScreenshotEngine>>,
    browser_bridge: crate::platform::browser_bridge::BrowserBridgeState,
    session_id: String,
    session_dir: PathBuf,
    pending_screenshots: Arc<AtomicUsize>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let setting_enabled = |key: &str, default: bool| {
            db.get_setting(key)
                .ok()
                .flatten()
                .map(|val| val == "true")
                .unwrap_or(default)
        };
        let capture_all = setting_enabled("capture_all_events", false);
        let dedupe = setting_enabled("dedupe_screenshots", true);
        let fast_sampling = supports_fast_sampling();

        let writer = ScreenshotWriter::spawn(screenshot_engine.clone(), db.clone(), dedupe, pending_screenshots);
        let mut post_click: Option<PostClickSampler> = None;
        let mut last_mouse: Option<(i64, i64)> = None;

        while !stop_flag.load(Ordering::SeqCst) {
            if is_paused.load(Ordering::SeqCst) {
                // Drain and discard any events while paused to avoid backlog
                if let Some(mut plat) = platform.try_lock() {
                    let _ = plat.input.drain_events();
                    let _ = plat.windows.drain_events();
                }
                post_click = None;
                thread::sleep(Duration::from_millis(150));
                continue;
            }

            let (input_events, window_events) = {
                let mut platform = platform.lock();
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                (
                    platform.input.drain_events(),
                    platform.windows.drain_events(),
                )
            };

            if stop_flag.load(Ordering::SeqCst) {
                break;
            }
            if is_paused.load(Ordering::SeqCst) {
                // Paused while draining: these events belong to the pause, drop them and keep
                // the loop alive so capture resumes with the recording.
                post_click = None;
                continue;
            }

            for input in &input_events {
                let x = input.payload.get("x").and_then(|v| v.as_i64());
                let y = input.payload.get("y").and_then(|v| v.as_i64());
                if let (Some(x), Some(y)) = (x, y) {
                    last_mouse = Some((x, y));
                    if let Some(sampler) = post_click.as_mut() {
                        sampler.track_mouse(x, y);
                    }
                }
            }

            let monitor_id;
            let mut pending_captures = Vec::new();
            let mut new_click: Option<(String, Option<(i64, i64)>)> = None;
            {
                let mut engine = screenshot_engine.lock();
                monitor_id = engine.monitor_id();
                if engine.is_active() {
                    for input in &input_events {
                        if stop_flag.load(Ordering::SeqCst) || is_paused.load(Ordering::SeqCst) {
                            break;
                        }
                        // Clicks on FlowCapture itself (Stop, Pause, HUD) are not workflow steps,
                        // and a pending "after" shot would now show FlowCapture as well.
                        if input.app_name.as_deref().is_some_and(is_flowcapture_app) {
                            if matches!(input.event_type.as_str(), "mouse_click" | "mouse_double_click") {
                                post_click = None;
                            }
                            continue;
                        }
                        if capture_all || should_trigger_screenshot(input) {
                            if let Ok(Some(pending)) = engine.prepare_input_event(
                                &session_id,
                                session_dir.clone(),
                                input,
                            ) {
                                if matches!(input.event_type.as_str(), "mouse_click" | "mouse_double_click") {
                                    new_click = Some((
                                        crate::events::screenshot_trigger_label(input),
                                        pending.click_x.zip(pending.click_y),
                                    ));
                                }
                                pending_captures.push(pending);
                            }
                        }
                    }

                    for window in &window_events {
                        if stop_flag.load(Ordering::SeqCst) {
                            break;
                        }
                        if is_flowcapture_app(&window.app_name) {
                            continue;
                        }
                        if let Ok(Some(pending)) = engine.prepare_window_change(
                            &session_id,
                            session_dir.clone(),
                            &window.app_name,
                            &window.title,
                            window.timestamp_ms,
                        ) {
                            pending_captures.push(pending);
                        }
                    }
                }
            }

            // Grab the screen once for everything that happened in this tick; encoding and
            // saving happen on the writer thread.
            if !pending_captures.is_empty() && !stop_flag.load(Ordering::SeqCst) {
                match capture_frame(monitor_id.as_deref()) {
                    Ok(frame) => {
                        if let Some((trigger, click)) = new_click {
                            // A new click supersedes the previous click's pending "after" shot:
                            // this "before" shot already shows that result.
                            post_click = Some(PostClickSampler::new(trigger, click, frame.clone(), fast_sampling));
                        }
                        for pending in pending_captures {
                            writer.submit(CaptureJob { pending, frame: frame.clone(), cursor: last_mouse });
                        }
                    }
                    Err(err) => eprintln!("FlowCapture: screen capture failed: {err:#}"),
                }
            }

            let now = Instant::now();
            if post_click.as_ref().is_some_and(|sampler| sampler.is_due(now)) {
                match capture_frame(monitor_id.as_deref()) {
                    Ok(frame) => {
                        let sampler = post_click.as_mut().expect("checked above");
                        if let Some(settled) = sampler.offer(frame, last_mouse, now) {
                            let sampler = post_click.take().expect("checked above");
                            let planned = screenshot_engine.lock().prepare_post_click(
                                &session_id,
                                session_dir.clone(),
                                &sampler.trigger,
                                sampler.click.map(|c| c.0),
                                sampler.click.map(|c| c.1),
                            );
                            if let Ok(Some(pending)) = planned {
                                writer.submit(CaptureJob { pending, frame: settled, cursor: last_mouse });
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("FlowCapture: post-click capture failed: {err:#}");
                        post_click = None;
                    }
                }
            }

            let mut batch = Vec::new();
            for input in input_events {
                if should_persist_event(&input.event_type) {
                    batch.push(stored_from_input(&session_id, input, &browser_bridge));
                }
            }
            for window in window_events {
                batch.push(stored_from_window(&session_id, window));
            }

            if !batch.is_empty() {
                let _ = db.insert_events(&batch);
            }

            if stop_flag.load(Ordering::SeqCst) {
                break;
            }

            thread::sleep(COLLECTOR_TICK);
        }

        writer.finish();
    })
}

fn should_persist_event(event_type: &str) -> bool {
    !matches!(event_type, "mouse_move")
}

fn stored_from_input(
    session_id: &str,
    mut event: crate::platform::CapturedInputEvent,
    browser_bridge: &crate::platform::browser_bridge::BrowserBridgeState,
) -> StoredEvent {
    if event.event_type == "mouse_click" || event.event_type == "mouse_double_click" {
        let x = event.payload.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let y = event.payload.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        // 1. Try matching with browser DOM event from the extension
        use crate::platform::browser_bridge::{is_browser_app, BROWSER_MATCH_DISTANCE, OTHER_APP_MATCH_DISTANCE};
        let max_distance = match event.app_name.as_deref() {
            Some(app) if !is_browser_app(app) => OTHER_APP_MATCH_DISTANCE,
            _ => BROWSER_MATCH_DISTANCE,
        };
        if let Some(dom) = browser_bridge.match_and_take_event(event.timestamp_ms, x, y, max_distance) {
            if let Some(obj) = event.payload.as_object_mut() {
                if !dom.text.is_empty() {
                    obj.insert("element_name".to_string(), serde_json::Value::String(dom.text));
                }
                obj.insert("element_type".to_string(), serde_json::Value::String(dom.tag.to_lowercase()));
                obj.insert("url".to_string(), serde_json::Value::String(dom.url));
                obj.insert("page_title".to_string(), serde_json::Value::String(dom.page_title));
                if let Some(sel) = dom.selector {
                    obj.insert("selector".to_string(), serde_json::Value::String(sel));
                }
                if let Some(role) = dom.role {
                    obj.insert("element_role".to_string(), serde_json::Value::String(role));
                }
            }
        } else {
            // 2. Fallback to Windows UI Automation
            #[cfg(target_os = "windows")]
            {
                if let Some(uia) = crate::platform::windows_uia::get_element_at_point(x, y) {
                    if let Some(obj) = event.payload.as_object_mut() {
                        if !uia.name.is_empty() {
                            obj.insert("element_name".to_string(), serde_json::Value::String(uia.name));
                        }
                        let type_desc = if !uia.localized_type.is_empty() {
                            uia.localized_type
                        } else {
                            uia.control_type
                        };
                        if !type_desc.is_empty() {
                            obj.insert("element_type".to_string(), serde_json::Value::String(type_desc));
                        }
                        if let Some(cls) = uia.class_name {
                            obj.insert("class_name".to_string(), serde_json::Value::String(cls));
                        }
                    }
                }
            }
        }
    }

    StoredEvent {
        id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        event_type: event.event_type,
        app_name: event.app_name,
        payload: event.payload.to_string(),
        created_at: Utc::now().to_rfc3339(),
        timestamp_ms: event.timestamp_ms,
    }
}

fn stored_from_window(session_id: &str, event: crate::platform::WindowInfo) -> StoredEvent {
    StoredEvent {
        id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        event_type: "window_focus".to_string(),
        app_name: Some(event.app_name.clone()),
        payload: serde_json::json!({
            "title": event.title,
            "app_name": event.app_name,
        })
        .to_string(),
        created_at: Utc::now().to_rfc3339(),
        timestamp_ms: event.timestamp_ms,
    }
}
