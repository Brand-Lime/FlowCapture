use std::sync::mpsc;
use std::sync::Arc;

use anyhow::anyhow;
use tauri::{AppHandle, State};

use crate::media::spawn_session_video_encode;
use crate::platform::{
    check_recording_permissions,
    open_accessibility_settings as open_accessibility_settings_impl,
    open_screen_recording_settings as open_screen_recording_settings_impl,
    prepare_recording_permissions as prepare_recording_permissions_impl,
    request_accessibility_permission as request_accessibility_permission_impl,
    reveal_executable_in_finder as reveal_executable_in_finder_impl,
    RecordingPermissions,
};
use crate::ai::AiPipeline;
use crate::export::ExportEngine;
use crate::state::AppState;
use crate::storage::models::{
    AiJob, ExportRecord, ProviderConfig, RedactionSummary, Session, Screenshot, StoredEvent,
};

fn run_on_main_thread<T, F>(app: &AppHandle, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = tx.send(task());
    })
    .map_err(|err| err.to_string())?;
    rx.recv().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_recording_permissions(app: AppHandle) -> Result<RecordingPermissions, String> {
    run_on_main_thread(&app, check_recording_permissions)
}

#[tauri::command]
pub async fn prepare_recording_permissions(app: AppHandle) -> Result<RecordingPermissions, String> {
    tauri::async_runtime::spawn_blocking(move || run_on_main_thread(&app, prepare_recording_permissions_impl))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
pub fn request_accessibility_permission(app: AppHandle) -> Result<bool, String> {
    run_on_main_thread(&app, request_accessibility_permission_impl)
}

#[tauri::command]
pub fn open_screen_recording_settings() -> Result<(), String> {
    open_screen_recording_settings_impl().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn open_accessibility_settings() -> Result<(), String> {
    open_accessibility_settings_impl().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn reveal_executable_in_finder() -> Result<(), String> {
    reveal_executable_in_finder_impl().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_platform_name(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    Ok(state.platform.lock().platform_name.clone())
}

#[tauri::command]
pub fn list_sessions(state: State<'_, Arc<AppState>>) -> Result<Vec<Session>, String> {
    let mut sessions = state.db.list_sessions().map_err(|err| err.to_string())?;
    for session in &mut sessions {
        session.preview_screenshot_path = state
            .db
            .session_preview_screenshot_path(&session.id)
            .ok()
            .flatten();
    }
    Ok(sessions)
}

#[tauri::command]
pub fn get_session(state: State<'_, Arc<AppState>>, session_id: String) -> Result<Option<Session>, String> {
    let mut session = state
        .db
        .get_session(&session_id)
        .map_err(|err| err.to_string())?;

    if let Some(session_ref) = session.as_mut() {
        if session_ref.documentation_md.is_some() {
            let screenshots = state
                .db
                .list_screenshots(&session_id)
                .map_err(|err| err.to_string())?;
            if let Some(markdown) = session_ref.documentation_md.as_mut() {
                *markdown = crate::ai::documentation::normalize_screenshot_references(
                    markdown,
                    &screenshots,
                );
            }
        }
    }

    Ok(session)
}

#[tauri::command]
pub fn update_session_title(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    title: String,
) -> Result<(), String> {
    state
        .db
        .update_session_title(&session_id, &title)
        .map_err(|err| err.to_string())?;
    // Exported files carry the session name: keep them in sync with the new title.
    if let Err(err) = crate::export::rename_session_exports(&state.db, &session_id, &title) {
        crate::logger::write_entry("WARN", "export", &format!("could not rename exports: {err:#}"));
    }
    Ok(())
}

#[tauri::command]
pub fn list_monitors() -> Result<Vec<crate::storage::models::MonitorInfo>, String> {
    crate::platform::list_monitors().map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn start_recording(
    state: State<'_, Arc<AppState>>,
    title: Option<String>,
    monitor_id: Option<String>,
) -> Result<Session, String> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || state.start_recording(title, monitor_id))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn stop_recording(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<Session, String> {
    let db = Arc::clone(&state.db);
    let state = Arc::clone(state.inner());
    let session = tauri::async_runtime::spawn_blocking(move || state.stop_recording())
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())?;
    spawn_session_video_encode(
        db.clone(),
        app.clone(),
        session.id.clone(),
        session.duration.max(1),
    );
    crate::media::spawn_session_full_video_finalize(
        db,
        app,
        session.id.clone(),
        None,
    );
    Ok(session)
}

#[tauri::command]
pub fn pause_recording(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.pause_recording().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn resume_recording(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.resume_recording().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn switch_recording_monitor(
    state: State<'_, Arc<AppState>>,
    monitor_id: String,
) -> Result<(), String> {
    state
        .switch_recording_monitor(monitor_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn capture_manual_screenshot(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || state.capture_manual_screenshot())
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn list_events(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<StoredEvent>, String> {
    state
        .db
        .list_events(&session_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn list_screenshots(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<Screenshot>, String> {
    state
        .db
        .list_screenshots(&session_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn delete_screenshot(
    state: State<'_, Arc<AppState>>,
    screenshot_id: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || delete_screenshot_everywhere(&db, &screenshot_id))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

/// Removes a screenshot from the database, from the steps and the Markdown that reference it,
/// and from disk (with its `_clean`/`_annotated` variants).
fn delete_screenshot_everywhere(db: &crate::storage::Database, screenshot_id: &str) -> anyhow::Result<()> {
    let Some(shot) = db.get_screenshot(screenshot_id)? else {
        return Ok(());
    };

    if let Some(session) = db.get_session(&shot.session_id)? {
        let mut steps: Vec<crate::storage::models::WorkflowStep> = session
            .steps_json
            .as_deref()
            .and_then(|json_str| serde_json::from_str(json_str).ok())
            .unwrap_or_default();
        let mut steps_changed = false;
        for step in &mut steps {
            let before = step.screenshot_ids.len();
            step.screenshot_ids.retain(|id| id != screenshot_id);
            if step.screenshot_ids.len() != before {
                steps_changed = true;
                // Annotations were drawn on this image; they mean nothing on another one.
                step.annotations_json = None;
            }
        }

        let doc_md = session.documentation_md.clone().unwrap_or_default();
        let new_md = replace_image_file_refs(&doc_md, file_name_of(&shot.path), None);
        if steps_changed || new_md != doc_md {
            db.save_documentation(
                &shot.session_id,
                &new_md,
                &serde_json::to_string(&steps)?,
                session.compressed_events_json.as_deref().unwrap_or("[]"),
            )?;
        }
    }

    db.delete_screenshot(screenshot_id)?;
    remove_screenshot_files(&shot.path);
    Ok(())
}

fn remove_screenshot_files(path: &str) {
    let p = std::path::Path::new(path);
    let _ = std::fs::remove_file(p);
    if let (Some(stem), Some(ext), Some(parent)) = (
        p.file_stem().and_then(|s| s.to_str()),
        p.extension().and_then(|e| e.to_str()),
        p.parent(),
    ) {
        let _ = std::fs::remove_file(parent.join(format!("{stem}_clean.{ext}")));
        let _ = std::fs::remove_file(parent.join(format!("{stem}_annotated.{ext}")));
    }
}

/// Rewrites every Markdown image whose target is the file `old_name` (whatever folder prefix
/// the reference uses): pointing it at `new_name`, or dropping the image when `None`.
fn replace_image_file_refs(md: &str, old_name: &str, new_name: Option<&str>) -> String {
    if old_name.is_empty() {
        return md.to_string();
    }
    let image_re = regex::Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").unwrap();
    let replaced = image_re.replace_all(md, |caps: &regex::Captures| {
        let target = caps[2].trim();
        if file_name_of(target) != old_name {
            return caps[0].to_string();
        }
        match new_name {
            Some(new_name) => {
                let prefix = &target[..target.len() - old_name.len()];
                format!("![{}]({prefix}{new_name})", &caps[1])
            }
            None => String::new(),
        }
    });
    if new_name.is_some() {
        return replaced.into_owned();
    }
    // Dropping an image leaves an empty line where it stood; don't let blank lines pile up.
    let mut out: Vec<&str> = Vec::new();
    for line in replaced.split('\n') {
        let blank = line.trim().is_empty();
        if blank && out.last().is_some_and(|prev| prev.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

#[tauri::command]
pub fn list_providers(state: State<'_, Arc<AppState>>) -> Result<Vec<ProviderConfig>, String> {
    state.db.list_providers().map_err(|err| err.to_string())
}

#[tauri::command]
pub fn update_provider(
    state: State<'_, Arc<AppState>>,
    provider: ProviderConfig,
) -> Result<(), String> {
    if provider.enabled == 1 {
        for mut existing in state.db.list_providers().map_err(|err| err.to_string())? {
            existing.enabled = 0;
            state
                .db
                .update_provider(&existing)
                .map_err(|err| err.to_string())?;
        }
    }
    state
        .db
        .update_provider(&provider)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn update_session_documentation(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    markdown: String,
) -> Result<(), String> {
    state
        .db
        .update_session_documentation(&session_id, &markdown)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn cancel_documentation_generation(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<bool, String> {
    let cancelled = state.cancel_ai_job(&session_id);
    let _ = state.db.update_session_status(&session_id, crate::storage::models::SessionStatus::Ready);
    Ok(cancelled)
}

#[tauri::command]
pub async fn generate_documentation(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<GenerateDocumentationResult, String> {
    let cancel_flag = state.register_ai_cancellation(&session_id);
    let pipeline = AiPipeline::new(Arc::clone(&state.db));
    let result = pipeline
        .run(&session_id, &app, cancel_flag)
        .await;

    state.remove_ai_cancellation(&session_id);

    match result {
        Ok(output) => Ok(GenerateDocumentationResult {
            markdown: output.markdown,
            redaction_summary: output.redaction_summary,
            usage: output.usage,
            unreported_calls: output.unreported_calls,
            cost_usd: output.cost_usd,
        }),
        Err(err) => {
            let _ = state.db.update_session_status(&session_id, crate::storage::models::SessionStatus::Ready);
            Err(err.to_string())
        }
    }
}

#[derive(serde::Serialize)]
pub struct GenerateDocumentationResult {
    pub markdown: String,
    pub redaction_summary: RedactionSummary,
    pub usage: crate::ai::providers::TokenUsage,
    pub unreported_calls: u32,
    pub cost_usd: Option<f64>,
}

/// Token / cost estimate shown before starting a generation.
/// Compresses and analyses every event of the session: off the UI thread for long recordings.
#[tauri::command]
pub async fn estimate_generation_cost(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<crate::ai::cost::CostEstimate, String> {
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || crate::ai::pipeline::estimate_generation_cost(&db, &session_id))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn list_ai_jobs(state: State<'_, Arc<AppState>>, session_id: String) -> Result<Vec<AiJob>, String> {
    state
        .db
        .list_ai_jobs(&session_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn export_session(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    format: String,
    options: Option<crate::export::ExportOptions>,
) -> Result<ExportRecord, String> {
    let db = Arc::clone(&state.db);
    let session_id = session_id.clone();
    let format = format.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let engine = ExportEngine::new(db);
        match format.as_str() {
            "markdown" => engine.export_markdown(&session_id, options),
            "html" => engine.export_html(&session_id, options),
            "pdf" => engine.export_pdf_with(&session_id, options, &|html, pdf, opts| {
                print_pdf(&app, html, pdf, opts)
            }),
            "video" => engine.export_video(&session_id),
            other => Err(anyhow!("unsupported export format: {other}")),
        }
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| err.to_string())
}

/// Windows: print with the app's own WebView2 first (independent of how Edge is installed or
/// managed), then fall back to an external headless browser. Elsewhere: headless browser.
fn print_pdf(
    app: &AppHandle,
    html: &std::path::Path,
    pdf: &std::path::Path,
    options: &crate::export::ExportOptions,
) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        match crate::export::webview_pdf::print_html_to_pdf(app, html, pdf, &options.page_size) {
            Ok(()) => return Ok(()),
            Err(err) => {
                crate::logger::write_entry("WARN", "export", &format!("WebView2 PDF printing failed, trying a browser: {err:#}"));
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, options);
    crate::export::try_print_pdf(html, pdf)
}

#[tauri::command]
pub fn list_exports(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<ExportRecord>, String> {
    state
        .db
        .list_exports(&session_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_setting(state: State<'_, Arc<AppState>>, key: String) -> Result<Option<String>, String> {
    state.db.get_setting(&key).map_err(|err| err.to_string())
}

#[tauri::command]
pub fn set_setting(state: State<'_, Arc<AppState>>, key: String, value: String) -> Result<(), String> {
    state
        .db
        .set_setting(&key, &value)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_compressed_timeline(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<crate::storage::models::SessionEvent>, String> {
    let events = state
        .db
        .list_events(&session_id)
        .map_err(|err| err.to_string())?
        .into_iter()
        .map(|event| crate::storage::models::SessionEvent {
            event_type: event.event_type,
            app_name: event.app_name,
            payload: serde_json::from_str(&event.payload).unwrap_or(serde_json::json!({})),
            timestamp_ms: event.timestamp_ms,
        })
        .collect::<Vec<_>>();
    Ok(crate::compression::compress_events(&events))
}

fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Line range `(header, end)` of step `step_index`'s section: from its `## Step N:` header up to
/// (not including) the next heading.
fn step_section(lines: &[&str], step_index: usize) -> Option<(usize, usize)> {
    let step_re = regex::Regex::new(r"(?i)^#{2,4}\s+(?:Step|Passo)?\s*(\d+)[:.]").unwrap();
    let boundary_re = regex::Regex::new(r"^#{1,4}\s").unwrap();
    let start = lines.iter().position(|line| {
        step_re
            .captures(line.trim())
            .and_then(|caps| caps.get(1)?.as_str().parse::<usize>().ok())
            == Some(step_index)
    })?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| boundary_re.is_match(line.trim_start()))
        .map(|(idx, _)| idx)
        .unwrap_or(lines.len());
    Some((start, end))
}

fn replace_step_image(md: &str, step_index: usize, old_path: &str, new_path: &str) -> String {
    let old_name = file_name_of(old_path);
    let new_name = file_name_of(new_path);
    if old_name.is_empty() || old_name == new_name {
        return md.to_string();
    }

    let lines: Vec<&str> = md.split('\n').collect();
    let Some((start, end)) = step_section(&lines, step_index) else {
        return replace_image_file_refs(md, old_name, Some(new_name));
    };

    lines
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            if idx > start && idx < end && line.contains("![") {
                replace_image_file_refs(line, old_name, Some(new_name))
            } else {
                (*line).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Applies a step title/description edit to that step's section of the Markdown only, whatever
/// heading style the document uses (`## Step 3.`, `### Passo 3:`…).
fn update_step_markdown(
    md: &str,
    step_index: usize,
    title: &str,
    old_description: &str,
    description: &str,
) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    let Some((start, end)) = step_section(&lines, step_index) else {
        return md.to_string();
    };

    let header_re =
        regex::Regex::new(r"(?i)^(\s*#{2,4}\s+(?:Step|Passo)?\s*\d+[:.]\s*)(.*)$").unwrap();
    let header = match header_re.captures(lines[start]) {
        Some(caps) if !title.trim().is_empty() => format!("{}{}", &caps[1], title.trim()),
        _ => lines[start].to_string(),
    };

    let body = lines[start + 1..end].join("\n");
    let old_description = old_description.trim();
    let new_body = if old_description == description.trim() {
        body
    } else if !old_description.is_empty() && body.contains(old_description) {
        body.replacen(old_description, description.trim(), 1)
    } else {
        // The Markdown text of this step doesn't match what the editor showed (e.g. the AI
        // rewrote it): the edited description replaces the step's text, images stay.
        let mut parts = vec![String::new()];
        if !description.trim().is_empty() {
            parts.push(description.trim().to_string());
            parts.push(String::new());
        }
        for image in lines[start + 1..end].iter().filter(|line| line.contains("![")) {
            parts.push(image.to_string());
            parts.push(String::new());
        }
        parts.join("\n")
    };

    let mut out: Vec<String> = lines[..start].iter().map(|l| l.to_string()).collect();
    out.push(header);
    out.push(new_body);
    out.extend(lines[end..].iter().map(|l| l.to_string()));
    out.join("\n")
}

/// Re-parsing the Markdown only yields titles, descriptions and images, so metadata that
/// lives exclusively in `steps_json` (step-scoped annotations, the "why", timestamps) would be
/// wiped on every save. Steps are matched by title first so that inserting a step and
/// renumbering the following ones keeps each step's metadata attached to the right step.
fn carry_over_step_metadata(
    parsed: &mut [crate::storage::models::WorkflowStep],
    previous: &[crate::storage::models::WorkflowStep],
) {
    let mut used = vec![false; previous.len()];
    let normalize = |value: &str| value.trim().to_lowercase();

    let mut matches: Vec<Option<usize>> = vec![None; parsed.len()];
    for (i, step) in parsed.iter().enumerate() {
        let title = normalize(&step.title);
        if title.is_empty() {
            continue;
        }
        if let Some(j) = (0..previous.len()).find(|&j| !used[j] && normalize(&previous[j].title) == title) {
            used[j] = true;
            matches[i] = Some(j);
        }
    }
    for (i, step) in parsed.iter().enumerate() {
        if matches[i].is_some() {
            continue;
        }
        let Some(first_shot) = step.screenshot_ids.first() else {
            continue;
        };
        if let Some(j) = (0..previous.len())
            .find(|&j| !used[j] && previous[j].screenshot_ids.first() == Some(first_shot))
        {
            used[j] = true;
            matches[i] = Some(j);
        }
    }

    for (step, matched) in parsed.iter_mut().zip(matches) {
        let Some(j) = matched else { continue };
        let old = &previous[j];
        let same_image = step.screenshot_ids.is_empty()
            || step.screenshot_ids.first() == old.screenshot_ids.first();
        // Annotations are drawn in the pixel space of one specific screenshot.
        if step.annotations_json.is_none() && same_image {
            step.annotations_json = old.annotations_json.clone();
        }
        if step.reason.is_none() {
            step.reason = old.reason.clone();
        }
        if step.timestamp_ms == 0 {
            step.timestamp_ms = old.timestamp_ms;
        }
        if step.screenshot_ids.is_empty() {
            step.screenshot_ids = old.screenshot_ids.clone();
        }
    }
}

fn parse_steps_from_markdown(md: &str, screenshots: &[crate::storage::models::Screenshot]) -> Vec<crate::storage::models::WorkflowStep> {
    let mut steps = Vec::new();
    let mut current_step: Option<crate::storage::models::WorkflowStep> = None;
    let step_header_re = regex::Regex::new(r"(?i)^#{2,4}\s+(?:Step|Passo)?\s*(\d+)[:.]\s*(.*)$").unwrap();
    let img_re = regex::Regex::new(r"!\[.*?\]\((.*?)\)").unwrap();

    for line in md.lines() {
        let trimmed = line.trim();
        if let Some(caps) = step_header_re.captures(trimmed) {
            if let Some(prev) = current_step.take() {
                steps.push(prev);
            }
            let step_num = caps.get(1).and_then(|m| m.as_str().parse::<usize>().ok()).unwrap_or(steps.len() + 1);
            let title = caps.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_else(|| format!("Step {}", step_num));
            current_step = Some(crate::storage::models::WorkflowStep {
                step: step_num,
                title,
                description: String::new(),
                reason: None,
                timestamp_ms: 0,
                screenshot_ids: Vec::new(),
                annotations_json: None,
            });
            continue;
        }

        if let Some(step) = &mut current_step {
            if let Some(caps) = img_re.captures(trimmed) {
                let img_path = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                let found_id = screenshots.iter().filter(|_| !img_path.is_empty()).find(|s| {
                    s.path == img_path
                        || s.path.ends_with(img_path)
                        || (!img_path.is_empty()
                            && s.path.ends_with(
                                std::path::Path::new(img_path)
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_str()
                                    .unwrap_or("___none___"),
                            ))
                }).map(|s| s.id.clone());

                if let Some(id) = found_id {
                    if !step.screenshot_ids.contains(&id) {
                        step.screenshot_ids.push(id);
                    }
                }
            } else if !trimmed.is_empty() {
                if !step.description.is_empty() {
                    step.description.push('\n');
                }
                step.description.push_str(trimmed);
            }
        }
    }

    if let Some(prev) = current_step {
        steps.push(prev);
    }

    steps
}

#[tauri::command]
pub fn get_replay_steps(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<crate::storage::models::WorkflowStep>, String> {
    let session = state
        .db
        .get_session(&session_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "session not found".to_string())?;

    if let Some(steps_json) = &session.steps_json {
        if let Ok(steps) = serde_json::from_str::<Vec<crate::storage::models::WorkflowStep>>(steps_json) {
            if !steps.is_empty() {
                return Ok(steps);
            }
        }
    }

    // Fallback: Recover steps from documentation_md if present!
    if let Some(doc_md) = &session.documentation_md {
        if !doc_md.trim().is_empty() {
            let screenshots = state.db.list_screenshots(&session_id).unwrap_or_default();
            let parsed_steps = parse_steps_from_markdown(doc_md, &screenshots);
            if !parsed_steps.is_empty() {
                if let Ok(json_str) = serde_json::to_string(&parsed_steps) {
                    let _ = state.db.save_documentation(
                        &session_id,
                        doc_md,
                        &json_str,
                        session.compressed_events_json.as_deref().unwrap_or("[]"),
                    );
                }
                return Ok(parsed_steps);
            }
        }
    }

    Ok(Vec::new())
}

#[tauri::command]
pub async fn save_session_audio(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    audio_base64: String,
    mime_type: String,
) -> Result<String, String> {
    use base64::Engine;
    let session_dir = state.db.session_dir(&session_id);
    let audio_dir = session_dir.join("audio");
    std::fs::create_dir_all(&audio_dir).map_err(|err| err.to_string())?;

    let ext = if mime_type.contains("wav") {
        "wav"
    } else if mime_type.contains("mp3") {
        "mp3"
    } else if mime_type.contains("ogg") {
        "ogg"
    } else {
        "webm"
    };

    let audio_path = audio_dir.join(format!("recording.{ext}"));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_base64.trim())
        .map_err(|err| format!("invalid audio base64: {err}"))?;

    std::fs::write(&audio_path, bytes).map_err(|err| err.to_string())?;

    let path_str = audio_path.to_string_lossy().to_string();
    state
        .db
        .update_session_audio(&session_id, &path_str)
        .map_err(|err| err.to_string())?;

    crate::media::spawn_session_full_video_finalize(
        state.db.clone(),
        app,
        session_id,
        Some(audio_path),
    );

    Ok(path_str)
}

#[tauri::command]
pub async fn transcribe_session_audio(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<String, String> {
    crate::ai::emit_log(&app, &session_id, "Inizializzazione trascrizione vocale del microfono...");

    let session = state
        .db
        .get_session(&session_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "session not found".to_string())?;

    let audio_path_str = session
        .audio_path
        .ok_or_else(|| "Nessun file audio registrato per questa sessione".to_string())?;
    let audio_path = std::path::PathBuf::from(&audio_path_str);
    if !audio_path.is_file() {
        let msg = "File audio non trovato su disco".to_string();
        crate::ai::emit_log(&app, &session_id, &format!("ERRORE: {msg}"));
        return Err(msg);
    }

    // Check if user set a dedicated provider, model or base URL for transcription
    let transcription_prov_id = state.db.get_setting("transcription_provider_id").unwrap_or(None);
    let transcription_model_override = state.db.get_setting("transcription_model").unwrap_or(None);
    let transcription_url_override = state.db.get_setting("transcription_base_url").unwrap_or(None);

    let mut provider = if let Some(prov_id) = transcription_prov_id.filter(|s| !s.trim().is_empty()) {
        state
            .db
            .list_providers()
            .map_err(|err| err.to_string())?
            .into_iter()
            .find(|p| p.id == prov_id)
            .unwrap_or(
                state
                    .db
                    .get_enabled_provider()
                    .map_err(|err| err.to_string())?
                    .ok_or_else(|| "Nessun provider AI configurato per la trascrizione".to_string())?
            )
    } else {
        state
            .db
            .get_enabled_provider()
            .map_err(|err| err.to_string())?
            .ok_or_else(|| "Nessun provider AI abilitato per la trascrizione".to_string())?
    };

    if let Some(model_override) = transcription_model_override.filter(|s| !s.trim().is_empty()) {
        provider.model = Some(model_override);
    }
    if let Some(url_override) = transcription_url_override.filter(|s| !s.trim().is_empty()) {
        provider.base_url = Some(url_override);
    }

    let transcription_lang = state.db.get_setting("transcription_language").unwrap_or(None);

    let filename = audio_path.file_name().and_then(|n| n.to_str()).unwrap_or("audio");
    let file_size = std::fs::metadata(&audio_path).map(|m| m.len()).unwrap_or(0);
    crate::ai::emit_log(
        &app,
        &session_id,
        &format!(
            "Connessione al servizio di trascrizione '{}' (endpoint: {:?}, modello: {:?})...",
            provider.name, provider.base_url, provider.model
        ),
    );
    crate::ai::emit_log(
        &app,
        &session_id,
        &format!("Invio file '{filename}' ({file_size} bytes, lingua: {:?}). In attesa...", transcription_lang),
    );

    let result = match crate::ai::transcription::transcribe_audio_with_segments(&provider, &audio_path, transcription_lang.as_deref()).await {
        Ok(res) => {
            let seg_count = res.segments.len();
            crate::ai::emit_log(
                &app,
                &session_id,
                &format!("Trascrizione vocale completata con successo ({} caratteri, {} segmenti temporizzati)!", res.text.len(), seg_count),
            );
            if seg_count > 0 {
                for (idx, seg) in res.segments.iter().take(3).enumerate() {
                    let preview = if seg.text.len() > 60 { format!("{}...", &seg.text[..60]) } else { seg.text.clone() };
                    crate::ai::emit_log(
                        &app,
                        &session_id,
                        &format!(
                            "  Seg. #{} [{:02}:{:02} - {:02}:{:02}]: \"{}\"",
                            idx + 1,
                            seg.start_ms / 60000, (seg.start_ms % 60000) / 1000,
                            seg.end_ms / 60000, (seg.end_ms % 60000) / 1000,
                            preview
                        ),
                    );
                }
                if seg_count > 3 {
                    crate::ai::emit_log(
                        &app,
                        &session_id,
                        &format!("  ... e altri {} segmenti salvati nel database.", seg_count - 3),
                    );
                }
            } else {
                crate::ai::emit_log(
                    &app,
                    &session_id,
                    "Nota: Trascrizione salvata. Il server Whisper non ha restituito segmenti temporizzati.",
                );
            }
            res
        }
        Err(err) => {
            let msg = format!("Errore durante la trascrizione audio: {err}");
            crate::ai::emit_log(&app, &session_id, &format!("ERRORE: {msg}"));
            return Err(msg);
        }
    };

    state
        .db
        .update_session_audio_transcript(&session_id, &result.text)
        .map_err(|err| err.to_string())?;

    if !result.segments.is_empty() {
        if let Ok(segs_json) = serde_json::to_string(&result.segments) {
            let _ = state.db.update_session_audio_segments(&session_id, &segs_json);
        }
    }

    crate::ai::emit_log(&app, &session_id, "Trascrizione e segmentazione vocale salvate con successo nel database!");
    Ok(result.text)
}

/// Removing a session folder (videos can be several GB) runs off the UI thread.
#[tauri::command]
pub async fn delete_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    state.cancel_ai_job(&session_id);
    state.remove_ai_cancellation(&session_id);
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || db.delete_session(&session_id))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn export_session_bundle(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    target_path: String,
) -> Result<String, String> {
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || {
        crate::storage::bundle::export_session_bundle(&db, &session_id, std::path::Path::new(&target_path))
    })
    .await
    .map_err(|err| err.to_string())?
    .map(|path| path.to_string_lossy().to_string())
    .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn import_session_bundle(
    state: State<'_, Arc<AppState>>,
    archive_path: String,
) -> Result<Session, String> {
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || {
        crate::storage::bundle::import_session_bundle(&db, std::path::Path::new(&archive_path))
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn translate_documentation(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    target_language: Option<String>,
) -> Result<String, String> {
    crate::ai::emit_log(&app, &session_id, "Inizializzazione traduzione della documentazione...");

    let session = state
        .db
        .get_session(&session_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "Sessione non trovata".to_string())?;

    let current_md = session
        .documentation_md
        .ok_or_else(|| "Nessuna documentazione generata per questa sessione".to_string())?;

    if current_md.trim().is_empty() {
        return Err("La documentazione è vuota".to_string());
    }

    // Estrai tutti i tag immagine del Markdown: ![alt](url)
    // Sostituiscili temporaneamente con un token compatto tipo <!-- FC_SCREENSHOT_0 -->
    // in modo che l'LLM riceva e traduca SOLO IL TESTO puro, senza alterare o confondere i percorsi immagine.
    let image_regex = regex::Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)")
        .map_err(|e| format!("Regex error: {e}"))?;

    let mut images: Vec<String> = Vec::new();
    let text_to_translate = image_regex
        .replace_all(&current_md, |caps: &regex::Captures| {
            let idx = images.len();
            images.push(caps[0].to_string());
            format!("<!-- FC_SCREENSHOT_{idx} -->")
        })
        .to_string();

    crate::ai::emit_log(
        &app,
        &session_id,
        &format!(
            "Isolate {} immagini: all'AI verrà inviato unicamente il testo puro.",
            images.len()
        ),
    );

    let provider_config = state
        .db
        .get_enabled_provider()
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "Nessun provider AI abilitato per la traduzione. Configura un provider in Impostazioni.".to_string())?;

    let llm = crate::ai::providers::build_provider(&provider_config)
        .map_err(|err| err.to_string())?;

    let lang = target_language.unwrap_or_else(|| "Italian".to_string());
    let provider_name = provider_config.name.clone();
    let model_name = provider_config.model.clone().unwrap_or_else(|| "default".to_string());
    let endpoint = provider_config.base_url.clone().unwrap_or_else(|| "default".to_string());

    crate::ai::emit_log(
        &app,
        &session_id,
        &format!(
            "Connessione a '{provider_name}' ({endpoint}) con modello '{model_name}'..."
        ),
    );

    let char_count = text_to_translate.len();
    let word_count = text_to_translate.split_whitespace().count();
    crate::ai::emit_log(
        &app,
        &session_id,
        &format!(
            "Invio testo al server AI ({char_count} caratteri, ~{word_count} parole). In attesa della traduzione in {lang}..."
        ),
    );

    let prompt = crate::ai::prompts::translate_guide(&lang, &text_to_translate);

    let options = crate::ai::providers::get_ai_generate_options(
        &state.db,
        crate::ai::providers::AiOperation::Translation,
    );

    crate::ai::emit_log(
        &app,
        &session_id,
        "Applicata modalità No-Think (ragionamento interno disabilitato per traduzione diretta)...",
    );

    let translated_res = llm.generate(&prompt.system, &prompt.user, &options).await;
    let translated_raw = match translated_res {
        Ok(response) => {
            let usage_note = response
                .usage
                .map(|u| format!(", {} token in ingresso / {} in uscita", u.input_tokens, u.output_tokens))
                .unwrap_or_default();
            crate::ai::emit_log(
                &app,
                &session_id,
                &format!(
                    "Risposta ricevuta con successo dal server AI ({} caratteri{usage_note})!",
                    response.text.len()
                ),
            );
            response.text
        }
        Err(err) => {
            let err_msg = format!("Errore durante la traduzione con {provider_name}: {err}");
            crate::ai::emit_log(&app, &session_id, &format!("ERRORE: {err_msg}"));
            return Err(err_msg);
        }
    };

    crate::ai::emit_log(&app, &session_id, "Ripristino dei riferimenti originali agli screenshot...");
    let mut final_md = translated_raw.trim().to_string();

    // Rimuovi eventuali ```markdown e ``` che alcuni modelli avvolgono attorno alla risposta
    if final_md.starts_with("```markdown") {
        final_md = final_md.trim_start_matches("```markdown").to_string();
    } else if final_md.starts_with("```") {
        final_md = final_md.trim_start_matches("```").to_string();
    }
    if final_md.ends_with("```") {
        final_md = final_md.trim_end_matches("```").to_string();
    }
    final_md = final_md.trim().to_string();

    for (idx, original_tag) in images.iter().enumerate() {
        let placeholder = format!("<!-- FC_SCREENSHOT_{idx} -->");
        if final_md.contains(&placeholder) {
            final_md = final_md.replace(&placeholder, original_tag);
        } else {
            // Se il modello ha per errore rimosso il placeholder, riaccoda l'immagine per non perderla
            final_md.push_str(&format!("\n\n{original_tag}\n"));
        }
    }

    state
        .db
        .update_session_documentation(&session_id, &final_md)
        .map_err(|err| err.to_string())?;

    crate::ai::emit_log(&app, &session_id, "Documentazione tradotta salvata nel database!");
    crate::ai::emit_log(&app, &session_id, &format!("Traduzione in {lang} completata con successo!"));

    Ok(final_md)
}

#[derive(serde::Deserialize)]
pub struct NewStepPayload {
    pub title: String,
    pub description: String,
}

/// Decoding a 4K PNG sent as base64 and writing it takes a moment: run it off the UI thread.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn save_annotated_screenshot(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    screenshot_id: String,
    image_base64: String,
    click_x: Option<i64>,
    click_y: Option<i64>,
    new_step: Option<NewStepPayload>,
    annotations_json: Option<String>,
) -> Result<crate::storage::models::Screenshot, String> {
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || {
        save_annotated_screenshot_blocking(
            &db,
            session_id,
            screenshot_id,
            image_base64,
            click_x,
            click_y,
            new_step,
            annotations_json,
        )
    })
    .await
    .map_err(|err| err.to_string())?
}

#[allow(clippy::too_many_arguments)]
fn save_annotated_screenshot_blocking(
    db: &crate::storage::Database,
    session_id: String,
    screenshot_id: String,
    image_base64: String,
    click_x: Option<i64>,
    click_y: Option<i64>,
    new_step: Option<NewStepPayload>,
    annotations_json: Option<String>,
) -> Result<crate::storage::models::Screenshot, String> {
    let screenshots = db.list_screenshots(&session_id).map_err(|err| err.to_string())?;
    let screenshot = screenshots
        .into_iter()
        .find(|s| s.id == screenshot_id)
        .ok_or_else(|| "Screenshot not found".to_string())?;

    let original_path = std::path::PathBuf::from(&screenshot.path);
    // Sessions from older versions may still have the click drawn on the original.
    crate::export::restore_legacy_original(&original_path);

    // CRITICAL CONSTRAINT: NEVER overwrite raw screenshot.path!
    // Annotations are stored as structured JSON metadata in SQLite (annotations_json).
    // An optional annotated preview cache can be kept as {stem}_annotated.{ext}.
    if !image_base64.is_empty() {
        let raw_base64 = if let Some(idx) = image_base64.find("base64,") {
            &image_base64[idx + 7..]
        } else {
            &image_base64
        };
        use base64::Engine;
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(raw_base64.trim()) {
            let parent = original_path.parent().unwrap_or(std::path::Path::new(""));
            let stem = original_path.file_stem().unwrap_or_default().to_string_lossy();
            let ext = original_path.extension().unwrap_or_default().to_string_lossy();
            let annotated_cache = parent.join(format!("{stem}_annotated.{ext}"));
            let _ = std::fs::write(annotated_cache, &bytes);
        }
    }

    db.update_screenshot_annotations(&screenshot_id, annotations_json.as_deref(), click_x, click_y)
        .map_err(|err| err.to_string())?;

    if let Some(step_data) = new_step {
        if let Ok(Some(session)) = db.get_session(&session_id) {
            let mut steps: Vec<crate::storage::models::WorkflowStep> = session
                .steps_json
                .as_deref()
                .and_then(|json_str| serde_json::from_str(json_str).ok())
                .unwrap_or_default();

            let next_step_num = steps.len() + 1;
            let new_step_obj = crate::storage::models::WorkflowStep {
                step: next_step_num,
                title: step_data.title.clone(),
                description: step_data.description.clone(),
                reason: None,
                timestamp_ms: screenshot.timestamp_ms,
                screenshot_ids: vec![screenshot.id.clone()],
                annotations_json: annotations_json.clone(),
            };
            steps.push(new_step_obj);

            let new_steps_json = serde_json::to_string(&steps).unwrap_or_default();

            let mut md = session.documentation_md.unwrap_or_default();
            let img_line = format!("![Step {}]({})\n\n", next_step_num, screenshot.path);
            md.push_str(&format!(
                "\n\n### Step {}: {}\n\n{}\n\n{}",
                next_step_num,
                step_data.title,
                step_data.description,
                img_line
            ));

            let _ = db.save_documentation(
                &session_id,
                &md,
                &new_steps_json,
                session.compressed_events_json.as_deref().unwrap_or("[]"),
            );
        }
    }

    let updated_screenshots = db.list_screenshots(&session_id).map_err(|err| err.to_string())?;
    updated_screenshots
        .into_iter()
        .find(|s| s.id == screenshot_id)
        .ok_or_else(|| "Screenshot not found after save".to_string())
}

#[tauri::command]
pub fn update_step_screenshot(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    step_index: usize,
    screenshot_id: String,
) -> Result<(), String> {
    let session = state.db.get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Session not found".to_string())?;

    let screenshots = state.db.list_screenshots(&session_id).map_err(|e| e.to_string())?;
    let target_screenshot = screenshots.iter().find(|s| s.id == screenshot_id)
        .ok_or_else(|| "Screenshot not found".to_string())?;

    let mut steps: Vec<crate::storage::models::WorkflowStep> = session
        .steps_json
        .as_deref()
        .and_then(|json_str| serde_json::from_str(json_str).ok())
        .unwrap_or_default();

    let mut old_screenshot_path: Option<String> = None;
    if let Some(step) = steps.iter_mut().find(|s| s.step == step_index) {
        if let Some(first_id) = step.screenshot_ids.first() {
            if let Some(old_s) = screenshots.iter().find(|s| &s.id == first_id) {
                old_screenshot_path = Some(old_s.path.clone());
            }
        }
        step.screenshot_ids = vec![screenshot_id.clone()];
    } else {
        return Err(format!("Step {} not found", step_index));
    }

    let new_steps_json = serde_json::to_string(&steps).map_err(|e| e.to_string())?;

    // Also update documentation_md. The Markdown references images as `screenshots/<file>`
    // (or a bare/absolute path), so match on the unique file name, and only inside this step's
    // section so a screenshot shared with other steps is left alone there.
    let mut doc_md = session.documentation_md.clone().unwrap_or_default();
    if let Some(old_path) = old_screenshot_path {
        doc_md = replace_step_image(&doc_md, step_index, &old_path, &target_screenshot.path);
    }

    state.db.save_documentation(
        &session_id,
        &doc_md,
        &new_steps_json,
        session.compressed_events_json.as_deref().unwrap_or("[]"),
    ).map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn update_step_content(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    step_index: usize,
    title: String,
    description: String,
) -> Result<(), String> {
    let session = state.db.get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Session not found".to_string())?;

    let mut steps: Vec<crate::storage::models::WorkflowStep> = session
        .steps_json
        .as_deref()
        .and_then(|json_str| serde_json::from_str(json_str).ok())
        .unwrap_or_default();

    let (old_title, old_description) = if let Some(step) = steps.iter_mut().find(|s| s.step == step_index) {
        let old_t = step.title.clone();
        let old_d = step.description.clone();
        step.title = title.clone();
        step.description = description.clone();
        (old_t, old_d)
    } else {
        return Err(format!("Step {} not found", step_index));
    };

    let new_steps_json = serde_json::to_string(&steps).map_err(|e| e.to_string())?;

    let doc_md = session.documentation_md.clone().unwrap_or_default();
    let doc_md = if old_title == title && old_description == description {
        doc_md
    } else {
        update_step_markdown(&doc_md, step_index, &title, &old_description, &description)
    };

    state.db.save_documentation(
        &session_id,
        &doc_md,
        &new_steps_json,
        session.compressed_events_json.as_deref().unwrap_or("[]"),
    ).map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn update_documentation(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    documentation_md: String,
) -> Result<(), String> {
    let session = state.db.get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Session not found".to_string())?;

    let screenshots = state.db.list_screenshots(&session_id).unwrap_or_default();
    let previous_steps: Vec<crate::storage::models::WorkflowStep> = session
        .steps_json
        .as_deref()
        .and_then(|json_str| serde_json::from_str(json_str).ok())
        .unwrap_or_default();
    let mut parsed_steps = parse_steps_from_markdown(&documentation_md, &screenshots);
    carry_over_step_metadata(&mut parsed_steps, &previous_steps);
    let previous_had_step_headings = session
        .documentation_md
        .as_deref()
        .is_some_and(|md| !parse_steps_from_markdown(md, &screenshots).is_empty());
    let steps_json = if !parsed_steps.is_empty() {
        serde_json::to_string(&parsed_steps).unwrap_or_else(|_| session.steps_json.unwrap_or_else(|| "[]".to_string()))
    } else if documentation_md.trim().is_empty() || previous_had_step_headings {
        // The user removed the steps (or the whole guide): the replay must not keep showing them.
        "[]".to_string()
    } else {
        // The guide never used "### Step N:" headings (steps came from the AI pipeline in
        // another shape): keep the steps we have.
        session.steps_json.unwrap_or_else(|| "[]".to_string())
    };

    state.db.save_documentation(
        &session_id,
        &documentation_md,
        &steps_json,
        session.compressed_events_json.as_deref().unwrap_or("[]"),
    ).map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn save_step_annotations(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    step_index: usize,
    annotations_json: Option<String>,
) -> Result<(), String> {
    let session = state.db.get_session(&session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Session not found".to_string())?;

    let mut steps: Vec<crate::storage::models::WorkflowStep> = session
        .steps_json
        .as_deref()
        .and_then(|json_str| serde_json::from_str(json_str).ok())
        .unwrap_or_default();

    if let Some(step) = steps.iter_mut().find(|s| s.step == step_index) {
        step.annotations_json = annotations_json;
    } else {
        return Err(format!("Step {} not found", step_index));
    }

    let new_steps_json = serde_json::to_string(&steps).map_err(|e| e.to_string())?;

    state.db.save_documentation(
        &session_id,
        session.documentation_md.as_deref().unwrap_or(""),
        &new_steps_json,
        session.compressed_events_json.as_deref().unwrap_or("[]"),
    ).map_err(|e| e.to_string())?;

    Ok(())
}

/// Side of the grayscale thumbnail screenshots are compared on.
const DUPLICATE_THUMB_SIDE: u32 = 64;
/// Per-pixel difference (0–255) below which two thumbnail pixels count as equal.
const DUPLICATE_PIXEL_TOLERANCE: u8 = 24;
/// Default share of equal pixels needed to call two screenshots duplicates (the user can change
/// it: setting `duplicate_min_similarity`, in percent).
const DUPLICATE_MIN_SIMILARITY: f64 = 0.97;
const DUPLICATE_SIMILARITY_SETTING: &str = "duplicate_min_similarity";
/// Below this, "duplicates" would group screenshots that merely share a layout.
const DUPLICATE_SIMILARITY_FLOOR: f64 = 0.5;

/// Threshold in percent (as typed by the user) -> ratio, clamped to a meaningful range.
fn duplicate_threshold(percent: Option<f64>) -> f64 {
    percent
        .filter(|p| p.is_finite())
        .map(|p| (p / 100.0).clamp(DUPLICATE_SIMILARITY_FLOOR, 1.0))
        .unwrap_or(DUPLICATE_MIN_SIMILARITY)
}

fn thumbnail_similarity(a: &[u8], b: &[u8]) -> f64 {
    let total = a.len().min(b.len()).max(1);
    let changed = a
        .iter()
        .zip(b)
        .filter(|(&x, &y)| x.abs_diff(y) > DUPLICATE_PIXEL_TOLERANCE)
        .count();
    1.0 - changed as f64 / total as f64
}

/// Groups near-identical screenshots. Every member of a group must match every other member
/// (complete linkage), so a slowly changing screen doesn't chain unrelated shots together.
fn group_duplicates(thumbs: &[Vec<u8>], min_similarity: f64) -> Vec<(Vec<usize>, f64)> {
    let n = thumbs.len();
    let mut sim = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let value = thumbnail_similarity(&thumbs[i], &thumbs[j]);
            sim[i][j] = value;
            sim[j][i] = value;
        }
    }

    let mut assigned = vec![false; n];
    let mut groups = Vec::new();
    for i in 0..n {
        if assigned[i] {
            continue;
        }
        let mut members = vec![i];
        for j in (i + 1)..n {
            if !assigned[j] && members.iter().all(|&m| sim[m][j] >= min_similarity) {
                members.push(j);
            }
        }
        if members.len() < 2 {
            continue;
        }
        let mut total = 0.0;
        let mut pairs = 0;
        for (idx, &a) in members.iter().enumerate() {
            for &b in &members[idx + 1..] {
                total += sim[a][b];
                pairs += 1;
            }
        }
        for &m in &members {
            assigned[m] = true;
        }
        groups.push((members, total / pairs as f64));
    }
    groups
}

#[tauri::command]
pub async fn find_duplicate_screenshots(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    min_similarity: Option<f64>,
) -> Result<Vec<crate::storage::models::DuplicateScreenshotGroup>, String> {
    let db = Arc::clone(&state.db);
    // Explicit value (percent) from the dialog, else the one saved in Settings.
    let percent = min_similarity.or_else(|| {
        db.get_setting(DUPLICATE_SIMILARITY_SETTING)
            .ok()
            .flatten()
            .and_then(|value| value.trim().parse::<f64>().ok())
    });
    let threshold = duplicate_threshold(percent);
    tauri::async_runtime::spawn_blocking(move || find_duplicate_screenshots_blocking(&db, &session_id, threshold))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}

fn find_duplicate_screenshots_blocking(
    db: &crate::storage::Database,
    session_id: &str,
    min_similarity: f64,
) -> anyhow::Result<Vec<crate::storage::models::DuplicateScreenshotGroup>> {
    let screenshots = db.list_screenshots(session_id)?;
    if screenshots.len() < 2 {
        return Ok(Vec::new());
    }

    // Decoding full-resolution PNGs dominates the cost: spread it over the available cores.
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    let chunk = screenshots.len().div_ceil(workers);
    let thumbs: Vec<(String, Vec<u8>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = screenshots
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .filter_map(|shot| {
                            let img = image::open(&shot.path).ok()?;
                            let gray = img
                                .thumbnail_exact(DUPLICATE_THUMB_SIDE, DUPLICATE_THUMB_SIDE)
                                .to_luma8();
                            Some((shot.id.clone(), gray.into_raw()))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    });

    if thumbs.len() < 2 {
        return Ok(Vec::new());
    }

    let pixels: Vec<Vec<u8>> = thumbs.iter().map(|(_, px)| px.clone()).collect();
    Ok(group_duplicates(&pixels, min_similarity)
        .into_iter()
        .enumerate()
        .map(|(idx, (members, similarity))| crate::storage::models::DuplicateScreenshotGroup {
            group_id: format!("group_{}", idx + 1),
            similarity_pct: (similarity * 1000.0).round() / 10.0,
            screenshot_ids: members.iter().map(|&m| thumbs[m].0.clone()).collect(),
        })
        .collect())
}

#[tauri::command]
pub async fn merge_duplicate_screenshots(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    keep_id: String,
    remove_ids: Vec<String>,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || {
        merge_duplicate_screenshots_blocking(&db, &session_id, &keep_id, &remove_ids)
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| err.to_string())
}

fn merge_duplicate_screenshots_blocking(
    db: &crate::storage::Database,
    session_id: &str,
    keep_id: &str,
    remove_ids: &[String],
) -> anyhow::Result<()> {
    let remove_ids: Vec<&String> = remove_ids.iter().filter(|id| id.as_str() != keep_id).collect();
    if remove_ids.is_empty() {
        return Ok(());
    }

    let session = db
        .get_session(session_id)?
        .ok_or_else(|| anyhow!("Session not found"))?;

    let screenshots = db.list_screenshots(session_id)?;
    let keep_screenshot = screenshots
        .iter()
        .find(|s| s.id == keep_id)
        .ok_or_else(|| anyhow!("Keep screenshot not found"))?;

    let remove_paths: Vec<String> = screenshots
        .iter()
        .filter(|s| remove_ids.contains(&&s.id))
        .map(|s| s.path.clone())
        .collect();

    // 1. Update steps_json: replace any remove_id with keep_id
    let mut steps: Vec<crate::storage::models::WorkflowStep> = session
        .steps_json
        .as_deref()
        .and_then(|json_str| serde_json::from_str(json_str).ok())
        .unwrap_or_default();

    for step in steps.iter_mut() {
        let mut new_ids: Vec<String> = Vec::new();
        for id in &step.screenshot_ids {
            let id = if remove_ids.contains(&id) { keep_id } else { id.as_str() };
            if !new_ids.iter().any(|existing| existing == id) {
                new_ids.push(id.to_string());
            }
        }
        step.screenshot_ids = new_ids;
    }
    let new_steps_json = serde_json::to_string(&steps)?;

    // 2. Point the Markdown images at the kept file. References use `screenshots/<file>`, an
    //    absolute path or a bare name, so match on the file name.
    let mut doc_md = session.documentation_md.unwrap_or_default();
    let keep_name = file_name_of(&keep_screenshot.path);
    for rem_path in &remove_paths {
        doc_md = replace_image_file_refs(&doc_md, file_name_of(rem_path), Some(keep_name));
    }

    db.save_documentation(
        session_id,
        &doc_md,
        &new_steps_json,
        session.compressed_events_json.as_deref().unwrap_or("[]"),
    )?;

    // 3. Delete removed screenshots from db and remove files from disk
    for rem_id in &remove_ids {
        let _ = db.delete_screenshot(rem_id);
    }
    for rem_path in &remove_paths {
        remove_screenshot_files(rem_path);
    }

    Ok(())
}

fn open_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(path).spawn();
}

#[tauri::command]
pub fn open_logs_folder() -> Result<String, String> {
    let log_dir = crate::logger::get_log_dir();
    open_in_file_manager(&log_dir);
    Ok(log_dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn open_browser_extension_folder(app: AppHandle) -> Result<String, String> {
    use tauri::Manager;

    // Installed builds ship the extension as a bundled resource; in development it lives at
    // the repository root, next to `src-tauri`.
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("browser-extension"));
    }
    candidates.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("browser-extension"));

    let folder = candidates
        .into_iter()
        .find(|dir| dir.join("manifest.json").is_file())
        .ok_or_else(|| "Cartella dell'estensione browser non trovata".to_string())?;
    let folder = dunce_canonicalize(&folder);
    open_in_file_manager(&folder);
    Ok(folder.to_string_lossy().to_string())
}

/// `canonicalize` without the `\\?\` prefix Windows adds, which Explorer doesn't open.
fn dunce_canonicalize(path: &std::path::Path) -> std::path::PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(stripped) if !stripped.starts_with("UNC\\") => std::path::PathBuf::from(stripped),
        _ => canonical,
    }
}

#[tauri::command]
pub fn get_browser_bridge_status(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "port": crate::platform::browser_bridge::DEFAULT_BRIDGE_PORT,
        "recording": state.browser_bridge.is_recording(),
    }))
}




#[cfg(test)]
mod doc_step_tests {
    use super::{
        carry_over_step_metadata, duplicate_threshold, group_duplicates, DUPLICATE_MIN_SIMILARITY,
        DUPLICATE_SIMILARITY_FLOOR, replace_image_file_refs, replace_step_image,
        update_step_markdown,
    };
    use crate::storage::models::WorkflowStep;

    #[test]
    fn step_edit_only_touches_its_own_section() {
        let md = "## Step 1. Apri\nClicca OK\n\n## Step 2. Salva\nClicca OK\n![s](screenshots/a.png)\n";
        let out = update_step_markdown(md, 2, "Salva tutto", "Clicca OK", "Premi Ctrl+S");
        assert_eq!(
            out,
            "## Step 1. Apri\nClicca OK\n\n## Step 2. Salva tutto\nPremi Ctrl+S\n![s](screenshots/a.png)\n"
        );
    }

    #[test]
    fn step_edit_replaces_text_the_editor_did_not_show_but_keeps_images() {
        let md = "### Passo 1: Apri\nTesto riscritto dall'AI.\n\n![a](screenshots/a.png)\n## Risultato\nFine";
        let out = update_step_markdown(md, 1, "Apri", "Descrizione originale", "Nuova descrizione");
        assert_eq!(
            out,
            "### Passo 1: Apri\n\nNuova descrizione\n\n![a](screenshots/a.png)\n\n## Risultato\nFine"
        );
    }

    #[test]
    fn image_refs_are_matched_by_file_name() {
        let md = "a\n\n![x](screenshots/old.png)\n\nb\n![y](C:\\data\\screenshots\\old.png)";
        assert_eq!(
            replace_image_file_refs(md, "old.png", Some("new.png")),
            "a\n\n![x](screenshots/new.png)\n\nb\n![y](C:\\data\\screenshots\\new.png)"
        );
        assert_eq!(replace_image_file_refs(md, "old.png", None), "a\n\nb\n");
    }

    #[test]
    fn duplicates_need_near_identical_images_and_do_not_chain() {
        let base = vec![100u8; 64 * 64];
        let mut tiny_change = base.clone();
        tiny_change[..40].fill(255); // 1 % of the pixels
        let mut drift = tiny_change.clone();
        drift[40..160].fill(255); // ~4 % away from `base`, <3 % from `tiny_change`
        let different = vec![20u8; 64 * 64];

        let thumbs = [base, tiny_change, drift, different];
        let groups = group_duplicates(&thumbs, DUPLICATE_MIN_SIMILARITY);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, vec![0, 1]);

        // A lower threshold chosen by the user also accepts the drifted shot…
        let loose = group_duplicates(&thumbs, 0.95);
        assert_eq!(loose[0].0, vec![0, 1, 2]);
        // …and 100 % only keeps pixel-identical thumbnails.
        assert!(group_duplicates(&thumbs, 1.0).is_empty());
    }

    #[test]
    fn duplicate_threshold_is_clamped() {
        assert_eq!(duplicate_threshold(None), DUPLICATE_MIN_SIMILARITY);
        assert_eq!(duplicate_threshold(Some(90.0)), 0.9);
        assert_eq!(duplicate_threshold(Some(10.0)), DUPLICATE_SIMILARITY_FLOOR);
        assert_eq!(duplicate_threshold(Some(150.0)), 1.0);
        assert_eq!(duplicate_threshold(Some(f64::NAN)), DUPLICATE_MIN_SIMILARITY);
    }

    fn step(n: usize, title: &str, shot: Option<&str>) -> WorkflowStep {
        WorkflowStep {
            step: n,
            title: title.into(),
            description: String::new(),
            reason: None,
            timestamp_ms: 0,
            screenshot_ids: shot.map(|s| vec![s.to_string()]).unwrap_or_default(),
            annotations_json: None,
        }
    }

    #[test]
    fn replaces_image_only_inside_the_target_step() {
        let md = "### Step 1: A\n![a](screenshots/old.png)\n\n### Step 2: B\n![b](screenshots/old.png)\n";
        let out = replace_step_image(md, 2, "/data/s/screenshots/old.png", r"C:\data\screenshots\new.png");
        assert_eq!(out, "### Step 1: A\n![a](screenshots/old.png)\n\n### Step 2: B\n![b](screenshots/new.png)\n");
    }

    #[test]
    fn keeps_metadata_when_a_step_is_inserted_and_renumbered() {
        let mut previous = vec![step(1, "Apri", Some("s1")), step(2, "Clicca", Some("s2"))];
        previous[1].annotations_json = Some("[{\"type\":\"badge\"}]".into());
        previous[1].reason = Some("perché".into());
        previous[1].timestamp_ms = 42;

        let mut parsed = vec![step(1, "Apri", Some("s1")), step(2, "Nuovo", None), step(3, "Clicca", Some("s2"))];
        carry_over_step_metadata(&mut parsed, &previous);
        assert_eq!(parsed[2].annotations_json.as_deref(), Some("[{\"type\":\"badge\"}]"));
        assert_eq!(parsed[2].reason.as_deref(), Some("perché"));
        assert_eq!(parsed[2].timestamp_ms, 42);
        assert!(parsed[1].annotations_json.is_none());
    }

    #[test]
    fn drops_annotations_when_the_image_changed() {
        let mut previous = vec![step(1, "Apri", Some("s1"))];
        previous[0].annotations_json = Some("[]".into());
        let mut parsed = vec![step(1, "Apri", Some("s9"))];
        carry_over_step_metadata(&mut parsed, &previous);
        assert!(parsed[0].annotations_json.is_none());
    }
}

/// Folder where every session is kept as a readable copy (`None` = feature off).
#[tauri::command]
pub fn get_session_mirror_dir(state: State<'_, Arc<AppState>>) -> Result<Option<String>, String> {
    Ok(crate::storage::mirror::mirror_root(&state.db).map(|p| p.to_string_lossy().to_string()))
}

/// Sets (or clears, with `None`) the sessions folder and copies every session into it.
/// Returns how many sessions are being written. Async: a network folder can take long to answer.
#[tauri::command]
pub async fn set_session_mirror_dir(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
) -> Result<usize, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let value = path.map(|p| p.trim().to_string()).unwrap_or_default();
        if !value.is_empty() {
            std::fs::create_dir_all(&value).map_err(|err| format!("Cartella non utilizzabile: {err}"))?;
        }
        state
            .db
            .set_setting(crate::storage::mirror::MIRROR_DIR_SETTING, &value)
            .map_err(|err| err.to_string())?;
        if value.is_empty() {
            return Ok(0);
        }
        crate::storage::mirror::sync_all(&state.db, &state.mirror).map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

#[derive(serde::Serialize)]
pub struct MirrorDirCheck {
    pub configured: bool,
    pub reachable: bool,
    pub path: Option<String>,
}

/// Whether the sessions folder can be reached (checked at startup: a network share needs the VPN).
/// `configured` is false when no folder is set or writing into it is turned off.
/// Gives up after a few seconds: an unreachable share can take a minute to fail.
#[tauri::command]
pub async fn check_session_mirror_dir(state: State<'_, Arc<AppState>>) -> Result<MirrorDirCheck, String> {
    let Some(root) = crate::storage::mirror::mirror_write_root(&state.db) else {
        return Ok(MirrorDirCheck { configured: false, reachable: false, path: None });
    };
    let path = Some(root.to_string_lossy().to_string());
    let probe = tauri::async_runtime::spawn_blocking(move || {
        root.is_dir() || std::fs::create_dir_all(&root).is_ok()
    });
    let reachable = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(8), probe).await,
        Ok(Ok(true))
    );
    Ok(MirrorDirCheck { configured: true, reachable, path })
}

/// Whether the FlowCapture Companion extension is installed and enabled in the browsers of this
/// computer (read from their profiles, no browser needs to be open).
#[tauri::command]
pub async fn get_browser_extension_status(
    state: State<'_, Arc<AppState>>,
) -> Result<crate::platform::browser_extensions::BrowserExtensionStatus, String> {
    let required = crate::platform::browser_extensions::extension_required(&state.db);
    tauri::async_runtime::spawn_blocking(move || crate::platform::browser_extensions::check(required))
        .await
        .map_err(|err| err.to_string())
}

/// FlowCapture was started with `--admin` (shows the admin tools, e.g. "Importa da cartella").
#[tauri::command]
pub fn is_admin_mode() -> bool {
    std::env::args().skip(1).any(|arg| arg.eq_ignore_ascii_case("--admin"))
}

/// Admin only: imports from the sessions folder the sessions recorded on other computers, only
/// the new and the changed ones.
#[tauri::command]
pub async fn import_sessions_from_mirror(
    state: State<'_, Arc<AppState>>,
) -> Result<crate::storage::folder_import::FolderImportSummary, String> {
    if !is_admin_mode() {
        return Err("Funzione riservata agli amministratori (avvia FlowCapture con --admin)".to_string());
    }
    let root = crate::storage::mirror::admin_import_root(&state.db)
        .ok_or_else(|| "Nessuna cartella sessioni configurata: impostala in Impostazioni".to_string())?;
    let db = Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || crate::storage::folder_import::import_changed_sessions(&db, &root))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| format!("{err:#}"))
}

/// Re-copies every session into the sessions folder (e.g. after files were removed by hand).
#[tauri::command]
pub fn sync_session_mirror(state: State<'_, Arc<AppState>>) -> Result<usize, String> {
    if crate::storage::mirror::mirror_write_root(&state.db).is_none() {
        return Err(if is_admin_mode() {
            "Nessuna cartella \"Copia completa\" configurata (Impostazioni)".to_string()
        } else if crate::storage::mirror::mirror_root(&state.db).is_none() {
            "Nessuna cartella sessioni configurata".to_string()
        } else {
            "La scrittura nella cartella sessioni è disattivata (Impostazioni → Cartella sessioni)".to_string()
        });
    }
    crate::storage::mirror::sync_all(&state.db, &state.mirror).map_err(|err| err.to_string())
}

/// Admin only: folder receiving the complete, unzipped copy of every session.
#[tauri::command]
pub fn get_session_full_copy_dir(state: State<'_, Arc<AppState>>) -> Option<String> {
    crate::storage::mirror::full_copy_root(&state.db).map(|p| p.to_string_lossy().to_string())
}

/// Admin only: sets (or clears, with `None`) the "Copia completa" folder and copies every session
/// into it. Returns how many sessions are being written.
#[tauri::command]
pub async fn set_session_full_copy_dir(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
) -> Result<usize, String> {
    if !is_admin_mode() {
        return Err("Funzione riservata agli amministratori (avvia FlowCapture con --admin)".to_string());
    }
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let value = path.map(|p| p.trim().to_string()).unwrap_or_default();
        if !value.is_empty() {
            std::fs::create_dir_all(&value).map_err(|err| format!("Cartella non utilizzabile: {err}"))?;
        }
        state
            .db
            .set_setting(crate::storage::mirror::FULL_COPY_DIR_SETTING, &value)
            .map_err(|err| err.to_string())?;
        if value.is_empty() {
            return Ok(0);
        }
        crate::storage::mirror::sync_all(&state.db, &state.mirror).map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

#[tauri::command]
pub async fn open_session_full_copy_dir(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let root = crate::storage::mirror::full_copy_root(&state.db)
        .ok_or_else(|| "Nessuna cartella \"Copia completa\" configurata".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&root).map_err(|err| format!("Cartella non raggiungibile ({err})."))?;
        open_in_file_manager(&root);
        Ok(())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Sessions that could not be copied to the sessions folder yet (e.g. network share offline).
#[tauri::command]
pub fn get_session_mirror_status(state: State<'_, Arc<AppState>>) -> crate::storage::mirror::MirrorStatus {
    state.mirror.status()
}

#[tauri::command]
pub async fn open_session_mirror_dir(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let root = crate::storage::mirror::mirror_root(&state.db)
        .ok_or_else(|| "Nessuna cartella sessioni configurata".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&root).map_err(|err| {
            format!("Cartella sessioni non raggiungibile ({err}). Controlla di essere collegato alla VPN.")
        })?;
        open_in_file_manager(&root);
        Ok(())
    })
    .await
    .map_err(|err| err.to_string())?
}
