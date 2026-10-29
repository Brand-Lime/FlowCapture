pub(crate) mod styled_html;
#[cfg(target_os = "windows")]
pub mod webview_pdf;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use uuid::Uuid;

pub use styled_html::{ExportOptions, render_styled_export_html};
use crate::ai::documentation::{
    infer_workflow_summary, markdown_image_path, render_documentation_markdown,
    strip_screenshot_references,
};
use crate::media::{ensure_session_mp4, resolve_ffmpeg};
use crate::storage::Database;
use crate::storage::models::{ExportRecord, Session, Screenshot, WorkflowStep};

pub struct ExportEngine {
    db: Arc<Database>,
}

impl ExportEngine {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub fn export_markdown(
        &self,
        session_id: &str,
        options: Option<ExportOptions>,
    ) -> Result<ExportRecord> {
        let options = options.unwrap_or_default().normalized();
        let session = self.require_session(session_id)?;
        let screenshots = self.db.list_screenshots(session_id)?;
        prepare_screenshots_for_export(&screenshots, highlight_clicks_enabled(&self.db));
        let markdown = self.resolve_markdown(session_id, &session, &screenshots)?;
        let export_path = self.export_path(session_id, &session, "md");
        let rewritten = if options.screenshots {
            // Images are copied next to the file (exports/screenshots/) so the .md and its
            // "screenshots" folder can be moved together.
            let normalized = crate::ai::documentation::normalize_screenshot_references(&markdown, &screenshots);
            let images_dir = export_path.parent().map(|p| p.join("screenshots")).context("percorso di esportazione non valido")?;
            copy_referenced_screenshots(&normalized, &screenshots, &images_dir)?;
            normalized
        } else {
            strip_screenshot_references(&markdown, &screenshots)
        };
        std::fs::write(&export_path, rewritten)?;
        self.persist_export(session_id, "markdown", export_path)
    }

    pub fn export_html(
        &self,
        session_id: &str,
        options: Option<ExportOptions>,
    ) -> Result<ExportRecord> {
        let session = self.require_session(session_id)?;
        let screenshots = self.db.list_screenshots(session_id)?;
        prepare_screenshots_for_export(&screenshots, highlight_clicks_enabled(&self.db));
        let steps = self.load_steps(session_id)?;
        let html = render_styled_export_html(
            &session,
            &steps,
            &screenshots,
            &options.unwrap_or_default(),
            "html",
        );
        let path = self.export_path(session_id, &session, "html");
        std::fs::write(&path, html)?;
        self.persist_export(session_id, "html", path)
    }

    /// PDF export. `printer(html_path, pdf_path, options)` turns the rendered page into the PDF:
    /// the app's WebView2 on Windows, a headless browser (`try_print_pdf`) elsewhere.
    pub fn export_pdf_with(
        &self,
        session_id: &str,
        options: Option<ExportOptions>,
        printer: &dyn Fn(&Path, &Path, &ExportOptions) -> Result<()>,
    ) -> Result<ExportRecord> {
        let options = options.unwrap_or_default().normalized();
        let session = self.require_session(session_id)?;
        let screenshots = self.db.list_screenshots(session_id)?;
        prepare_screenshots_for_export(&screenshots, highlight_clicks_enabled(&self.db));
        let steps = self.load_steps(session_id)?;
        let html = render_styled_export_html(&session, &steps, &screenshots, &options, "pdf");
        let pdf_path = self.export_path(session_id, &session, "pdf");
        // Intermediate page for the printer: not an export, removed afterwards.
        let html_path = pdf_path.with_extension("print.html");
        std::fs::write(&html_path, html)?;
        let printed = printer(&html_path, &pdf_path, &options);
        let _ = std::fs::remove_file(&html_path);
        if printed.is_err() {
            let _ = std::fs::remove_file(&pdf_path);
        }
        printed.context("esportazione PDF non riuscita")?;
        self.persist_export(session_id, "pdf", pdf_path)
    }

