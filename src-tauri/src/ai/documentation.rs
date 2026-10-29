use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde_json::json;

use crate::events::is_flowcapture_app;
use crate::ai::prompts::DocLanguage;
use crate::storage::models::{Screenshot, SessionEvent, WorkflowStep};

pub fn is_web_browser(app_name: &str) -> bool {
    let lower = app_name.to_lowercase();
    lower.contains("chrome")
        || lower.contains("edge")
        || lower.contains("firefox")
        || lower.contains("mozilla")
        || lower.contains("brave")
        || lower.contains("opera")
        || lower.contains("safari")
        || lower.contains("vivaldi")
}

pub fn format_browser_name(app_name: &str) -> &'static str {
    let lower = app_name.to_lowercase();
    if lower.contains("chrome") {
        "Google Chrome"
    } else if lower.contains("edge") {
        "Microsoft Edge"
    } else if lower.contains("firefox") || lower.contains("mozilla") {
        "Mozilla Firefox"
    } else if lower.contains("brave") {
        "Brave"
    } else if lower.contains("opera") {
        "Opera"
    } else if lower.contains("safari") {
        "Safari"
    } else {
        "Browser"
    }
}

pub fn clean_browser_window_title(title: &str, browser_name: &str) -> String {
    let mut cleaned = title.trim();
    for suffix in &[
        " - Google Chrome",
        " - Microsoft Edge",
        " - Microsoft\u{200B} Edge",
        " - Mozilla Firefox",
        " - Brave",
        " - Opera",
        " - Google",
        " — Mozilla Firefox",
    ] {
        if let Some(stripped) = cleaned.strip_suffix(suffix) {
            cleaned = stripped.trim();
        }
    }
    if cleaned.eq_ignore_ascii_case(browser_name) {
        return String::new();
    }
    cleaned.to_string()
}

