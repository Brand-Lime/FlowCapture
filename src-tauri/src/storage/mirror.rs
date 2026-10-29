//! Session folder mirror: keeps an up-to-date copy of every session recorded on this computer in
//! a folder, updated by a background worker a moment after the last change.
//!
//! - Users (Settings → "Cartella sessioni", usually a network share): one `<title>.flowcapture`
//!   archive per session, the complete export that "Importa Sessione" / "Importa da cartella"
//!   rebuild on any computer.
//! - Admins (started with `--admin`): the sessions folder is only read ("Importa da cartella"),
//!   never written. Their sessions go to a second folder ("Copia completa"), one sub-folder per
//!   session with the content of an export unzipped: `manifest.json` (session, steps, events,
//!   screenshots, exports…), `documentation.md`, `screenshots/`, `media/` (HD video, timelapse,
//!   audio) and `exports/`.
//!
//! Renaming a session renames its archive / folder. Sessions imported from other computers are
//! never written back.
//!
//! The folder can be a network share: when it cannot be reached (offline, VPN off) the sessions
//! are remembered and copied again every few minutes, and at the next start.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::Serialize;

use super::bundle::{write_bundle_archive, write_session_bundle, BundleSink, PlanSink};
use super::Database;

/// Setting holding the mirror root folder; empty or missing = mirroring off.
pub const MIRROR_DIR_SETTING: &str = "session_mirror_dir";
/// Setting turning off the writing of the sessions into the folder ("false") while keeping the
/// folder set: e.g. an admin computer that only reads it ("Importa da cartella").
pub const MIRROR_WRITE_SETTING: &str = "session_mirror_write";
/// Admin only: folder receiving the complete, unzipped copy of every session.
pub const FULL_COPY_DIR_SETTING: &str = "session_full_copy_dir";
/// Per-session setting with the name of the folder the session was last written to (admin copy).
const FOLDER_SETTING_PREFIX: &str = "mirror_folder:";
/// Per-session settings of the user archive: its file name, and the fingerprint of the content
/// it was written from (rewritten only when the session changed: it contains the videos).
const ARCHIVE_SETTING_PREFIX: &str = "mirror_archive:";
const ARCHIVE_FINGERPRINT_PREFIX: &str = "mirror_archive_fp:";
/// Marker file identifying which session a folder belongs to.
pub(crate) const OWNER_MARKER: &str = ".flowcapture-session";
/// Per-session setting marking sessions imported from another computer's folder (admin import):
/// that folder belongs to the computer that recorded them, so they are not written back.
pub(crate) const IMPORTED_SETTING_PREFIX: &str = "mirror_imported:";
/// Extension of the session archive written for every session.
pub(crate) const BUNDLE_EXTENSION: &str = "flowcapture";
/// Sub-folders whose content is fully managed (stale files are removed from them).
const MANAGED_DIRS: [&str; 3] = ["screenshots", "media", "exports"];

const QUIET_PERIOD: Duration = Duration::from_millis(1500);
const MAX_DELAY: Duration = Duration::from_secs(10);
/// How often sessions that could not be copied are tried again.
const RETRY_INTERVAL: Duration = Duration::from_secs(120);

/// State of the copy to the sessions folder, shown to the user: copies running in the
/// background and sessions that could not be copied (retried automatically).
#[derive(Debug, Clone, Serialize, Default)]
pub struct MirrorStatus {
    /// Sessions whose last copy failed (folder unreachable, e.g. VPN off).
    pub pending_sessions: usize,
    pub last_error: Option<String>,
    /// A session is being copied right now.
    pub syncing: bool,
    /// Title of the session being copied.
    pub current_session: Option<String>,
    /// Sessions still waiting in the current round (the one being copied included).
    pub queued: usize,
    /// What the current copy is doing: "files" (copying the session files) or "archive"
    /// (writing the .flowcapture archive); `None` between two steps.
    pub phase: Option<&'static str>,
    /// Bytes written / to write in the current phase.
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// Videos still being encoded after a recording: they reach the folder when done.
    pub processing_videos: usize,
}

const PHASE_NONE: u8 = 0;
const PHASE_FILES: u8 = 1;
const PHASE_ARCHIVE: u8 = 2;

/// Progress of the copy running now, updated while bytes are written.
#[derive(Default)]
pub struct SyncProgress {
    phase: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
}

