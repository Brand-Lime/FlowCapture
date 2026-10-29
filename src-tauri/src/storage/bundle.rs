use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::storage::models::{AiJob, ExportRecord, Screenshot, Session, StoredEvent};
use crate::storage::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBundleManifest {
    pub version: u32,
    pub exported_at: String,
    pub app_version: String,
    pub session: Session,
    pub events: Vec<StoredEvent>,
    pub screenshots: Vec<BundleScreenshotMeta>,
    pub ai_jobs: Vec<AiJob>,
    pub exports: Vec<ExportRecord>,
    pub audio_file_name: Option<String>,
    pub video_file_name: Option<String>,
    /// Full-resolution screen recording (`video/recording_hd.mp4`). Added in bundle version 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_video_file_name: Option<String>,
    /// Pause intervals (epoch ms) needed to align events with the audio track.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pauses: Vec<(i64, i64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_segments: Option<Vec<crate::ai::transcription::AudioSegment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aligned_events: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleScreenshotMeta {
    pub id: String,
    pub timestamp_ms: i64,
    pub trigger: Option<String>,
    pub selected: i64,
    pub click_x: Option<i64>,
    pub click_y: Option<i64>,
    pub annotations_json: Option<String>,
    pub file_name: String,
}

/// Where the files of a session bundle go: a `.flowcapture` zip archive or a plain folder.
pub(crate) trait BundleSink {
    /// Copies `source` to `entry` (a `/`-separated relative path). Each entry is written once.
    fn add_file(&mut self, entry: &str, source: &Path) -> Result<()>;
    fn add_bytes(&mut self, entry: &str, bytes: &[u8]) -> Result<()>;
}

struct ZipSink<W: Write + Seek> {
    zip: ZipWriter<W>,
    written: HashSet<String>,
}

/// File that adds the bytes written to it to a counter (progress of a long copy).
struct CountingFile<'a> {
    file: File,
    counter: Option<&'a AtomicU64>,
}