pub fn build_workflow_steps(events: &[SessionEvent]) -> Vec<WorkflowStep> {
    let mut steps = Vec::new();
    let mut last_window: Option<(String, String)> = None;
    let mut last_click_key: Option<String> = None;

    for event in events {
        if should_skip_event(event) {
            continue;
        }

        match event.event_type.as_str() {
            "window_focus" => {
                let app = event
                    .app_name
                    .clone()
                    .or_else(|| event.payload.get("app_name").and_then(|v| v.as_str()).map(str::to_string))
                    .unwrap_or_else(|| "Application".to_string());
                let title = event
                    .payload
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&app)
                    .to_string();

                if last_window.as_ref() == Some(&(app.clone(), title.clone())) {
                    continue;
                }
                last_window = Some((app.clone(), title.clone()));
                last_click_key = None;

                let (step_title, step_desc) = if is_web_browser(&app) {
                    let bname = format_browser_name(&app);
                    let page_title = clean_browser_window_title(&title, bname);
                    let is_empty_page = page_title.is_empty()
                        || page_title.eq_ignore_ascii_case("nuova scheda")
                        || page_title.eq_ignore_ascii_case("new tab")
                        || page_title.eq_ignore_ascii_case("blank");

                    if is_empty_page {
                        (
                            "Vai sul browser".to_string(),
                            format!("Apri il browser (**{bname}**)."),
                        )
                    } else {
                        (
                            format!("Vai sul browser - {page_title}"),
                            format!("Vai sul browser (**{bname}**) e apri **{page_title}**."),
                        )
                    }
                } else {
                    (
                        format!("Open {title}"),
                        format!("Switch to **{app}** and open **{title}**."),
                    )
                };

                steps.push(WorkflowStep {
                    step: 0,
                    title: step_title,
                    description: step_desc,
                    reason: None,
                    timestamp_ms: event.timestamp_ms,
                    screenshot_ids: Vec::new(),
                    annotations_json: None,
                });
            }
            "mouse_click" | "mouse_double_click" => {
                let double = event.event_type == "mouse_double_click";
                let button = event
                    .payload
                    .get("button")
                    .and_then(|v| v.as_str())
                    .unwrap_or("left");
                let x = event.payload.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
                let y = event.payload.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
                let elem_name = event
                    .payload
                    .get("element_name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let elem_type = event
                    .payload
                    .get("element_type")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let elem_url = event
                    .payload
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());

                let context = last_window
                    .as_ref()
                    .map(|(app, title)| {
                        if is_web_browser(app) {
                            let bname = format_browser_name(app);
                            let ptitle = clean_browser_window_title(title, bname);
                            if ptitle.is_empty() {
                                format!(" nel browser (**{bname}**)")
                            } else {
                                format!(" nel browser su **{ptitle}** ({bname})")
                            }
                        } else {
                            format!(" in **{title}** ({app})")
                        }
                    })
                    .unwrap_or_default();
                let click_key = format!("{button}:{double}:{x}:{y}:{context}:{:?}", elem_name);

                if last_click_key.as_deref() == Some(click_key.as_str()) {
                    continue;
                }
                last_click_key = Some(click_key);

                let (title_text, desc_text) = match elem_name {
                    Some(name) => {
                        let action = match button {
                            "right" => "Right-click",
                            "middle" => "Middle-click",
                            _ if double => "Double-click",
                            _ => "Click",
                        };
                        let type_desc = elem_type
                            .map(|t| format!(" ({t})"))
                            .unwrap_or_default();
                        let title = format!("{action} \"{name}\"");
                        let mut desc = format!("{action} on **{name}**{type_desc}{context}.");
                        if let Some(url) = elem_url {
                            desc.push_str(&format!(" (URL: `{url}`)"));
                        }
                        (title, desc)
                    }
                    None => {
                        let title = if double {
                            format!("{button} double-click")
                        } else {
                            format!("{button}-click")
                        };
                        let desc = format!(
                            "{}{}.",
                            match button {
                                "right" => "Right-click the target element",
                                "middle" => "Middle-click the target element",
                                _ if double => "Double-click the target element",
                                _ => "Click the target element",
                            },
                            context
                        );
                        (title, desc)
                    }
                };

                steps.push(WorkflowStep {
                    step: 0,
                    title: title_text,
                    description: desc_text,
                    reason: None,
                    timestamp_ms: event.timestamp_ms,
                    screenshot_ids: Vec::new(),
                    annotations_json: None,
                });
            }
            "typed_text" => {
                let text = event
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if text.trim().is_empty() {
                    continue;
                }
                let app = event
                    .app_name
                    .clone()
                    .unwrap_or_else(|| "the active application".to_string());
                steps.push(WorkflowStep {
                    step: 0,
                    title: "Enter text".to_string(),
                    description: format!("In **{app}**, type `{text}`."),
                    reason: None,
                    timestamp_ms: event.timestamp_ms,
                    screenshot_ids: Vec::new(),
                    annotations_json: None,
                });
            }
            "shortcut_press" | "key_press" => {
                let key = event.payload.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let label = match event.payload.get("combo").and_then(|v| v.as_str()) {
                    Some(combo) => combo
                        .split('+')
                        .map(crate::compression::display_key_name)
                        .collect::<Vec<_>>()
                        .join("+"),
                    None => crate::compression::display_key_name(key),
                };
                if label.is_empty() {
                    continue;
                }
                let app = event.app_name.clone();

                // "Type X, then press Enter" reads better than two separate steps.
                if matches!(label.as_str(), "Enter" | "Tab") {
                    if let Some(last) = steps.last_mut() {
                        let same_app = app.as_deref().is_none_or(|a| last.description.contains(&format!("**{a}**")));
                        if last.title == "Enter text"
                            && same_app
                            && (event.timestamp_ms - last.timestamp_ms).abs() < 10_000
                        {
                            let sentence = last.description.trim_end_matches('.').to_string();
                            last.description = format!("{sentence}, then press **{label}**.");
                            continue;
                        }
                    }
                }

                let where_ = app.map(|a| format!(" in **{a}**")).unwrap_or_default();
                steps.push(WorkflowStep {
                    step: 0,
                    title: format!("Press {label}"),
                    description: format!("Press **{label}**{where_}."),
                    reason: None,
                    timestamp_ms: event.timestamp_ms,
                    screenshot_ids: Vec::new(),
                    annotations_json: None,
                });
            }
            "manual_marker" => {
                steps.push(WorkflowStep {
                    step: 0,
                    title: "Manual step".to_string(),
                    description: "Complete the marked manual step before continuing.".to_string(),
                    reason: None,
                    timestamp_ms: event.timestamp_ms,
                    screenshot_ids: Vec::new(),
                    annotations_json: None,
                });
            }
            _ => {}
        }
    }

    renumber_steps(&mut steps);
    collapse_duplicate_steps(&mut steps);
    // Removed the arbitrary 150-step truncation to preserve the complete workflow.
    // if steps.len() > 150 {
    //     steps.truncate(150);
    // }
    renumber_steps(&mut steps);
    steps
}