    pub fn export_video(&self, session_id: &str) -> Result<ExportRecord> {
        let session = self.require_session(session_id)?;
        let session_dir = self.db.session_dir(session_id);
        // Prefer the full-resolution recording; the slideshow is a low-fps fallback.
        let full_video = session
            .full_video_path
            .as_deref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "mp4"));
        let mp4 = match full_video {
            Some(path) => path,
            None => match ensure_session_mp4(
            &session_dir,
            session.video_path.as_deref(),
            session.duration,
        )? {
            Some(path) => path,
            None if resolve_ffmpeg().is_none() => {
                anyhow::bail!("La creazione del video non è disponibile: manca ffmpeg in questa installazione.")
            }
            None => anyhow::bail!("Nessun video registrato per questa sessione"),
            },
        };
        let path = self.export_path(session_id, &session, "mp4");
        std::fs::copy(&mp4, &path)?;
        self.persist_export(session_id, "video", path)
    }

    fn resolve_markdown(
        &self,
        session_id: &str,
        session: &Session,
        screenshots: &[Screenshot],
    ) -> Result<String> {
        if let Some(value) = &session.documentation_md {
            if !value.trim().is_empty() {
                return Ok(value.clone());
            }
        }
        self.fallback_markdown(session_id, session, screenshots)
    }

    fn require_session(&self, session_id: &str) -> Result<Session> {
        self.db
            .get_session(session_id)?
            .context("sessione non trovata")
    }

    fn load_steps(&self, session_id: &str) -> Result<Vec<WorkflowStep>> {
        let session = self.require_session(session_id)?;
        if let Some(steps_json) = session.steps_json {
            Ok(serde_json::from_str(&steps_json)?)
        } else {
            Ok(Vec::new())
        }
    }

    fn fallback_markdown(
        &self,
        session_id: &str,
        session: &Session,
        screenshots: &[Screenshot],
    ) -> Result<String> {
        let steps = self.load_steps(session_id)?;
        let (title, overview) = infer_workflow_summary(&session.title, &[], &steps, None);
        Ok(render_documentation_markdown(
            crate::ai::prompts::DocLanguage::from_db(&self.db),
            &title,
            &overview,
            &steps,
            screenshots,
        ))
    }

    /// Readable, unique file name: "<session title>_<date-time>.<ext>".
    fn export_path(&self, session_id: &str, session: &Session, extension: &str) -> PathBuf {
        let dir = self.db.session_dir(session_id).join("exports");
        let _ = std::fs::create_dir_all(&dir);
        let base = format!(
            "{}_{}",
            file_name_slug(&session.title),
            chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
        );
        let mut candidate = dir.join(format!("{base}.{extension}"));
        let mut counter = 2;
        while candidate.exists() {
            candidate = dir.join(format!("{base}_{counter}.{extension}"));
            counter += 1;
        }
        candidate
    }

    fn persist_export(&self, session_id: &str, format: &str, path: PathBuf) -> Result<ExportRecord> {
        let export = ExportRecord {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            format: format.to_string(),
            path: path.to_string_lossy().to_string(),
            created_at: Utc::now().to_rfc3339(),
        };
        self.db.create_export(&export)?;
        self.db.update_session_status(
            session_id,
            crate::storage::models::SessionStatus::Exported,
        )?;
        Ok(export)
    }
}

