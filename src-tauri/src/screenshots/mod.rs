pub mod frame;
mod writer;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use uuid::Uuid;

pub use frame::{capture_frame, supports_fast_sampling};
pub use writer::{CaptureJob, PostClickSampler, ScreenshotWriter};

use crate::events::{screenshot_trigger_label, should_trigger_screenshot};
use crate::platform::{CapturedInputEvent, ScreenshotCapturer};
use crate::storage::Database;
use crate::storage::models::Screenshot;

pub struct PendingCapture {
    pub id: String,
    pub path: PathBuf,
    pub session_id: String,
    pub timestamp_ms: i64,
    pub trigger: String,
    pub click_x: Option<i64>,
    pub click_y: Option<i64>,
    pub monitor_id: Option<String>,
}

pub struct ScreenshotEngine {
    db: Arc<Database>,
    session_id: Option<String>,
    monitor_id: Option<String>,
    last_window_key: Option<String>,
}

impl ScreenshotEngine {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            session_id: None,
            monitor_id: None,
            last_window_key: None,
        }
    }

    pub fn start(&mut self, session_id: &str, _session_dir: PathBuf, monitor_id: Option<String>) -> Result<()> {
        self.session_id = Some(session_id.to_string());
        self.monitor_id = monitor_id;
        self.last_window_key = None;
        Ok(())
    }

    pub fn stop(&mut self, session_id: &str) -> Result<()> {
        let _ = session_id;
        self.session_id = None;
        self.monitor_id = None;
        self.last_window_key = None;
        Ok(())
    }

    pub fn set_monitor_id(&mut self, monitor_id: Option<String>) {
        self.monitor_id = monitor_id;
    }

    pub fn monitor_id(&self) -> Option<String> {
        self.monitor_id.clone()
    }

    pub fn is_active(&self) -> bool {
        self.session_id.is_some()
    }

    pub fn prepare_manual(
        &self,
        session_id: &str,
        session_dir: PathBuf,
    ) -> Result<PendingCapture> {
        self.plan_capture(session_id, session_dir, "manual_marker", now_ms(), None, None)
    }

    /// Plans the "after the click" screenshot once the post-click sampler decided the screen
    /// has settled; the timestamp is the moment the frame was actually taken.
    pub fn prepare_post_click(
        &self,
        session_id: &str,
        session_dir: PathBuf,
        trigger: &str,
        click_x: Option<i64>,
        click_y: Option<i64>,
    ) -> Result<Option<PendingCapture>> {
        if !self.is_active() {
            return Ok(None);
        }
        Ok(Some(self.plan_capture(
            session_id,
            session_dir,
            &format!("post_{trigger}"),
            now_ms(),
            click_x,
            click_y,
        )?))
    }

    pub fn prepare_input_event(
        &mut self,
        session_id: &str,
        session_dir: PathBuf,
        event: &CapturedInputEvent,
    ) -> Result<Option<PendingCapture>> {
        if !self.is_active() || !should_trigger_screenshot(event) {
            return Ok(None);
        }

        let (click_x, click_y) = if event.event_type == "mouse_click" || event.event_type == "mouse_double_click" {
            let x = event.payload.get("x").and_then(|value| value.as_i64());
            let y = event.payload.get("y").and_then(|value| value.as_i64());
            (x, y)
        } else {
            (None, None)
        };

        let trigger = screenshot_trigger_label(event);
        let capture = self.plan_capture(
            session_id,
            session_dir,
            &trigger,
            event.timestamp_ms,
            click_x,
            click_y,
        )?;

        Ok(Some(capture))
    }

    pub fn prepare_window_change(
        &mut self,
        session_id: &str,
        session_dir: PathBuf,
        app_name: &str,
        window_title: &str,
        timestamp_ms: i64,
    ) -> Result<Option<PendingCapture>> {
        if !self.is_active() {
            return Ok(None);
        }

        let window_key = format!("{app_name}::{window_title}");
        if self.last_window_key.as_deref() == Some(window_key.as_str()) {
            return Ok(None);
        }
        self.last_window_key = Some(window_key);
        Ok(Some(
            self.plan_capture(session_id, session_dir, "window_change", timestamp_ms, None, None)?,
        ))
    }

    pub fn finish_capture(&self, pending: PendingCapture) -> Result<Screenshot> {
        let highlight_enabled = self
            .db
            .get_setting("highlight_clicks")
            .ok()
            .flatten()
            .map(|val| val != "false")
            .unwrap_or(true);

        let annotations_json = if highlight_enabled {
            if let (Some(x), Some(y)) = (pending.click_x, pending.click_y) {
                Some(serde_json::json!([
                    {
                        "id": "click_primary",
                        "type": "click",
                        "x": x,
                        "y": y,
                        "color": "#ef4444",
                        "strokeWidth": 3
                    }
                ]).to_string())
            } else {
                None
            }
        } else {
            None
        };

        let screenshot = Screenshot {
            id: pending.id,
            session_id: pending.session_id,
            path: pending.path.to_string_lossy().to_string(),
            timestamp_ms: pending.timestamp_ms,
            trigger: Some(pending.trigger),
            selected: 0,
            click_x: pending.click_x,
            click_y: pending.click_y,
            annotations_json,
        };
        self.db.insert_screenshot(&screenshot)?;
        Ok(screenshot)
    }

    fn plan_capture(
        &self,
        session_id: &str,
        session_dir: PathBuf,
        trigger: &str,
        timestamp_ms: i64,
        click_x: Option<i64>,
        click_y: Option<i64>,
    ) -> Result<PendingCapture> {
        if self.session_id.is_none() {
            anyhow::bail!("nessuna registrazione in corso");
        }
        let id = Uuid::new_v4().to_string();
        let screenshots_dir = session_dir.join("screenshots");
        std::fs::create_dir_all(&screenshots_dir)?;
        let path = screenshots_dir.join(format!("{id}.png"));
        Ok(PendingCapture {
            id,
            path,
            session_id: session_id.to_string(),
            timestamp_ms,
            trigger: trigger.to_string(),
            click_x,
            click_y,
            monitor_id: self.monitor_id.clone(),
        })
    }
}