impl Write for CountingFile<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(buf)?;
        if let Some(counter) = self.counter {
            counter.fetch_add(written as u64, Ordering::Relaxed);
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Seek for CountingFile<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

/// Bundle entries collected without writing anything (the mirror plans its copy with them).
#[derive(Default)]
pub(crate) struct PlanSink {
    pub files: Vec<(String, PathBuf)>,
    pub bytes: Vec<(String, Vec<u8>)>,
    seen: HashSet<String>,
}

impl BundleSink for PlanSink {
    fn add_file(&mut self, entry: &str, source: &Path) -> Result<()> {
        if self.seen.insert(entry.to_string()) {
            self.files.push((entry.to_string(), source.to_path_buf()));
        }
        Ok(())
    }

    fn add_bytes(&mut self, entry: &str, bytes: &[u8]) -> Result<()> {
        if self.seen.insert(entry.to_string()) {
            self.bytes.push((entry.to_string(), bytes.to_vec()));
        }
        Ok(())
    }
}

/// Writes a `.flowcapture` archive from planned entries plus `manifest.json` (same content as
/// `export_session_bundle`), adding the bytes written to `written_bytes`.
pub(crate) fn write_bundle_archive(
    target_path: &Path,
    plan: &PlanSink,
    manifest: &[u8],
    written_bytes: Option<&AtomicU64>,
) -> Result<()> {
    let file = File::create(target_path)
        .with_context(|| format!("Failed to create export bundle at {}", target_path.display()))?;
    let mut sink = ZipSink {
        zip: ZipWriter::new(CountingFile { file, counter: written_bytes }),
        written: HashSet::new(),
    };
    for (entry, source) in &plan.files {
        sink.add_file(entry, source)?;
    }
    for (entry, bytes) in &plan.bytes {
        sink.add_bytes(entry, bytes)?;
    }
    sink.add_bytes("manifest.json", manifest)?;
    sink.zip.finish()?;
    Ok(())
}

impl<W: Write + Seek> BundleSink for ZipSink<W> {
    fn add_file(&mut self, entry: &str, source: &Path) -> Result<()> {
        add_file(&mut self.zip, &mut self.written, entry, source)
    }

    fn add_bytes(&mut self, entry: &str, bytes: &[u8]) -> Result<()> {
        if !self.written.insert(entry.to_string()) {
            return Ok(());
        }
        self.zip.start_file(
            entry,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )?;
        self.zip.write_all(bytes)?;
        Ok(())
    }
}

pub fn export_session_bundle(
    db: &Database,
    session_id: &str,
    target_path: &Path,
) -> Result<PathBuf> {
    db.get_session(session_id)?.context("Session not found for export")?;
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(target_path)
        .with_context(|| format!("Failed to create export bundle at {}", target_path.display()))?;
    let mut sink = ZipSink { zip: ZipWriter::new(file), written: HashSet::new() };

    let manifest = write_session_bundle(db, session_id, &mut sink)?;
    sink.add_bytes("manifest.json", &serde_json::to_vec_pretty(&manifest)?)?;
    sink.zip.finish()?;
    Ok(target_path.to_path_buf())
}

/// Writes screenshots, media and exports of a session into `sink` and returns the manifest
/// describing them (the caller stores it as `manifest.json`).
pub(crate) fn write_session_bundle(
    db: &Database,
    session_id: &str,
    sink: &mut dyn BundleSink,
) -> Result<SessionBundleManifest> {
    let session = db
        .get_session(session_id)?
        .context("Session not found for export")?;
    let events = db.list_events(session_id)?;
    let screenshots = db.list_screenshots(session_id)?;
    let ai_jobs = db.list_ai_jobs(session_id)?;
    let exports = db.list_exports(session_id)?;
    let session_dir = db.session_dir(session_id);

    // 1. Screenshots, with their derived variants (clean copy, annotated / click-highlighted copy)
    let mut manifest_screenshots = Vec::new();
    for s in &screenshots {
        let original_path = Path::new(&s.path);
        let file_name = original_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{}.png", s.id));

        let source = if original_path.is_file() {
            Some(original_path.to_path_buf())
        } else {
            Some(session_dir.join("screenshots").join(&file_name)).filter(|p| p.is_file())
        };

        if let Some(source) = source {
            for candidate in std::iter::once(source.clone()).chain(screenshot_variants(&source)) {
                if let Some(name) = candidate.file_name().and_then(|n| n.to_str()) {
                    sink.add_file(&format!("screenshots/{name}"), &candidate)?;
                }
            }
        }

        manifest_screenshots.push(BundleScreenshotMeta {
            id: s.id.clone(),
            timestamp_ms: s.timestamp_ms,
            trigger: s.trigger.clone(),
            selected: s.selected,
            click_x: s.click_x,
            click_y: s.click_y,
            annotations_json: s.annotations_json.clone(),
            file_name,
        });
    }

    // 2. Media: microphone audio, slideshow video and full HD video
    let pack_media = |sink: &mut dyn BundleSink, stored: Option<&String>| -> Result<Option<String>> {
        let Some(stored) = stored else {
            return Ok(None);
        };
        let path = Path::new(stored);
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            return Ok(None);
        };
        let source = [path.to_path_buf(), session_dir.join(&name), session_dir.join("video").join(&name), session_dir.join("audio").join(&name)]
            .into_iter()
            .find(|p| p.is_file());
        match source {
            Some(source) => {
                sink.add_file(&format!("media/{name}"), &source)?;
                Ok(Some(name))
            }
            None => Ok(None),
        }
    };
    let audio_file_name = pack_media(sink, session.audio_path.as_ref())?;
    let video_file_name = pack_media(sink, session.video_path.as_ref())?;
    let full_video_file_name = pack_media(sink, session.full_video_path.as_ref())?;

    // 3. Exported documents, including the images folder the HTML / Markdown exports reference
    let exports_dir = session_dir.join("exports");
    if exports_dir.is_dir() {
        for path in walk_files(&exports_dir)? {
            if let Ok(relative) = path.strip_prefix(&exports_dir) {
                let entry = relative.to_string_lossy().replace('\\', "/");
                sink.add_file(&format!("exports/{entry}"), &path)?;
            }
        }
    }
    for export in &exports {
        let exp_path = Path::new(&export.path);
        if let Some(fname) = exp_path.file_name().and_then(|f| f.to_str()) {
            if exp_path.is_file() {
                sink.add_file(&format!("exports/{fname}"), exp_path)?;
            }
        }
    }

    // 5. Compute aligned events with nearby_audio if audio_segments are present
    let pauses = db.get_session_pauses(session_id).unwrap_or_default();
    let (audio_segments, aligned_events) = if let Some(segs_json) = &session.audio_segments_json {
        if let Ok(segs) = serde_json::from_str::<Vec<crate::ai::transcription::AudioSegment>>(segs_json) {
            if !segs.is_empty() {
                const PRE_MS: i64 = 3000;
                const POST_MS: i64 = 2000;

                let first_event_ts = events.first().map(|e| e.timestamp_ms).unwrap_or(0);
                let parsed_start = chrono::DateTime::parse_from_rfc3339(&session.started_at)
                    .map(|dt| dt.timestamp_millis())
                    .ok();
                let origin = crate::ai::pipeline::audio_timeline_origin(first_event_ts, parsed_start);

                let aligned = events
                    .iter()
                    .map(|ev| {
                        let event_offset_ms =
                            crate::ai::pipeline::event_audio_offset_ms(ev.timestamp_ms, origin, &pauses);
                        let w_start = event_offset_ms.saturating_sub(PRE_MS);
                        let w_end = event_offset_ms + POST_MS;
                        let nearby: Vec<&str> = segs
                            .iter()
                            .filter(|s| s.start_ms <= w_end && s.end_ms >= w_start)
                            .map(|s| s.text.as_str())
                            .collect();
                        let payload_val: serde_json::Value =
                            serde_json::from_str(&ev.payload).unwrap_or(serde_json::json!({}));
                        serde_json::json!({
                            "id": ev.id,
                            "event_type": ev.event_type,
                            "app_name": ev.app_name,
                            "payload": payload_val,
                            "timestamp_ms": ev.timestamp_ms,
                            "nearby_audio": nearby,
                        })
                    })
                    .collect::<Vec<_>>();
                (Some(segs), Some(aligned))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let manifest = SessionBundleManifest {
        version: 2,
        exported_at: Utc::now().to_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        session,
        events,
        screenshots: manifest_screenshots,
        ai_jobs,
        exports,
        audio_file_name,
        video_file_name,
        full_video_file_name,
        pauses,
        audio_segments,
        aligned_events,
    };

    Ok(manifest)
}

/// Where an imported session comes from: a `.flowcapture` archive, or a session folder written by
/// the sessions folder mirror (same entries, unzipped).
enum BundleSource {
    Zip(ZipArchive<File>),
    Dir(PathBuf),
}

impl BundleSource {
    fn open(path: &Path) -> Result<Self> {
        // The folder itself, or its manifest.json picked in a file dialog.
        let folder = if path.is_dir() {
            Some(path.to_path_buf())
        } else if path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("manifest.json")) {
            path.parent().map(Path::to_path_buf)
        } else {
            None
        };
        if let Some(folder) = folder {
            if !folder.join("manifest.json").is_file() {
                anyhow::bail!(
                    "La cartella {} non contiene manifest.json: non è una cartella di sessione FlowCapture",
                    folder.display()
                );
            }
            return Ok(Self::Dir(folder));
        }
        let file = File::open(path)
            .with_context(|| format!("Failed to open import bundle at {}", path.display()))?;
        let zip = ZipArchive::new(file).with_context(|| "Failed to read zip archive")?;
        Ok(Self::Zip(zip))
    }

    fn read_manifest(&mut self) -> Result<String> {
        let mut manifest = String::new();
        match self {
            Self::Zip(zip) => {
                zip.by_name("manifest.json")
                    .context("Missing manifest.json in archive: invalid or unsupported FlowCapture bundle")?
                    .read_to_string(&mut manifest)?;
            }
            Self::Dir(root) => {
                manifest = std::fs::read_to_string(root.join("manifest.json"))?;
            }
        }
        Ok(manifest)
    }

    /// Copies every entry to the path `place` returns for it (`None` = skip the entry).
    fn extract(&mut self, place: impl Fn(&str) -> Option<PathBuf>) -> Result<()> {
        let copy_to = |entry_name: &str, input: &mut dyn Read| -> Result<()> {
            let Some(out_path) = place(entry_name) else {
                if !entry_name.eq_ignore_ascii_case("manifest.json") {
                    crate::logger::info("bundle", &format!("Skipping unsafe or unknown archive entry: {entry_name}"));
                }
                return Ok(());
            };
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = File::create(&out_path)?;
            std::io::copy(input, &mut out_file)?;
            Ok(())
        };

        match self {
            Self::Zip(zip) => {
                for i in 0..zip.len() {
                    let mut entry = zip.by_index(i)?;
                    if entry.is_dir() {
                        continue;
                    }
                    let entry_name = entry.name().to_string();
                    copy_to(&entry_name, &mut entry)?;
                }
            }
            Self::Dir(root) => {
                // Only the bundle folders: the session folder also holds documentation.md and
                // the owner marker, which are not part of the bundle.
                for dir in ["screenshots", "media", "exports"] {
                    let dir = root.join(dir);
                    if !dir.is_dir() {
                        continue;
                    }
                    for path in walk_files(&dir)? {
                        // Half-written copies of the mirror are not session files.
                        if path.extension().is_some_and(|ext| ext == "partial") {
                            continue;
                        }
                        let Ok(relative) = path.strip_prefix(root.as_path()) else {
                            continue;
                        };
                        let entry_name = relative
                            .components()
                            .map(|part| part.as_os_str().to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("/");
                        let mut input = File::open(&path)
                            .with_context(|| format!("Cannot read {}", path.display()))?;
                        copy_to(&entry_name, &mut input)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// `manifest.json` of a `.flowcapture` archive or session folder, without importing anything.
pub(crate) fn read_bundle_manifest(path: &Path) -> Result<Vec<u8>> {
    Ok(BundleSource::open(path)?.read_manifest()?.into_bytes())
}

/// Imports a `.flowcapture` archive or a session folder of the sessions folder mirror.
pub fn import_session_bundle(
    db: &Database,
    archive_path: &Path,
) -> Result<Session> {
    let mut source = BundleSource::open(archive_path)?;

    // 1. Read manifest.json
    let manifest_str = source.read_manifest()?;

    let manifest: SessionBundleManifest = serde_json::from_str(&manifest_str)
        .context("Failed to parse bundle manifest.json")?;

    // 2. Check collision with existing sessions. The id becomes a directory name, so anything
    //    that is not a plain UUID (e.g. "../../x") is replaced with a fresh one.
    let id_is_safe = Uuid::parse_str(&manifest.session.id).is_ok();
    let (target_session_id, is_remapped) = if !id_is_safe {
        (Uuid::new_v4().to_string(), true)
    } else {
        match db.get_session(&manifest.session.id)? {
            Some(_) => (Uuid::new_v4().to_string(), true),
            None => (manifest.session.id.clone(), false),
        }
    };

    let target_session_dir = db.session_dir(&target_session_id);
    let target_screenshots_dir = target_session_dir.join("screenshots");
    let target_exports_dir = target_session_dir.join("exports");
    std::fs::create_dir_all(&target_screenshots_dir)?;
    std::fs::create_dir_all(&target_exports_dir)?;

    // 3. Extract all files from zip into target session directory.
    //    Entry names come from an untrusted archive: only plain file names directly under the
    //    known folders are accepted, so "screenshots/../../evil.exe" cannot escape the session dir.
    source.extract(|entry_name| {
        if let Some(rest) = entry_name.strip_prefix("screenshots/") {
            safe_file_name(rest).map(|name| target_screenshots_dir.join(name))
        } else if let Some(rest) = entry_name.strip_prefix("media/") {
            safe_file_name(rest).map(|name| target_session_dir.join(name))
        } else if let Some(rest) = entry_name.strip_prefix("exports/") {
            safe_relative_path(rest).map(|relative| target_exports_dir.join(relative))
        } else {
            None
        }
    })?;

    // 4. Map screenshot IDs and local paths
    let mut screenshot_id_map: HashMap<String, String> = HashMap::new();
    let mut restored_screenshots = Vec::new();

    for meta in &manifest.screenshots {
        let Some(file_name) = safe_file_name(&meta.file_name) else {
            continue;
        };
        let new_id = if is_remapped {
            let gen = Uuid::new_v4().to_string();
            screenshot_id_map.insert(meta.id.clone(), gen.clone());
            gen
        } else {
            meta.id.clone()
        };

        let local_path = target_screenshots_dir
            .join(file_name)
            .to_string_lossy()
            .to_string();

        restored_screenshots.push(Screenshot {
            id: new_id,
            session_id: target_session_id.clone(),
            path: local_path,
            timestamp_ms: meta.timestamp_ms,
            trigger: meta.trigger.clone(),
            selected: meta.selected,
            click_x: meta.click_x,
            click_y: meta.click_y,
            annotations_json: meta.annotations_json.clone(),
        });
    }

    // 5. Prepare restored session record
    let mut session = manifest.session;
    let old_session_id = session.id.clone();
    session.id = target_session_id.clone();

    // Fix audio & video path to point to local extracted files
    session.audio_path = manifest
        .audio_file_name
        .as_deref()
        .and_then(safe_file_name)
        .map(|name| target_session_dir.join(name))
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string());

    session.video_path = manifest
        .video_file_name
        .as_deref()
        .and_then(safe_file_name)
        .map(|name| target_session_dir.join(name))
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string());

    // Never keep a path that points outside this session.
    let full_video_name = manifest.full_video_file_name.clone().or_else(|| {
        session
            .full_video_path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .and_then(|name| name.to_str())
            .map(str::to_string)
    });
    session.full_video_path = full_video_name
        .as_deref()
        .and_then(safe_file_name)
        .map(|name| target_session_dir.join(name))
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string());

    if session.audio_segments_json.is_none() {
        if let Some(segs) = &manifest.audio_segments {
            if let Ok(json_str) = serde_json::to_string(segs) {
                session.audio_segments_json = Some(json_str);
            }
        }
    }

    // If remapped, update IDs in documentation_md, steps_json, compressed_events_json
    if is_remapped {
        if let Some(ref mut steps_str) = session.steps_json {
            for (old_id, new_id) in &screenshot_id_map {
                *steps_str = steps_str.replace(old_id, new_id);
            }
        }
    }

    // Normalize markdown image references for local platform
    if let Some(ref mut md) = session.documentation_md {
        if is_remapped {
            for (old_id, new_id) in &screenshot_id_map {
                *md = md.replace(old_id, new_id);
            }
            *md = md.replace(&old_session_id, &target_session_id);
        }
        *md = crate::ai::documentation::normalize_screenshot_references(md, &restored_screenshots);
    }

    // 6. Remap events
    let mut restored_events = manifest.events;
    for event in &mut restored_events {
        if is_remapped {
            event.id = Uuid::new_v4().to_string();
        }
        event.session_id = target_session_id.clone();
    }

    // 7. Remap AI jobs
    let mut restored_ai_jobs = manifest.ai_jobs;
    for job in &mut restored_ai_jobs {
        if is_remapped {
            job.id = Uuid::new_v4().to_string();
        }
        job.session_id = target_session_id.clone();
    }

    // 8. Remap exports
    let mut restored_exports = manifest.exports;
    for export in &mut restored_exports {
        if is_remapped {
            export.id = Uuid::new_v4().to_string();
        }
        export.session_id = target_session_id.clone();
        let exp_filename = Path::new(&export.path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        export.path = target_exports_dir
            .join(exp_filename)
            .to_string_lossy()
            .to_string();
    }

    // 9. Insert into database
    db.insert_imported_session(
        &session,
        &restored_events,
        &restored_screenshots,
        &restored_ai_jobs,
        &restored_exports,
    )?;
    if !manifest.pauses.is_empty() {
        db.set_session_pauses(&target_session_id, &serde_json::to_string(&manifest.pauses)?)?;
    }

    // 10. Return imported session
    db.get_session(&target_session_id)?
        .context("Failed to reload newly imported session")
}

/// Streams a file into the archive. Already-compressed media (images, video, audio) is stored
/// as-is: deflating it again costs time and saves almost nothing. Each entry is written once.
fn add_file<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    written: &mut HashSet<String>,
    entry_name: &str,
    source: &Path,
) -> Result<()> {
    if !written.insert(entry_name.to_string()) {
        return Ok(());
    }
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let method = if matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "mp4" | "webm" | "m4a" | "mp3" | "ogg" | "zip" | "pdf"
    ) {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .large_file(source.metadata().map(|m| m.len() >= u32::MAX as u64).unwrap_or(false));
    let mut input = File::open(source)
        .with_context(|| format!("Cannot read {} for the bundle", source.display()))?;
    zip.start_file(entry_name, options)?;
    std::io::copy(&mut input, zip)?;
    Ok(())
}

/// `shot.png` -> existing `shot_clean.png` / `shot_annotated.png` next to it.
fn screenshot_variants(path: &Path) -> Vec<PathBuf> {
    let (Some(stem), Some(ext), Some(parent)) = (
        path.file_stem().and_then(|s| s.to_str()),
        path.extension().and_then(|e| e.to_str()),
        path.parent(),
    ) else {
        return Vec::new();
    };
    ["clean", "annotated"]
        .iter()
        .map(|suffix| parent.join(format!("{stem}_{suffix}.{ext}")))
        .filter(|p| p.is_file())
        .collect()
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Like `safe_file_name`, but allows sub-folders ("screenshots/shot.png"): every component
/// must be a plain name.
fn safe_relative_path(path: &str) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.split('/') {
        result.push(safe_file_name(component)?);
    }
    Some(result).filter(|p| p.components().count() > 0)
}

/// Returns `name` only if it is a single, plain file name (no directories, no `..`,
/// no drive prefixes or absolute paths). Used to sanitize names read from imported bundles.
fn safe_file_name(name: &str) -> Option<&str> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':', '\0'])
    {
        return None;
    }
    let path = Path::new(name);
    if path.file_name().and_then(|f| f.to_str()) != Some(name) {
        return None;
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::{export_session_bundle, import_session_bundle, safe_file_name, safe_relative_path};
    use crate::storage::models::Screenshot;
    use crate::storage::Database;

    #[test]
    fn bundle_round_trip_keeps_all_session_files() {
        let root = std::env::temp_dir().join(format!("flowcapture-bundle-{}", uuid::Uuid::new_v4()));
        let db = Database::new(root.join("data")).unwrap();
        let session = db.create_session(Some("Test".into())).unwrap();
        let dir = db.session_dir(&session.id);

        let write = |rel: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, rel.as_bytes()).unwrap();
            path
        };
        let shot = write("screenshots/s1.png");
        write("screenshots/s1_annotated.png");
        write("screenshots/s1_clean.png");
        let audio = write("audio/mic.webm");
        let video = write("video/recording.mp4");
        let hd = write("video/recording_hd.mp4");
        write("exports/guide.html");
        write("exports/screenshots/step_1.png");

        db.insert_screenshot(&Screenshot {
            id: "shot-1".into(),
            session_id: session.id.clone(),
            path: shot.to_string_lossy().into(),
            timestamp_ms: 1,
            trigger: None,
            selected: 1,
            click_x: None,
            click_y: None,
            annotations_json: None,
        })
        .unwrap();
        db.update_session_audio(&session.id, &audio.to_string_lossy()).unwrap();
        db.update_session_video_path(&session.id, &video.to_string_lossy()).unwrap();
        db.update_session_full_video_path(&session.id, &hd.to_string_lossy()).unwrap();

        let archive = root.join("bundle.zip");
        export_session_bundle(&db, &session.id, &archive).unwrap();

        // Same database: the id collides, so the import gets a new session.
        let imported = import_session_bundle(&db, &archive).unwrap();
        assert_ne!(imported.id, session.id);
        let new_dir = db.session_dir(&imported.id);
        for rel in [
            "screenshots/s1.png",
            "screenshots/s1_annotated.png",
            "screenshots/s1_clean.png",
            "exports/guide.html",
            "exports/screenshots/step_1.png",
        ] {
            assert!(new_dir.join(rel).is_file(), "missing {rel}");
        }
        assert!(imported.audio_path.as_deref().is_some_and(|p| std::path::Path::new(p).is_file()));
        assert!(imported.video_path.as_deref().is_some_and(|p| std::path::Path::new(p).is_file()));
        assert!(imported.full_video_path.as_deref().is_some_and(|p| std::path::Path::new(p).is_file()));

        drop(db);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_nested_export_paths_only_when_safe() {
        assert!(safe_relative_path("screenshots/a.png").is_some());
        assert!(safe_relative_path("guide.html").is_some());
        assert!(safe_relative_path("../a.png").is_none());
        assert!(safe_relative_path("screenshots/../../a.png").is_none());
        assert!(safe_relative_path("screenshots//a.png").is_none());
    }

    #[test]
    fn accepts_plain_file_names() {
        assert_eq!(safe_file_name("shot_001.png"), Some("shot_001.png"));
        assert_eq!(safe_file_name("audio.webm"), Some("audio.webm"));
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        for bad in [
            "",
            ".",
            "..",
            "../evil.exe",
            r"..\evil.exe",
            "sub/dir.png",
            r"C:\Windows\evil.dll",
            "C:evil.dll",
            "/etc/passwd",
            "file.txt:stream",
        ] {
            assert_eq!(safe_file_name(bad), None, "should reject {bad:?}");
        }
    }
}