/// Renames the exported files of a session after its title changed, so they keep matching the
/// session name: "<old title>_<date>.pdf" becomes "<new title>_<date>.pdf". Files from older
/// versions named "export_<uuid>.<ext>" get the new scheme too. Returns how many were renamed.
pub fn rename_session_exports(db: &Database, session_id: &str, new_title: &str) -> Result<usize> {
    let slug = file_name_slug(new_title);
    let mut renamed = 0;
    for record in db.list_exports(session_id)? {
        let path = PathBuf::from(&record.path);
        if !path.is_file() {
            continue;
        }
        let (Some(dir), Some(stem), Some(ext)) = (
            path.parent(),
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        let Some(suffix) = export_name_suffix(stem, &record.created_at) else {
            continue;
        };
        let new_stem = format!("{slug}_{suffix}");
        if new_stem == stem {
            continue;
        }
        let mut target = dir.join(format!("{new_stem}.{ext}"));
        let mut counter = 2;
        while target.exists() {
            target = dir.join(format!("{new_stem}_{counter}.{ext}"));
            counter += 1;
        }
        std::fs::rename(&path, &target)
            .with_context(|| format!("rinomina di {} in {}", path.display(), target.display()))?;
        db.update_export_path(&record.id, &target.to_string_lossy())?;
        renamed += 1;
    }
    Ok(renamed)
}

/// The part of an export file name that does not depend on the session title: the
/// "<date>_<time>[_n]" tail of current names, or the creation time for legacy GUID names.
fn export_name_suffix(stem: &str, created_at: &str) -> Option<String> {
    static TAIL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static LEGACY: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tail = TAIL.get_or_init(|| {
        regex::Regex::new(r"_(\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}(?:_\d+)?)$").unwrap()
    });
    if let Some(caps) = tail.captures(stem) {
        return Some(caps[1].to_string());
    }
    let legacy = LEGACY.get_or_init(|| {
        regex::Regex::new(r"(?i)^export_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap()
    });
    if legacy.is_match(stem) {
        let created = chrono::DateTime::parse_from_rfc3339(created_at)
            .map(|dt| dt.with_timezone(&chrono::Local))
            .unwrap_or_else(|_| chrono::Local::now());
        return Some(created.format("%Y-%m-%d_%H-%M-%S").to_string());
    }
    None
}

/// Session title -> safe file name (keeps letters incl. accents, digits, '-' and '_').
pub(crate) fn file_name_slug(title: &str) -> String {
    let slug: String = title
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let slug = slug
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let slug: String = slug.chars().take(60).collect();
    if slug.is_empty() {
        "FlowCapture".to_string()
    } else {
        slug
    }
}

/// Copies the screenshots referenced by the guide into `images_dir`, preferring the
/// annotated / click-highlighted variant, under the name the Markdown already uses.
fn copy_referenced_screenshots(markdown: &str, screenshots: &[Screenshot], images_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(images_dir)?;
    for shot in screenshots {
        let reference = markdown_image_path(shot);
        if !markdown.contains(&reference) {
            continue;
        }
        let Some(name) = Path::new(&reference).file_name() else {
            continue;
        };
        let original = Path::new(&shot.path);
        let annotated = original.with_file_name(format!(
            "{}_annotated.{}",
            original.file_stem().unwrap_or_default().to_string_lossy(),
            original.extension().unwrap_or_default().to_string_lossy()
        ));
        let source = if annotated.is_file() { annotated } else { original.to_path_buf() };
        if source.is_file() {
            std::fs::copy(&source, images_dir.join(name))
                .with_context(|| format!("copia di {}", source.display()))?;
        }
    }
    Ok(())
}

const PDF_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn try_print_pdf(html_path: &Path, pdf_path: &Path) -> Result<()> {
    let html_url = path_to_file_url(&html_path.to_string_lossy())?;
    let pdf_arg = pdf_path.to_string_lossy().to_string();
    let mut failures: Vec<String> = Vec::new();

    let candidates = chrome_binary_candidates();
    for chrome in &candidates {
        // `--headless=new` needs Chromium/Edge >= 112; older builds only know `--headless`.
        for headless_flag in ["--headless=new", "--headless"] {
            let _ = std::fs::remove_file(pdf_path);
            // A throw-away profile keeps headless Edge/Chrome from handing the job off to an
            // already running browser (Edge "startup boost" does this on Windows and the
            // process then exits without ever writing the PDF).
            let profile_dir = std::env::temp_dir().join(format!("flowcapture-pdf-{}", Uuid::new_v4()));
            let mut command = crate::process_util::background_command(chrome);
            command.args([
                headless_flag,
                "--disable-gpu",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--disable-background-networking",
                "--disable-extensions",
                "--disable-sync",
                "--disable-features=msEdgeStartupBoost,Translate",
                "--no-first-run",
                "--no-default-browser-check",
                "--allow-file-access-from-files",
                "--no-pdf-header-footer",
                "--print-to-pdf-no-header",
                "--run-all-compositor-stages-before-draw",
                "--virtual-time-budget=10000",
                &format!("--user-data-dir={}", profile_dir.display()),
                &format!("--print-to-pdf={pdf_arg}"),
                &html_url,
            ]);

            let result = run_command_with_timeout(command, PDF_TIMEOUT);
            let _ = std::fs::remove_dir_all(&profile_dir);
            match result {
                Ok(_) if is_valid_pdf(pdf_path) => return Ok(()),
                Ok(status) => failures.push(format!("{chrome} {headless_flag}: exited with {status}")),
                Err(err) => failures.push(format!("{chrome} {headless_flag}: {err}")),
            }
        }
    }

    let mut wkhtml = crate::process_util::background_command("wkhtmltopdf");
    wkhtml.args([
        "--enable-local-file-access",
        "--print-media-type",
        html_path.to_string_lossy().as_ref(),
        pdf_path.to_string_lossy().as_ref(),
    ]);
    match run_command_with_timeout(wkhtml, PDF_TIMEOUT) {
        Ok(_) if is_valid_pdf(pdf_path) => return Ok(()),
        Ok(status) => failures.push(format!("wkhtmltopdf: exited with {status}")),
        Err(err) => failures.push(format!("wkhtmltopdf: {err}")),
    }

    for failure in &failures {
        eprintln!("FlowCapture: PDF render attempt failed: {failure}");
    }
    if candidates.is_empty() {
        anyhow::bail!(
            "Impossibile creare il PDF: nessun browser compatibile trovato. Installa Microsoft Edge, Google Chrome, Chromium o wkhtmltopdf."
        )
    }
    anyhow::bail!("Impossibile creare il PDF. Tentativi: {}", failures.join("; "))
}

pub(crate) fn is_valid_pdf(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 5];
    file.read_exact(&mut header).is_ok() && &header == b"%PDF-"
}

/// Runs a command with stdout/stderr discarded (Chromium logs heavily; piping without
/// draining can fill the pipe buffer and hang the child until the timeout). Callers build it
/// with `background_command`, so no console window flashes on Windows.
fn run_command_with_timeout(
    mut command: std::process::Command,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    use std::process::Stdio;
    use std::thread;
    use std::time::Instant;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = command.spawn()?;
    let start = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("comando interrotto dopo {} secondi senza risposta", timeout.as_secs());
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn chrome_binary_candidates() -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // Edge ships with every Windows 10/11 install, so try it first. Per-user installs
        // of Chrome/Edge live under %LOCALAPPDATA%, system-wide ones under Program Files.
        let roots: Vec<String> = ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432", "LOCALAPPDATA"]
            .iter()
            .filter_map(|var| std::env::var(var).ok())
            .collect();
        for rel in [
            r"Microsoft\Edge\Application\msedge.exe",
            r"Google\Chrome\Application\chrome.exe",
            r"Chromium\Application\chrome.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
        ] {
            for root in &roots {
                candidates.push(format!(r"{root}\{rel}"));
            }
        }
        candidates.extend(
            [
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
                r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            ]
            .map(str::to_string),
        );
    }

    #[cfg(target_os = "macos")]
    candidates.extend(
        [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        ]
        .map(str::to_string),
    );

    #[cfg(target_os = "linux")]
    candidates.extend(
        [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
            "brave-browser",
        ]
        .map(str::to_string),
    );

    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.to_lowercase()))
        .filter(|candidate| {
            if candidate.contains('/') || candidate.contains('\\') {
                Path::new(candidate).is_file()
            } else {
                binary_on_path(candidate)
            }
        })
        .collect()
}