pub fn run_capture(
    capturer: &dyn ScreenshotCapturer,
    pending: &PendingCapture,
) -> Result<()> {
    capturer.capture_monitor(pending.path.clone(), pending.monitor_id.as_deref())?;
    Ok(())
}

/// Writes to `target` a copy of the screenshot at `source` with a click marker drawn at
/// `(click_x, click_y)` (image pixels). The source file is never modified. Returns `false` when
/// the point is outside the image and nothing was written.
pub fn render_click_highlight(source: &Path, target: &Path, click_x: i64, click_y: i64) -> Result<bool> {
    if !source.is_file() {
        return Ok(false);
    }
    let mut img = image::open(source)?.to_rgba8();
    let (width, height) = img.dimensions();
    if click_x < 0 || click_y < 0 || click_x >= width as i64 || click_y >= height as i64 {
        return Ok(false);
    }
    let (cx, cy) = (click_x as i32, click_y as i32);

    // High-visibility click cursor marker:
    // Center dot + dual contrast ring + outer subtle ripple
    let max_r = 30i32;
    for dy in -max_r..=max_r {
        for dx in -max_r..=max_r {
            let px = cx + dx;
            let py = cy + dy;
            if px < 0 || px >= width as i32 || py < 0 || py >= height as i32 {
                continue;
            }

            let dist_sq = dx * dx + dy * dy;
            let dist = (dist_sq as f64).sqrt();

            let color: Option<[u8; 4]> = if dist <= 4.0 {
                // Central bright dot
                Some([239, 68, 68, 255])
            } else if (13.0..=17.0).contains(&dist) {
                // Main neon ring
                Some([239, 68, 68, 230])
            } else if (dist > 17.0 && dist <= 19.5) || (10.5..13.0).contains(&dist) {
                // White outline for contrast against both dark and light backgrounds
                Some([255, 255, 255, 220])
            } else if dist > 19.5 && dist <= 28.0 {
                // Outer translucent ripple
                Some([239, 68, 68, 55])
            } else {
                None
            };

            if let Some(c) = color {
                let existing = img.get_pixel_mut(px as u32, py as u32);
                let alpha = c[3] as f32 / 255.0;
                let inv_alpha = 1.0 - alpha;
                existing[0] = (c[0] as f32 * alpha + existing[0] as f32 * inv_alpha) as u8;
                existing[1] = (c[1] as f32 * alpha + existing[1] as f32 * inv_alpha) as u8;
                existing[2] = (c[2] as f32 * alpha + existing[2] as f32 * inv_alpha) as u8;
            }
        }
    }

    // Same format as the source, written next to the target and renamed into place.
    let format = image::ImageFormat::from_path(source).unwrap_or(image::ImageFormat::Png);
    let partial = target.with_extension("partial");
    img.save_with_format(&partial, format)?;
    std::fs::rename(&partial, target)?;
    Ok(true)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