impl SyncProgress {
    fn start(&self, phase: u8, total: u64) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.phase.store(phase, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct WorkerState {
    /// Session id -> last error, for the sessions whose copy failed.
    failed: HashMap<String, String>,
    current: Option<String>,
    queued: usize,
}

/// Background worker: collects "session changed" notifications and syncs each session once
/// the changes have settled. Never blocks the app: everything runs on its own thread.
#[derive(Clone)]
pub struct MirrorWorker {
    tx: Sender<String>,
    state: Arc<Mutex<WorkerState>>,
    progress: Arc<SyncProgress>,
    /// Requests sent but not received by the worker yet (a change waiting to be copied).
    waiting: Arc<AtomicUsize>,
}

impl MirrorWorker {
    pub fn spawn(db: Arc<Database>) -> Self {
        let (tx, rx) = mpsc::channel::<String>();
        let state: Arc<Mutex<WorkerState>> = Arc::default();
        let worker_state = state.clone();
        let progress: Arc<SyncProgress> = Arc::default();
        let worker_progress = progress.clone();
        let waiting: Arc<AtomicUsize> = Arc::default();
        let worker_waiting = waiting.clone();
        let _ = std::thread::Builder::new()
            .name("session-mirror".to_string())
            .spawn(move || {
                let state = worker_state;
                let progress = worker_progress;
                let received = || {
                    let _ = worker_waiting.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
                };
                loop {
                    let has_failures = !state.lock().failed.is_empty();
                    let first = if has_failures {
                        match rx.recv_timeout(RETRY_INTERVAL) {
                            Ok(id) => {
                                received();
                                Some(id)
                            }
                            Err(RecvTimeoutError::Timeout) => None,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    } else {
                        match rx.recv() {
                            Ok(id) => {
                                received();
                                Some(id)
                            }
                            Err(_) => return,
                        }
                    };
                    let mut pending: HashSet<String> = match first {
                        Some(id) => HashSet::from([id]),
                        // Nothing changed for a while: try the failed copies again.
                        None => state.lock().failed.keys().cloned().collect(),
                    };
                    // Counted as queued while waiting for the changes to settle, so the copy shows as
                    // running (and closing the app is held back) from the first change.
                    state.lock().queued = pending.len();
                    let started = Instant::now();
                    loop {
                        let remaining = MAX_DELAY.saturating_sub(started.elapsed());
                        match rx.recv_timeout(QUIET_PERIOD.min(remaining)) {
                            Ok(id) => {
                                received();
                                pending.insert(id);
                                state.lock().queued = pending.len();
                            }
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                        if started.elapsed() >= MAX_DELAY {
                            break;
                        }
                    }
                    // Writing turned off (or no folder): nothing to copy, nothing left to retry.
                    if mirror_write_root(&db).is_none() {
                        let mut guard = state.lock();
                        guard.failed.clear();
                        guard.queued = 0;
                        continue;
                    }
                    state.lock().queued = pending.len();
                    for session_id in pending {
                        let title = db.get_session(&session_id).ok().flatten().map(|session| session.title);
                        state.lock().current = title;
                        let result = sync_session_with_progress(&db, &session_id, Some(&progress));
                        progress.phase.store(PHASE_NONE, Ordering::Relaxed);
                        let mut guard = state.lock();
                        guard.current = None;
                        guard.queued = guard.queued.saturating_sub(1);
                        match result {
                            Ok(_) => {
                                guard.failed.remove(&session_id);
                            }
                            Err(err) => {
                                crate::logger::write_entry(
                                    "WARN",
                                    "mirror",
                                    &format!("could not update the folder of session {session_id}: {err:#}"),
                                );
                                guard.failed.insert(session_id, format!("{err:#}"));
                            }
                        }
                    }
                }
            });
        Self { tx, state, progress, waiting }
    }

    pub fn request(&self, session_id: &str) {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        if self.tx.send(session_id.to_string()).is_err() {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub fn status(&self) -> MirrorStatus {
        let state = self.state.lock();
        let phase = match self.progress.phase.load(Ordering::Relaxed) {
            PHASE_FILES => Some("files"),
            PHASE_ARCHIVE => Some("archive"),
            _ => None,
        };
        MirrorStatus {
            pending_sessions: state.failed.len(),
            last_error: state.failed.values().next().cloned(),
            syncing: state.current.is_some() || state.queued > 0 || self.waiting.load(Ordering::SeqCst) > 0,
            current_session: state.current.clone(),
            queued: state.queued,
            phase,
            done_bytes: self.progress.done.load(Ordering::Relaxed),
            total_bytes: self.progress.total.load(Ordering::Relaxed),
            processing_videos: crate::media::active_video_jobs(),
        }
    }
}

/// Whether the sessions are written into the sessions folder (on unless turned off in Settings).
pub fn mirror_write_enabled(db: &Database) -> bool {
    db.get_setting(MIRROR_WRITE_SETTING).ok().flatten().as_deref() != Some("false")
}

/// Folder the sessions are written to: for an admin the "Copia completa" folder, for a user the
/// sessions folder. `None` when no folder is set or writing is turned off.
pub fn mirror_write_root(db: &Database) -> Option<PathBuf> {
    if admin_mode() {
        full_copy_root(db)
    } else if mirror_write_enabled(db) {
        mirror_root(db)
    } else {
        None
    }
}

/// Admin "Copia completa" folder (set also when not in admin mode, used only in admin mode).
pub fn full_copy_root(db: &Database) -> Option<PathBuf> {
    non_empty_setting(db, FULL_COPY_DIR_SETTING).map(PathBuf::from)
}

fn non_empty_setting(db: &Database, key: &str) -> Option<String> {
    db.get_setting(key)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(not(test))]
fn admin_mode() -> bool {
    crate::commands::is_admin_mode()
}

#[cfg(test)]
thread_local! {
    static TEST_ADMIN_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn admin_mode() -> bool {
    TEST_ADMIN_MODE.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn set_test_admin_mode(admin: bool) {
    TEST_ADMIN_MODE.with(|mode| mode.set(admin));
}

/// Sessions folder proposed on a new installation, chosen when the app is built through the
/// `FLOWCAPTURE_DEFAULT_SESSION_DIR` environment variable (set by `scripts/publish-velopack.ps1`).
/// `%NAME%` parts are replaced with the environment variables of the user, e.g.
/// `\\server\FlowCapture\%USERNAME%`.
const DEFAULT_MIRROR_DIR: Option<&str> = option_env!("FLOWCAPTURE_DEFAULT_SESSION_DIR");

/// Sets the default sessions folder when none was ever chosen on this computer. A folder removed
/// by the user (saved as empty) stays removed.
pub fn apply_default_mirror_dir(db: &Database) -> Result<()> {
    let Some(default) = DEFAULT_MIRROR_DIR.map(str::trim).filter(|dir| !dir.is_empty()) else {
        return Ok(());
    };
    if db.get_setting(MIRROR_DIR_SETTING)?.is_none() {
        db.set_setting(MIRROR_DIR_SETTING, &expand_env_vars(default, |name| std::env::var(name).ok()))?;
    }
    Ok(())
}

/// Folder read by the admin import ("Importa da cartella"). When the folder in use is the default
/// one with a per-user part (e.g. `\\server\FlowCapture\%USERNAME%`), the folder holding every
/// user's folder (`\\server\FlowCapture`); otherwise the folder set in Settings.
pub fn admin_import_root(db: &Database) -> Option<PathBuf> {
    let root = mirror_root(db)?;
    let template = DEFAULT_MIRROR_DIR.map(str::trim).unwrap_or_default();
    if template.is_empty() {
        return Some(root);
    }
    let expanded = expand_env_vars(template, |name| std::env::var(name).ok());
    if !same_path(&root, Path::new(&expanded)) {
        return Some(root);
    }
    Some(shared_parent(template).map(PathBuf::from).unwrap_or(root))
}

const SEPARATORS: [char; 2] = ['\\', '/'];

/// The part of a folder template before its first component with a `%VARIABLE%`; `None` when
/// that would be just a server name or a drive.
fn shared_parent(template: &str) -> Option<&str> {
    let variable = template.find('%')?;
    let separator = template[..variable].rfind(SEPARATORS)?;
    let parent = template[..separator].trim_end_matches(SEPARATORS);
    let components = parent.split(SEPARATORS).filter(|part| !part.is_empty()).count();
    let unc = parent.starts_with(SEPARATORS);
    let enough = if unc { components >= 2 } else { components >= 2 || (components == 1 && !parent.ends_with(':')) };
    enough.then_some(parent)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| path.to_string_lossy().trim_end_matches(SEPARATORS).to_lowercase();
    normalize(a) == normalize(b)
}

/// Replaces `%NAME%` with the value of the variable; unknown variables are left as they are.
fn expand_env_vars(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        result.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match lookup(&after[..end]) {
                Some(value) => {
                    result.push_str(&value);
                    rest = &after[end + 1..];
                }
                None => {
                    result.push('%');
                    rest = after;
                }
            },
            _ => {
                result.push('%');
                rest = after;
            }
        }
    }
    result.push_str(rest);
    result
}

/// Folder set in Settings, also when writing into it is turned off (the admin import reads it).
pub fn mirror_root(db: &Database) -> Option<PathBuf> {
    non_empty_setting(db, MIRROR_DIR_SETTING).map(PathBuf::from)
}

/// Writes (or refreshes) the copy of one session: the `.flowcapture` archive (user) or the
/// unzipped folder (admin). Returns its path, or `None` when there is nothing to write to, the
/// session no longer exists, is still being recorded or was imported from another computer.
#[cfg(test)]
pub fn sync_session(db: &Database, session_id: &str) -> Result<Option<PathBuf>> {
    sync_session_with_progress(db, session_id, None)
}

fn sync_session_with_progress(
    db: &Database,
    session_id: &str,
    progress: Option<&SyncProgress>,
) -> Result<Option<PathBuf>> {
    let Some(root) = mirror_write_root(db) else {
        return Ok(None);
    };
    let Some(session) = db.get_session(session_id)? else {
        return Ok(None);
    };
    if session.status == crate::storage::models::SessionStatus::Recording.as_str() {
        return Ok(None);
    }
    if db.get_setting(&format!("{IMPORTED_SETTING_PREFIX}{session_id}"))?.is_some() {
        return Ok(None);
    }
    std::fs::create_dir_all(&root)
        .with_context(|| format!("cannot create the sessions folder {}", root.display()))?;

    // The entries first: to know how much has to be copied (progress shown to the user) and
    // whether anything changed.
    let mut plan = PlanSink::default();
    let manifest = write_session_bundle(db, session_id, &mut plan)?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    if !admin_mode() {
        return write_archive(db, &root, session_id, &session.title, &plan, &manifest_bytes, progress).map(Some);
    }

    let folder = resolve_folder(db, &root, session_id, &session.title)?;
    std::fs::create_dir_all(&folder)?;
    std::fs::write(folder.join(OWNER_MARKER), session_id)?;
    if let Some(progress) = progress {
        let to_copy = plan
            .files
            .iter()
            .filter_map(|(entry, source)| {
                let target = entry_path(&folder, entry)?;
                (!is_up_to_date(source, &target)).then(|| file_size(source))
            })
            .sum();
        progress.start(PHASE_FILES, to_copy);
    }

    let mut sink = DirSink {
        root: folder.clone(),
        written: HashSet::new(),
        changed: false,
        copied: progress.map(|p| &p.done),
    };
    for (entry, source) in &plan.files {
        sink.add_file(entry, source)?;
    }
    for (entry, bytes) in &plan.bytes {
        sink.add_bytes(entry, bytes)?;
    }
    sink.add_manifest(&manifest_bytes)?;
    if let Some(markdown) = session.documentation_md.as_deref().filter(|md| !md.trim().is_empty()) {
        let screenshots = db.list_screenshots(session_id)?;
        // References become `screenshots/<file>`, which is where the images are in the folder.
        let normalized = crate::ai::documentation::normalize_screenshot_references(markdown, &screenshots);
        sink.add_bytes("documentation.md", normalized.as_bytes())?;
    }
    sink.prune_stale_files()?;
    Ok(Some(folder))
}

fn file_size(path: &Path) -> u64 {
    path.metadata().map(|m| m.len()).unwrap_or(0)
}

/// Keeps `<root>/<title>.flowcapture` up to date: rewritten only when the session changed (or the
/// file is missing), renamed with the session.
fn write_archive(
    db: &Database,
    root: &Path,
    session_id: &str,
    title: &str,
    plan: &PlanSink,
    manifest: &[u8],
    progress: Option<&SyncProgress>,
) -> Result<PathBuf> {
    let target = resolve_archive(db, root, session_id, title)?;
    let fingerprint_key = format!("{ARCHIVE_FINGERPRINT_PREFIX}{session_id}");
    let fingerprint = plan_fingerprint(plan, manifest);
    if target.is_file() && db.get_setting(&fingerprint_key)?.as_deref() == Some(fingerprint.as_str()) {
        return Ok(target);
    }
    if let Some(progress) = progress {
        let total = plan.files.iter().map(|(_, source)| file_size(source)).sum::<u64>() + manifest.len() as u64;
        progress.start(PHASE_ARCHIVE, total);
    }
    // Written next to the target and renamed, so a reader never sees a half-written archive.
    let partial = partial_path(&target);
    if let Err(err) = write_bundle_archive(&partial, plan, manifest, progress.map(|p| &p.done)) {
        let _ = std::fs::remove_file(&partial);
        return Err(err).context("writing the .flowcapture archive in the sessions folder");
    }
    replace_file(&partial, &target)?;
    db.set_setting(&fingerprint_key, &fingerprint)?;
    Ok(target)
}

/// What the archive is made of: the manifest (without its export time) and name, size and
/// modification time of every file.
fn plan_fingerprint(plan: &PlanSink, manifest: &[u8]) -> String {
    let mut parts = vec![comparable_manifest(manifest).map(|value| value.to_string()).unwrap_or_default()];
    for (entry, source) in &plan.files {
        let meta = source.metadata().ok();
        let modified = meta
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|time| time.as_millis())
            .unwrap_or(0);
        parts.push(format!("{entry}:{}:{modified}", meta.map(|meta| meta.len()).unwrap_or(0)));
    }
    for (entry, bytes) in &plan.bytes {
        parts.push(format!("{entry}:{:016x}", fnv1a(bytes)));
    }
    format!("{:016x}", fnv1a(parts.join("\n").as_bytes()))
}

/// FNV-1a 64: stable across builds (the fingerprints are stored in the database).
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Archive of the session under `root`, named after its title. Renames the existing archive when
/// the title changed; never takes over an archive of another session.
fn resolve_archive(db: &Database, root: &Path, session_id: &str, title: &str) -> Result<PathBuf> {
    let desired = folder_name_for_title(title);
    let setting_key = format!("{ARCHIVE_SETTING_PREFIX}{session_id}");
    let previous = non_empty_setting(db, &setting_key)
        .map(|name| root.join(name))
        .filter(|path| archive_owned_by(path, session_id));

    let archive_name = |name: &str| format!("{name}.{BUNDLE_EXTENSION}");
    let target = match previous {
        Some(current) if current.file_name().and_then(|n| n.to_str()) == Some(archive_name(&desired).as_str()) => {
            current
        }
        Some(current) => {
            let target = free_archive(root, &desired, session_id);
            if target == current {
                current
            } else {
                match std::fs::rename(&current, &target) {
                    Ok(()) => target,
                    // Open somewhere (e.g. being imported): keep the old name, the next change retries.
                    Err(err) => {
                        crate::logger::write_entry(
                            "WARN",
                            "mirror",
                            &format!("cannot rename {} to {}: {err}", current.display(), target.display()),
                        );
                        current
                    }
                }
            }
        }
        None => free_archive(root, &desired, session_id),
    };

    if let Some(name) = target.file_name().and_then(|n| n.to_str()) {
        db.set_setting(&setting_key, name)?;
    }
    Ok(target)
}

/// `desired.flowcapture`, or `desired (2).flowcapture`… skipping archives of other sessions.
fn free_archive(root: &Path, desired: &str, session_id: &str) -> PathBuf {
    let mut counter = 1;
    loop {
        let name = if counter == 1 { desired.to_string() } else { format!("{desired} ({counter})") };
        let candidate = root.join(format!("{name}.{BUNDLE_EXTENSION}"));
        if !candidate.exists() || archive_owned_by(&candidate, session_id) {
            return candidate;
        }
        counter += 1;
    }
}

fn archive_owned_by(path: &Path, session_id: &str) -> bool {
    super::bundle::read_bundle_manifest(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|manifest| manifest["session"]["id"].as_str() == Some(session_id))
}

/// `manifest.json` without the fields that change at every write (`exported_at`): two manifests
/// with the same value describe the same session content.
pub(crate) fn comparable_manifest(bytes: &[u8]) -> Option<serde_json::Value> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value.as_object_mut()?.remove("exported_at");
    Some(value)
}

/// Requests a sync of every session (after the folder was chosen or changed, and at startup to
/// copy what could not be copied while the folder was unreachable).
pub fn sync_all(db: &Database, worker: &MirrorWorker) -> Result<usize> {
    if mirror_write_root(db).is_none() {
        return Ok(0);
    }
    let sessions = db.list_sessions()?;
    for session in &sessions {
        worker.request(&session.id);
    }
    Ok(sessions.len())
}

/// Folder of the session under `root`, named after its title. Moves the existing folder when
/// the title changed; never takes over a folder that belongs to another session (or to the user).
fn resolve_folder(db: &Database, root: &Path, session_id: &str, title: &str) -> Result<PathBuf> {
    let desired = folder_name_for_title(title);
    let setting_key = format!("{FOLDER_SETTING_PREFIX}{session_id}");
    let previous = db
        .get_setting(&setting_key)?
        .map(|name| root.join(name))
        .filter(|path| owned_by(path, session_id));

    let folder = match previous {
        Some(current) if current.file_name().and_then(|n| n.to_str()) == Some(desired.as_str()) => current,
        Some(current) => {
            let target = free_folder(root, &desired, session_id);
            match std::fs::rename(&current, &target) {
                Ok(()) => target,
                // A file in it is open (e.g. a PDF in a viewer): keep the old name for now,
                // the next change retries.
                Err(err) => {
                    crate::logger::write_entry(
                        "WARN",
                        "mirror",
                        &format!("cannot rename {} to {}: {err}", current.display(), target.display()),
                    );
                    current
                }
            }
        }
        None => free_folder(root, &desired, session_id),
    };

    if let Some(name) = folder.file_name().and_then(|n| n.to_str()) {
        db.set_setting(&setting_key, name)?;
    }
    Ok(folder)
}

/// `desired`, or `desired (2)`, `desired (3)`… skipping folders owned by someone else.
fn free_folder(root: &Path, desired: &str, session_id: &str) -> PathBuf {
    let mut counter = 1;
    loop {
        let name = if counter == 1 { desired.to_string() } else { format!("{desired} ({counter})") };
        let candidate = root.join(&name);
        if !candidate.exists() || owned_by(&candidate, session_id) {
            return candidate;
        }
        counter += 1;
    }
}

fn owned_by(folder: &Path, session_id: &str) -> bool {
    std::fs::read_to_string(folder.join(OWNER_MARKER))
        .map(|owner| owner.trim() == session_id)
        .unwrap_or(false)
}

/// Session title -> folder name valid on Windows, macOS and Linux (keeps spaces and accents).
pub fn folder_name_for_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name: String = collapsed.chars().take(80).collect();
    // Windows drops trailing dots/spaces and refuses reserved device names.
    name = name.trim_end_matches(['.', ' ']).trim().to_string();
    let upper = name.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    let reserved = matches!(base, "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit());
    if name.is_empty() || reserved {
        name = if name.is_empty() { "Sessione".to_string() } else { format!("{name}_") };
    }
    name
}

/// Bundle destination writing plain files into a folder. Unchanged files are not copied again
/// (videos can be large), and files no longer part of the session are removed afterwards.
struct DirSink<'a> {
    root: PathBuf,
    written: HashSet<PathBuf>,
    /// Something was copied, rewritten or removed: the session content changed.
    changed: bool,
    /// Bytes copied so far (progress).
    copied: Option<&'a AtomicU64>,
}

/// Path of a `/`-separated bundle entry under `root`; `None` for unsafe entries.
fn entry_path(root: &Path, entry: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in entry.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':']) {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

/// Copies `source` to `target`, adding the bytes copied to `counter`.
fn copy_counting(source: &Path, target: &Path, counter: Option<&AtomicU64>) -> std::io::Result<()> {
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::File::create(target)?;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        if let Some(counter) = counter {
            counter.fetch_add(read as u64, Ordering::Relaxed);
        }
    }
    output.flush()
}

impl DirSink<'_> {
    fn target(&mut self, entry: &str) -> Option<PathBuf> {
        let path = entry_path(&self.root, entry)?;
        self.written.insert(path.clone()).then_some(path)
    }

    /// Writes `manifest.json` unless only its export time changed (keeps the folder untouched,
    /// and tells the admin import nothing changed).
    fn add_manifest(&mut self, bytes: &[u8]) -> Result<()> {
        let current = std::fs::read(self.root.join("manifest.json")).ok();
        let unchanged = current
            .as_deref()
            .and_then(comparable_manifest)
            .is_some_and(|current| Some(current) == comparable_manifest(bytes));
        if unchanged {
            self.written.insert(self.root.join("manifest.json"));
            return Ok(());
        }
        self.add_bytes("manifest.json", bytes)
    }

    fn prune_stale_files(&mut self) -> Result<()> {
        for dir in MANAGED_DIRS {
            let dir = self.root.join(dir);
            if !dir.is_dir() {
                continue;
            }
            let mut stack = vec![dir];
            while let Some(current) = stack.pop() {
                for entry in std::fs::read_dir(&current)? {
                    let path = entry?.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if !self.written.contains(&path) && std::fs::remove_file(&path).is_ok() {
                        self.changed = true;
                    }
                }
            }
        }
        Ok(())
    }
}

impl BundleSink for DirSink<'_> {
    fn add_file(&mut self, entry: &str, source: &Path) -> Result<()> {
        let Some(target) = self.target(entry) else {
            return Ok(());
        };
        if is_up_to_date(source, &target) {
            return Ok(());
        }
        self.changed = true;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Copy next to the target and rename, so a reader never sees a half-written file.
        let partial = partial_path(&target);
        copy_counting(source, &partial, self.copied)
            .with_context(|| format!("copying {} to the sessions folder", source.display()))?;
        replace_file(&partial, &target)
    }