fn binary_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Builds a `file:///` URL that Chromium accepts on every platform. On Windows
/// `canonicalize` returns verbatim paths (`\\?\C:\...`) which must be stripped, and
/// drive paths need a third slash (`file:///C:/...`). Anything outside the unreserved
/// set (spaces, `#`, `%`, non-ASCII user names...) is percent-encoded.
pub(crate) fn path_to_file_url(path: &str) -> Result<String> {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    Ok(file_url_from_path_string(&absolute.to_string_lossy()))
}

fn file_url_from_path_string(raw: &str) -> String {
    let mut normalized = raw.replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        normalized = format!("//{rest}");
    } else if let Some(rest) = normalized.strip_prefix("//?/") {
        normalized = rest.to_string();
    }

    let mut encoded = String::with_capacity(normalized.len());
    for byte in normalized.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }

    if encoded.starts_with("//") {
        // UNC share: file://server/share/...
        format!("file:{encoded}")
    } else if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// `shot.png` -> `shot_<suffix>.png` next to it.
fn screenshot_variant(path: &Path, suffix: &str) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let ext = path.extension()?.to_str()?;
    Some(path.with_file_name(format!("{stem}_{suffix}.{ext}")))
}

/// Older versions drew the click marker straight onto the screenshot and kept the pristine
/// capture as `{stem}_clean.{ext}`. Bring such files back to the current layout: the original
/// is the untouched capture again, and the version with the marker becomes the
/// `{stem}_annotated.{ext}` variant (unless an annotated one already exists). Both are kept.
pub fn restore_legacy_original(path: &Path) {
    let (Some(clean), Some(annotated)) = (screenshot_variant(path, "clean"), screenshot_variant(path, "annotated")) else {
        return;
    };
    if !clean.is_file() {
        return;
    }
    if path.is_file() && !annotated.exists() {
        let modified = std::fs::read(path).ok() != std::fs::read(&clean).ok();
        if modified {
            let _ = std::fs::copy(path, &annotated);
        }
    }
    if std::fs::copy(&clean, path).is_ok() {
        let _ = std::fs::remove_file(&clean);
    }
}