pub fn match_screenshots_to_steps(steps: &mut [WorkflowStep], screenshots: &[Screenshot]) {
    if screenshots.is_empty() {
        return;
    }

    for step in steps.iter_mut() {
        step.screenshot_ids.clear();
        if let Some(shot) = nearest_screenshot(step.timestamp_ms, screenshots) {
            step.screenshot_ids.push(shot.id.clone());
        }
    }
}

pub fn infer_workflow_summary(
    session_title: &str,
    events: &[SessionEvent],
    steps: &[WorkflowStep],
    audio_transcript: Option<&str>,
) -> (String, String) {
    let mut panes = HashSet::new();
    let mut apps = HashSet::new();

    for event in events {
        if let Some(app) = &event.app_name {
            if !is_flowcapture_app(app) {
                apps.insert(app.clone());
            }
        }
        if event.event_type == "window_focus" {
            if let Some(title) = event.payload.get("title").and_then(|v| v.as_str()) {
                if !title.is_empty() && title != "FlowCapture" {
                    panes.insert(title.to_string());
                }
            }
        }
    }

    // Try to extract a meaningful summary from the audio transcript
    let transcript_hint = audio_transcript
        .filter(|t| !t.trim().is_empty())
        .map(|t| {
            // Take the first ~120 chars of the transcript as context hint
            let trimmed = t.trim();
            if trimmed.len() > 120 {
                format!("{}...", &trimmed[..trimmed.char_indices().take(120).last().map(|(i, _)| i).unwrap_or(120)])
            } else {
                trimmed.to_string()
            }
        });

    let title = if is_generic_session_title(session_title) {
        if let Some(ref hint) = transcript_hint {
            // Use the first sentence of the transcript as a title hint
            let first_sentence = hint.split(['.', '!', '?']).next().unwrap_or(hint).trim();
            if first_sentence.len() > 10 && first_sentence.len() < 80 {
                format!("How to: {first_sentence}")
            } else if panes.len() == 1 {
                format!("How to use {}", panes.iter().next().unwrap())
            } else if panes.is_empty() {
                "Workflow Documentation".to_string()
            } else {
                format!("Workflow: {}", summarize_panes(&panes))
            }
        } else if panes.is_empty() {
            "Workflow Documentation".to_string()
        } else if panes.len() == 1 {
            format!("How to use {}", panes.iter().next().unwrap())
        } else {
            format!("Workflow: {}", summarize_panes(&panes))
        }
    } else {
        session_title.to_string()
    };

    let overview = if steps.is_empty() {
        "This workflow was recorded with FlowCapture.".to_string()
    } else if let Some(ref hint) = transcript_hint {
        format!(
            "This guide walks through {} steps to complete **{}**. The user explained: \"{}\"",
            steps.len(),
            title,
            hint
        )
    } else if panes.is_empty() {
        format!(
            "This guide walks through {} recorded actions to complete **{}**.",
            steps.len(),
            title
        )
    } else {
        format!(
            "This guide documents how to work with {} across {} steps, including navigating {}.",
            apps.into_iter().collect::<Vec<_>>().join(", "),
            steps.len(),
            summarize_panes(&panes)
        )
    };

    (title, overview)
}