    fn add_bytes(&mut self, entry: &str, bytes: &[u8]) -> Result<()> {
        let Some(target) = self.target(entry) else {
            return Ok(());
        };
        if std::fs::read(&target).is_ok_and(|current| current == bytes) {
            return Ok(());
        }
        self.changed = true;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let partial = partial_path(&target);
        std::fs::write(&partial, bytes)?;
        replace_file(&partial, &target)
    }
}

fn partial_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    target.with_file_name(name)
}

fn replace_file(partial: &Path, target: &Path) -> Result<()> {
    if std::fs::rename(partial, target).is_err() {
        // Windows cannot rename over an existing file in every case (e.g. read-only flag).
        let _ = std::fs::remove_file(target);
        if let Err(err) = std::fs::rename(partial, target) {
            let _ = std::fs::remove_file(partial);
            return Err(err).with_context(|| format!("writing {}", target.display()));
        }
    }
    Ok(())
}

fn is_up_to_date(source: &Path, target: &Path) -> bool {
    let (Ok(src), Ok(dst)) = (source.metadata(), target.metadata()) else {
        return false;
    };
    if src.len() != dst.len() {
        return false;
    }
    match (src.modified(), dst.modified()) {
        (Ok(src_time), Ok(dst_time)) => dst_time >= src_time,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flowcapture-mirror-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folder_names_are_valid_everywhere() {
        assert_eq!(folder_name_for_title("Ordine: cliente/fornitore?"), "Ordine cliente fornitore");
        assert_eq!(folder_name_for_title("  Perché più file...  "), "Perché più file");
        assert_eq!(folder_name_for_title("CON"), "CON_");
        assert_eq!(folder_name_for_title("com1.txt"), "com1.txt_");
        assert_eq!(folder_name_for_title("***"), "Sessione");
    }

    #[test]
    fn default_folder_expands_user_variables() {
        let lookup = |name: &str| (name == "USERNAME").then(|| "mario.rossi".to_string());
        assert_eq!(
            expand_env_vars(r"\\srv\FlowCapture\%USERNAME%", lookup),
            r"\\srv\FlowCapture\mario.rossi"
        );
        assert_eq!(expand_env_vars(r"C:\100%\%NOPE%\x", lookup), r"C:\100%\%NOPE%\x");
        assert_eq!(expand_env_vars("%%USERNAME%", lookup), "%mario.rossi");
    }

    #[test]
    fn admin_import_reads_every_user_folder() {
        assert_eq!(shared_parent(r"\\srvdati\GenAuto13485\%USERNAME%"), Some(r"\\srvdati\GenAuto13485"));
        assert_eq!(shared_parent(r"\\srvdati\Gen\utenti\u_%USERNAME%\sessioni"), Some(r"\\srvdati\Gen\utenti"));
        assert_eq!(shared_parent(r"D:\FlowCapture\%USERNAME%"), Some(r"D:\FlowCapture"));
        assert_eq!(shared_parent(r"\\srvdati\%USERNAME%"), None);
        assert_eq!(shared_parent(r"D:\%USERNAME%"), None);
        assert_eq!(shared_parent(r"\\srvdati\GenAuto13485"), None);
        assert!(same_path(Path::new(r"\\SRV\x\"), Path::new(r"\\srv\X")));
    }

    fn session_with_screenshot(db: &Database, title: &str, shot_id: &str) -> String {
        let session = db.create_session(Some(title.into())).unwrap();
        db.finish_session(&session.id, None, 5).unwrap();
        let shots_dir = db.session_dir(&session.id).join("screenshots");
        std::fs::create_dir_all(&shots_dir).unwrap();
        let shot_path = shots_dir.join(format!("{shot_id}.png"));
        image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 8, 7, 255])).save(&shot_path).unwrap();
        db.insert_screenshot(&crate::storage::models::Screenshot {
            id: shot_id.into(),
            session_id: session.id.clone(),
            path: shot_path.to_string_lossy().to_string(),
            timestamp_ms: 1,
            trigger: Some("click".into()),
            selected: 1,
            click_x: Some(3),
            click_y: Some(2),
            annotations_json: None,
        })
        .unwrap();
        session.id
    }

    fn modified(path: &Path) -> std::time::SystemTime {
        path.metadata().unwrap().modified().unwrap()
    }

    #[test]
    fn user_writes_only_the_archive() {
        set_test_admin_mode(false);
        let data = temp_dir("user-data");
        let other = temp_dir("user-other");
        let root = temp_dir("user-root");
        let db = Database::new(data.clone()).unwrap();
        db.set_setting(MIRROR_DIR_SETTING, &root.to_string_lossy()).unwrap();
        let session = session_with_screenshot(&db, "Primo titolo", "shot-u");

        let archive = sync_session(&db, &session).unwrap().unwrap();
        assert_eq!(archive, root.join("Primo titolo.flowcapture"));
        let names: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("Primo titolo.flowcapture")]);

        // Unchanged session: the archive is not rewritten.
        let written_at = modified(&archive);
        std::thread::sleep(Duration::from_millis(20));
        sync_session(&db, &session).unwrap();
        assert_eq!(modified(&archive), written_at);

        // Changed and renamed session: the archive is rewritten under the new name.
        db.save_documentation(&session, "# Guida nuova\n", "[]", "[]").unwrap();
        db.update_session_title(&session, "Secondo titolo").unwrap();
        let renamed = sync_session(&db, &session).unwrap().unwrap();
        assert_eq!(renamed, root.join("Secondo titolo.flowcapture"));
        assert!(!archive.exists());

        // Another session with the same title never takes the archive over.
        let twin = session_with_screenshot(&db, "Secondo titolo", "shot-t");
        assert_eq!(sync_session(&db, &twin).unwrap().unwrap(), root.join("Secondo titolo (2).flowcapture"));

        // With writing turned off nothing is written (and the folder stays set).
        db.set_setting(MIRROR_WRITE_SETTING, "false").unwrap();
        assert!(sync_session(&db, &session).unwrap().is_none());
        assert_eq!(mirror_root(&db), Some(root.clone()));

        // The archive rebuilds the session on another computer, same id.
        let target = Database::new(other.clone()).unwrap();
        let imported = crate::storage::bundle::import_session_bundle(&target, &renamed).unwrap();
        assert_eq!(imported.id, session);
        assert!(imported.documentation_md.unwrap().contains("nuova"));
        assert_eq!(target.list_screenshots(&imported.id).unwrap().len(), 1);

        drop(db);
        drop(target);
        let _ = std::fs::remove_dir_all(data);
        let _ = std::fs::remove_dir_all(other);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mirrors_a_session_and_follows_renames() {
        set_test_admin_mode(true);
        let data = temp_dir("data");
        let root = temp_dir("root");
        let share = temp_dir("admin-share");
        let db = Database::new(data.clone()).unwrap();
        // The admin only reads the sessions folder: the sessions go to the full copy folder.
        db.set_setting(MIRROR_DIR_SETTING, &share.to_string_lossy()).unwrap();
        db.set_setting(FULL_COPY_DIR_SETTING, &root.to_string_lossy()).unwrap();

        let session = db.create_session(Some("Primo titolo".into())).unwrap();
        db.finish_session(&session.id, None, 12).unwrap();
        let shots_dir = db.session_dir(&session.id).join("screenshots");
        std::fs::create_dir_all(&shots_dir).unwrap();
        let shot_path = shots_dir.join("a.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])).save(&shot_path).unwrap();
        db.insert_screenshot(&crate::storage::models::Screenshot {
            id: "shot-a".into(),
            session_id: session.id.clone(),
            path: shot_path.to_string_lossy().to_string(),
            timestamp_ms: 1,
            trigger: None,
            selected: 1,
            click_x: None,
            click_y: None,
            annotations_json: None,
        })
        .unwrap();
        db.save_documentation(&session.id, "# Guida\n\n### Step 1: Apri\n![a](screenshots/a.png)\n", "[]", "[]")
            .unwrap();

        let folder = sync_session(&db, &session.id).unwrap().unwrap();
        assert_eq!(folder, root.join("Primo titolo"));
        assert!(folder.join("manifest.json").is_file());
        assert!(folder.join("screenshots/a.png").is_file());
        assert!(std::fs::read_to_string(folder.join("documentation.md")).unwrap().contains("screenshots/a.png"));

        db.update_session_title(&session.id, "Secondo titolo").unwrap();
        let renamed = sync_session(&db, &session.id).unwrap().unwrap();
        assert_eq!(renamed, root.join("Secondo titolo"));
        assert!(!root.join("Primo titolo").exists());
        assert!(renamed.join("screenshots/a.png").is_file());

        // A user folder with the same name is never taken over.
        let other = db.create_session(Some("Secondo titolo".into())).unwrap();
        db.finish_session(&other.id, None, 1).unwrap();
        let second = sync_session(&db, &other.id).unwrap().unwrap();
        assert_eq!(second, root.join("Secondo titolo (2)"));

        // Without a full copy folder nothing is written.
        db.set_setting(FULL_COPY_DIR_SETTING, "").unwrap();
        assert!(sync_session(&db, &session.id).unwrap().is_none());
        db.set_setting(FULL_COPY_DIR_SETTING, &root.to_string_lossy()).unwrap();

        // Deleted screenshots disappear from the copy.
        db.delete_screenshot("shot-a").unwrap();
        sync_session(&db, &session.id).unwrap();
        assert!(!renamed.join("screenshots/a.png").exists());
        assert!(std::fs::read_dir(&share).unwrap().next().is_none(), "nothing written to the sessions folder");

        set_test_admin_mode(false);
        drop(db);
        let _ = std::fs::remove_dir_all(data);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(share);
    }

    #[test]
    fn mirrored_folder_imports_as_the_same_session() {
        set_test_admin_mode(true);
        let data = temp_dir("source");
        let other = temp_dir("other");
        let root = temp_dir("share");
        let db = Database::new(data.clone()).unwrap();
        db.set_setting(FULL_COPY_DIR_SETTING, &root.to_string_lossy()).unwrap();

        let session = db.create_session(Some("Sessione condivisa".into())).unwrap();
        db.finish_session(&session.id, None, 5).unwrap();
        let shots_dir = db.session_dir(&session.id).join("screenshots");
        std::fs::create_dir_all(&shots_dir).unwrap();
        let shot_path = shots_dir.join("b.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 8, 7, 255])).save(&shot_path).unwrap();
        db.insert_screenshot(&crate::storage::models::Screenshot {
            id: "shot-b".into(),
            session_id: session.id.clone(),
            path: shot_path.to_string_lossy().to_string(),
            timestamp_ms: 1,
            trigger: Some("click".into()),
            selected: 1,
            click_x: Some(3),
            click_y: Some(2),
            annotations_json: None,
        })
        .unwrap();
        db.save_documentation(&session.id, "# Guida\n\n### Step 1: Apri\n![b](screenshots/b.png)\n", "[]", "[]")
            .unwrap();
        let folder = sync_session(&db, &session.id).unwrap().unwrap();

        // Another installation (e.g. on the server) opens the folder.
        let target = Database::new(other.clone()).unwrap();
        let imported = crate::storage::bundle::import_session_bundle(&target, &folder).unwrap();
        assert_eq!(imported.id, session.id);
        assert_eq!(imported.title, "Sessione condivisa");
        assert!(imported.documentation_md.unwrap().contains("b.png"));
        let shots = target.list_screenshots(&imported.id).unwrap();
        assert_eq!(shots.len(), 1);
        assert_eq!(shots[0].id, "shot-b");
        assert_eq!(shots[0].click_x, Some(3));
        assert!(std::path::Path::new(&shots[0].path).is_file());
        assert!(std::path::Path::new(&shots[0].path).starts_with(&other));

        // Picking the manifest.json inside the folder works too (a second copy gets a new id).
        let again = crate::storage::bundle::import_session_bundle(&target, &folder.join("manifest.json")).unwrap();
        assert_ne!(again.id, session.id);

        set_test_admin_mode(false);
        drop(db);
        drop(target);
        let _ = std::fs::remove_dir_all(data);
        let _ = std::fs::remove_dir_all(other);
        let _ = std::fs::remove_dir_all(root);
    }
}