/// Whether the screenshot carries annotations other than the automatic click marker (those are
/// drawn by the annotation editor, which writes the `_annotated` variant itself).
fn has_custom_annotations(shot: &Screenshot) -> bool {
    shot.annotations_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<serde_json::Value>>(json).ok())
        .is_some_and(|items| items.iter().any(|item| item.get("type").and_then(|t| t.as_str()) != Some("click")))
}

/// Gets screenshots ready for documents: the original capture is never modified; when click
/// highlighting is on, a copy with the click marker is written as `{stem}_annotated.{ext}`
/// (used by exports with "annotations" enabled). Existing annotated variants, e.g. from the
/// annotation editor, are kept as they are.
pub fn prepare_screenshots_for_export(screenshots: &[Screenshot], highlight_clicks: bool) {
    for shot in screenshots {
        let path = PathBuf::from(&shot.path);
        restore_legacy_original(&path);
        if !highlight_clicks || !path.is_file() || has_custom_annotations(shot) {
            continue;
        }
        let (Some(cx), Some(cy)) = (shot.click_x, shot.click_y) else {
            continue;
        };
        let Some(annotated) = screenshot_variant(&path, "annotated") else {
            continue;
        };
        if !annotated.exists() {
            let _ = crate::screenshots::render_click_highlight(&path, &annotated, cx, cy);
        }
    }
}