/// Builds the base guide from the steps. Headings follow `lang` and match what the exporters
/// recognise (`## Prerequisiti`, `### Passo N:`, `> **Perché:**`, ...).
pub fn render_documentation_markdown(
    lang: DocLanguage,
    title: &str,
    overview: &str,
    steps: &[WorkflowStep],
    screenshots: &[Screenshot],
) -> String {
    let h = lang.headings();
    let mut output = format!(
        "# {title}\n\n{overview}\n\n## {}\n\n{}\n\n## {}\n\n",
        h.prerequisites, h.default_prerequisites, h.steps
    );
    let lookup = screenshot_lookup(screenshots);

    for step in steps {
        output.push_str(&format!("### {} {}: {}\n\n", h.step_prefix, step.step, step.title));
        output.push_str(&format!("{}\n\n", step.description));
        if let Some(reason) = &step.reason {
            if !reason.trim().is_empty() {
                output.push_str(&format!("> **{}:** {}\n\n", h.why, reason.trim()));
            }
        }
        if let Some(id) = step.screenshot_ids.first() {
            if let Some(shot) = lookup.get(id.as_str()) {
                output.push_str(&format!(
                    "![{} {}]({})\n\n",
                    h.step_prefix,
                    step.step,
                    markdown_image_path(shot)
                ));
            }
        }
    }

    output.push_str(&format!("## {}\n\n{}\n", h.expected_result, h.default_expected_result));

    output.trim().to_string()
}

pub fn sanitize_markdown(markdown: &str) -> String {
    let placeholder = Regex::new(r"\[[^\]]*(?:MISSING|SPECIFY|SPECIFIC|Describe|List any|TBD|TODO)[^\]]*\]")
        .expect("valid regex");
    let bracket_action =
        Regex::new(r"(?m)^\s*[\*\-]\s+\*\*[^*]+\*\*:\s*\[[^\]]+\]\s*$").expect("valid regex");

    let mut cleaned = markdown.to_string();
    cleaned = placeholder.replace_all(&cleaned, "").to_string();
    cleaned = bracket_action.replace_all(&cleaned, "").to_string();

    cleaned
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.contains("[MISSING SCREENSHOT]")
                && !trimmed.contains("[Specify")
                && !trimmed.contains("[SPECIFIC")
                && !trimmed.contains("[Describe what")
                && !trimmed.contains("[List any")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\n\n\n\n", "\n\n")
        .replace("\n\n\n", "\n\n")
        .trim()
        .to_string()
}

pub fn normalize_screenshot_references(markdown: &str, screenshots: &[Screenshot]) -> String {
    let image_re =
        Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").expect("valid markdown image regex");

    image_re
        .replace_all(markdown, |caps: &regex::Captures| {
            let alt = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let reference = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if let Some(shot) = find_screenshot_for_reference(reference, screenshots) {
                format!("![{}]({})", alt, markdown_image_path(shot))
            } else {
                caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string()
            }
        })
        .into_owned()
}

pub fn strip_screenshot_references(markdown: &str, screenshots: &[Screenshot]) -> String {
    let image_re =
        Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").expect("valid markdown image regex");

    let stripped = image_re
        .replace_all(markdown, |caps: &regex::Captures| {
            let reference = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            if find_screenshot_for_reference(reference, screenshots).is_some() {
                String::new()
            } else {
                caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string()
            }
        })
        .into_owned();

    stripped
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\n\n\n\n", "\n\n")
        .replace("\n\n\n", "\n\n")
        .trim()
        .to_string()
}

pub fn export_markdown_image_path(shot: &Screenshot) -> String {
    format!("../screenshots/{}", screenshot_filename(shot))
}

pub fn markdown_image_path(shot: &Screenshot) -> String {
    format!("screenshots/{}", screenshot_filename(shot))
}

fn find_screenshot_for_reference<'a>(
    reference: &str,
    screenshots: &'a [Screenshot],
) -> Option<&'a Screenshot> {
    let reference = reference.trim();

    for shot in screenshots {
        if reference == markdown_image_path(shot)
            || reference == export_markdown_image_path(shot)
            || reference == shot.path
        {
            return Some(shot);
        }
    }

    for shot in screenshots {
        let filename = screenshot_filename(shot);
        if reference == filename
            || reference.ends_with(&format!("/{filename}"))
            || reference.contains(&shot.id)
        {
            return Some(shot);
        }
    }

    None
}