/// The "highlight clicks" setting (on by default).
pub fn highlight_clicks_enabled(db: &Database) -> bool {
    db.get_setting("highlight_clicks")
        .ok()
        .flatten()
        .map(|val| val != "false")
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::{file_url_from_path_string, render_styled_export_html, try_print_pdf, ExportOptions};
    use crate::storage::models::{Screenshot, Session, WorkflowStep};

    /// End-to-end check of the PDF path with a real Chromium. Run with
    /// `cargo test -- --ignored pdf_renders` once a Chromium/Edge binary is on PATH.
    #[test]
    #[ignore]
    fn pdf_renders_with_local_chromium() {
        let dir = std::env::temp_dir().join(format!("flowcapture pdf tëst {}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("screenshots")).unwrap();
        let shot_path = dir.join("screenshots").join("shot.png");
        image::RgbaImage::from_pixel(1600, 900, image::Rgba([40, 120, 200, 255]))
            .save(&shot_path)
            .unwrap();
        let screenshot = Screenshot {
            id: "s1".into(),
            session_id: "sess".into(),
            path: shot_path.to_string_lossy().to_string(),
            timestamp_ms: 1_700_000_005_000,
            trigger: Some("mouse_click".into()),
            selected: 1,
            click_x: Some(10),
            click_y: Some(10),
            annotations_json: None,
        };
        let mut md = String::from("# Guida di prova\n\nPanoramica.\n\n## Steps\n\n");
        for n in 1..=12 {
            md.push_str(&format!(
                "### Step {n}: Passo numero {n}\nDescrizione lunga del passo {n} con **grassetto** e testo àèìòù.\n\n![shot](screenshots/shot.png)\n\n"
            ));
        }
        let session = Session {
            id: "sess".into(),
            title: "Prova".into(),
            status: "ready".into(),
            started_at: "2023-11-14T22:13:20Z".into(),
            ended_at: None,
            video_path: None,
            duration: 95,
            documentation_md: Some(md),
            steps_json: None,
            compressed_events_json: None,
            audio_path: None,
            audio_transcript: None,
            full_video_path: None,
            preview_screenshot_path: None,
            audio_segments_json: None,
        };
        let steps: Vec<WorkflowStep> = Vec::new();
        let options = ExportOptions { page_size: "a4".into(), screenshots: true, cover: true, ..Default::default() };
        let html = render_styled_export_html(&session, &steps, &[screenshot], &options, "pdf");
        assert!(html.contains("@page { size: A4"));
        assert!(!html.contains("fonts.googleapis.com"));
        let html_path = dir.join("export.html");
        std::fs::write(&html_path, html).unwrap();
        let pdf_path = dir.join("export.pdf");
        try_print_pdf(&html_path, &pdf_path).expect("pdf render");
        let bytes = std::fs::read(&pdf_path).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        println!("PDF written to {} ({} bytes)", pdf_path.display(), bytes.len());
    }

    #[test]
    fn windows_verbatim_drive_path_becomes_triple_slash_url() {
        assert_eq!(
            file_url_from_path_string(r"\\?\C:\Users\Nicolò Rossi\AppData\export #1.html"),
            "file:///C:/Users/Nicol%C3%B2%20Rossi/AppData/export%20%231.html"
        );
    }

    #[test]
    fn plain_windows_drive_path() {
        assert_eq!(
            file_url_from_path_string(r"C:\data\a.html"),
            "file:///C:/data/a.html"
        );
    }

    #[test]
    fn windows_unc_paths() {
        assert_eq!(
            file_url_from_path_string(r"\\?\UNC\server\share\a b.html"),
            "file://server/share/a%20b.html"
        );
    }

    #[test]
    fn unix_path() {
        assert_eq!(
            file_url_from_path_string("/home/me/My Docs/a.html"),
            "file:///home/me/My%20Docs/a.html"
        );
    }

    use super::{export_name_suffix, file_name_slug, rename_session_exports};

    #[test]
    fn export_preparation_never_touches_the_original() {
        let dir = std::env::temp_dir().join(format!("flowcapture-prep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("shot.png");
        image::RgbaImage::from_pixel(120, 80, image::Rgba([10, 20, 30, 255])).save(&original).unwrap();
        let before = std::fs::read(&original).unwrap();
        let shot = Screenshot {
            id: "s".into(),
            session_id: "x".into(),
            path: original.to_string_lossy().to_string(),
            timestamp_ms: 0,
            trigger: None,
            selected: 1,
            click_x: Some(60),
            click_y: Some(40),
            annotations_json: None,
        };

        super::prepare_screenshots_for_export(std::slice::from_ref(&shot), true);
        assert_eq!(std::fs::read(&original).unwrap(), before, "original must stay untouched");
        let annotated = dir.join("shot_annotated.png");
        assert!(annotated.is_file(), "the highlighted copy is kept next to it");
        assert_ne!(std::fs::read(&annotated).unwrap(), before);

        // Highlighting off: nothing new is written.
        std::fs::remove_file(&annotated).unwrap();
        super::prepare_screenshots_for_export(std::slice::from_ref(&shot), false);
        assert!(!annotated.exists());

        // Legacy layout (marker baked into the original, pristine copy in `_clean`).
        let clean = dir.join("shot_clean.png");
        std::fs::write(&clean, &before).unwrap();
        image::RgbaImage::from_pixel(120, 80, image::Rgba([200, 0, 0, 255])).save(&original).unwrap();
        let baked = std::fs::read(&original).unwrap();
        super::prepare_screenshots_for_export(std::slice::from_ref(&shot), false);
        assert_eq!(std::fs::read(&original).unwrap(), before, "original restored");
        assert_eq!(std::fs::read(&annotated).unwrap(), baked, "modified version kept");
        assert!(!clean.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exports_follow_the_session_title() {
        let data = std::env::temp_dir().join(format!("flowcapture-exp-{}", uuid::Uuid::new_v4()));
        let db = crate::storage::Database::new(data.clone()).unwrap();
        let session = db.create_session(Some("Vecchio titolo".into())).unwrap();
        let exports = db.session_dir(&session.id).join("exports");
        let current = exports.join("Vecchio_titolo_2026-10-02_10-15-30.pdf");
        let legacy = exports.join("export_3f2c1d4e-1111-4222-8333-944455556666.html");
        for (idx, path) in [&current, &legacy].into_iter().enumerate() {
            std::fs::write(path, b"x").unwrap();
            db.create_export(&crate::storage::models::ExportRecord {
                id: format!("e{idx}"),
                session_id: session.id.clone(),
                format: "pdf".into(),
                path: path.to_string_lossy().to_string(),
                created_at: "2026-10-01T08:00:00Z".into(),
            })
            .unwrap();
        }

        assert_eq!(rename_session_exports(&db, &session.id, "Nuovo titolo").unwrap(), 2);
        let names: Vec<String> = db
            .list_exports(&session.id)
            .unwrap()
            .iter()
            .map(|e| std::path::Path::new(&e.path).file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"Nuovo_titolo_2026-10-02_10-15-30.pdf".to_string()), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("Nuovo_titolo_2026-10-01_") && n.ends_with(".html")), "{names:?}");
        assert!(!current.exists() && !legacy.exists());
        drop(db);
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn export_suffix_survives_renames() {
        assert_eq!(
            export_name_suffix("Vecchio_titolo_2026-10-02_10-15-30", "").as_deref(),
            Some("2026-10-02_10-15-30")
        );
        assert_eq!(
            export_name_suffix("Titolo_2026-10-02_10-15-30_2", "").as_deref(),
            Some("2026-10-02_10-15-30_2")
        );
        assert!(export_name_suffix("export_3f2c1d4e-1111-4222-8333-944455556666", "2026-10-02T08:00:00Z").is_some());
        assert_eq!(export_name_suffix("appunti miei", ""), None);
    }

    #[test]
    fn slugs_are_safe_file_names() {
        assert_eq!(file_name_slug("Come creare un ordine: guida/2"), "Come_creare_un_ordine_guida_2");
        assert_eq!(file_name_slug("Perché? Più file"), "Perché_Più_file");
        assert_eq!(file_name_slug("  "), "FlowCapture");
        assert_eq!(file_name_slug(r"..\evil"), "evil");
    }
}