fn screenshot_filename(shot: &Screenshot) -> String {
    std::path::Path::new(&shot.path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&shot.id)
        .to_string()
}

fn screenshot_lookup(screenshots: &[Screenshot]) -> HashMap<&str, &Screenshot> {
    screenshots.iter().map(|shot| (shot.id.as_str(), shot)).collect()
}

fn nearest_screenshot(timestamp_ms: i64, screenshots: &[Screenshot]) -> Option<&Screenshot> {
    screenshots
        .iter()
        .min_by_key(|shot| (shot.timestamp_ms - timestamp_ms).unsigned_abs())
}

/// Interactions with FlowCapture itself (Stop, Pause, renaming the session…) are not part of
/// the documented workflow.
fn should_skip_event(event: &SessionEvent) -> bool {
    event
        .app_name
        .as_deref()
        .is_some_and(is_flowcapture_app)
}

fn is_generic_session_title(title: &str) -> bool {
    let normalized = title.trim().to_lowercase();
    normalized.is_empty()
        || normalized == "nuova sessione"
        || normalized == "new workflow session"
        || normalized.starts_with("new workflow")
}

fn summarize_panes(panes: &HashSet<String>) -> String {
    let mut items: Vec<_> = panes.iter().cloned().collect();
    items.sort();
    match items.len() {
        0 => "the recorded workflow".to_string(),
        1 => items[0].clone(),
        2 => format!("{} and {}", items[0], items[1]),
        _ => format!("{}, {}, and more", items[0], items[1]),
    }
}

fn renumber_steps(steps: &mut [WorkflowStep]) {
    for (index, step) in steps.iter_mut().enumerate() {
        step.step = index + 1;
    }
}

fn collapse_duplicate_steps(steps: &mut Vec<WorkflowStep>) {
    let mut collapsed: Vec<WorkflowStep> = Vec::with_capacity(steps.len());
    for step in steps.drain(..) {
        if let Some(last) = collapsed.last_mut() {
            if last.title == step.title
                && last.description == step.description
                && (step.timestamp_ms - last.timestamp_ms).unsigned_abs() < 1500
            {
                continue;
            }
        }
        collapsed.push(step);
    }
    *steps = collapsed;
}

pub fn events_for_prompt(events: &[SessionEvent]) -> serde_json::Value {
    let compact: Vec<_> = events
        .iter()
        .take(90000)
        .map(|event| {
            json!({
                "type": event.event_type,
                "app": event.app_name,
                "payload": event.payload,
                "timestamp_ms": event.timestamp_ms,
            })
        })
        .collect();
    json!(compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(n: usize, title: &str, reason: Option<&str>) -> WorkflowStep {
        WorkflowStep {
            step: n,
            title: title.to_string(),
            description: format!("Descrizione {n}"),
            reason: reason.map(str::to_string),
            timestamp_ms: n as i64 * 1000,
            screenshot_ids: Vec::new(),
            annotations_json: None,
        }
    }

    /// The base template must stay readable by the styled HTML exporter in both languages.
    #[test]
    fn rendered_template_is_parsed_by_exporter() {
        let steps = vec![step(1, "Apri il browser", Some("Serve per il login")), step(2, "Clicca Salva", None)];
        for lang in [DocLanguage::Italian, DocLanguage::English] {
            let md = render_documentation_markdown(lang, "Titolo", "Panoramica", &steps, &[]);
            let parsed = crate::export::styled_html::parse_documentation_markdown(&md, &[], "Sessione", &[]);
            assert_eq!(parsed.title, "Titolo");
            assert_eq!(parsed.steps.len(), 2, "{lang:?}: {md}");
            assert_eq!(parsed.steps[0].title, "Apri il browser");
            assert_eq!(parsed.steps[0].reason.as_deref(), Some("Serve per il login"));
            assert!(parsed.prerequisites.is_some(), "{lang:?}");
            assert!(parsed.expected_result.is_some(), "{lang:?}");
        }
    }
}
