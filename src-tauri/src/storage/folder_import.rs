//! Admin import ("Importa da cartella", visible with `--admin`): reads the sessions folder set in
//! Settings (usually the network share every user's FlowCapture copies its sessions to) and
//! imports the sessions recorded on other computers, as if they had been recorded here: the
//! `.flowcapture` archives written by the users, and the unzipped session folders (admin copies).
//!
//! Only what changed is imported: each session gets a fingerprint (its `manifest.json` without
//! the export time, plus the size of the archive or names and sizes of the folder's files). New
//! sessions are imported, sessions whose fingerprint differs from the last import are replaced,
//! the others are left alone.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use super::bundle::{import_session_bundle, read_bundle_manifest, SessionBundleManifest};
use super::mirror::{comparable_manifest, fnv1a, BUNDLE_EXTENSION, IMPORTED_SETTING_PREFIX};
use super::Database;

/// Per-session setting with the fingerprint of the folder at the last import.
const FINGERPRINT_SETTING_PREFIX: &str = "folder_import:";
/// How deep sessions are looked for (e.g. `<root>/<user>/<session>`).
const MAX_DEPTH: usize = 3;

#[derive(Debug, Clone, Serialize, Default)]
pub struct FolderImportSummary {
    /// Folder the sessions were read from.
    pub root: String,
    /// Sessions that were not on this computer.
    pub imported: usize,
    /// Sessions changed since the last import (replaced with the folder's version).
    pub updated: usize,
    pub unchanged: usize,
    /// Sessions being written right now or still recording: imported at the next run.
    pub skipped: usize,
    pub errors: Vec<String>,
}

pub fn import_changed_sessions(db: &Database, root: &Path) -> Result<FolderImportSummary> {
    if !root.is_dir() {
        anyhow::bail!(
            "La cartella sessioni {} non è raggiungibile. Controlla di essere collegato alla VPN.",
            root.display()
        );
    }
    let mut summary = FolderImportSummary { root: root.to_string_lossy().to_string(), ..Default::default() };
    for folder in find_session_folders(root)? {
        if let Err(err) = import_folder(db, &folder, &mut summary) {
            summary.errors.push(format!("{}: {err:#}", folder.display()));
        }
    }
    Ok(summary)
}

/// Imports one session: a `.flowcapture` archive or an unzipped session folder.
fn import_folder(db: &Database, folder: &Path, summary: &mut FolderImportSummary) -> Result<()> {
    let manifest_bytes = read_bundle_manifest(folder).context("manifest.json non leggibile")?;
    let manifest: SessionBundleManifest =
        serde_json::from_slice(&manifest_bytes).context("manifest.json non valido")?;
    let session_id = manifest.session.id.clone();

    let fingerprint = if folder.is_dir() {
        folder_fingerprint(&manifest_bytes, folder)?
    } else {
        Some(archive_fingerprint(&manifest_bytes, folder)?)
    };
    let Some(fingerprint) = fingerprint else {
        // The computer that recorded the session is copying it right now.
        summary.skipped += 1;
        return Ok(());
    };
    if manifest.session.status == crate::storage::models::SessionStatus::Recording.as_str() {
        summary.skipped += 1;
        return Ok(());
    }

    let fingerprint_key = format!("{FINGERPRINT_SETTING_PREFIX}{session_id}");
    let imported_key = format!("{IMPORTED_SETTING_PREFIX}{session_id}");
    let local = db.get_session(&session_id)?;
    if let Some(local) = &local {
        if local.status == crate::storage::models::SessionStatus::Recording.as_str() {
            summary.skipped += 1;
            return Ok(());
        }
        match db.get_setting(&fingerprint_key)? {
            Some(previous) if previous == fingerprint => {
                summary.unchanged += 1;
                return Ok(());
            }
            Some(_) => {}
            // Never imported from a folder: a session recorded on this computer (the folder is the
            // copy written by its own mirror), never replaced with itself.
            None if db.get_setting(&imported_key)?.is_none() => {
                summary.unchanged += 1;
                return Ok(());
            }
            None => {}
        }
    }

    // Set before the import: the mirror must never write this session into its folder, which
    // belongs to the computer that recorded it.
    db.set_setting(&imported_key, "1")?;
    if local.is_some() {
        // Replaced with the folder's version, keeping the same id. If the import fails the next
        // run imports it again (the fingerprint is stored only after a successful import).
        db.delete_session(&session_id)?;
    }
    let imported = import_session_bundle(db, folder)?;
    db.set_setting(&format!("{FINGERPRINT_SETTING_PREFIX}{}", imported.id), &fingerprint)?;
    if local.is_some() {
        summary.updated += 1;
    } else {
        summary.imported += 1;
    }
    Ok(())
}

/// Session folders (holding a `manifest.json`) and `.flowcapture` archives, up to `MAX_DEPTH`
/// levels under `root`.
fn find_session_folders(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if dir.join("manifest.json").is_file() {
            found.push(dir);
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let entries = std::fs::read_dir(&dir).with_context(|| format!("lettura di {}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case(BUNDLE_EXTENSION)) {
                // Archives are written as `.partial` and renamed: never seen half-written.
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Fingerprint of a session folder, or `None` while files are being written in it (`.partial`).
fn folder_fingerprint(manifest_bytes: &[u8], folder: &Path) -> Result<Option<String>> {
    let manifest = comparable_manifest(manifest_bytes).context("manifest.json non valido")?;
    let mut parts = vec![serde_json::to_string(&manifest)?];
    let mut files = Vec::new();
    for dir in ["screenshots", "media", "exports"] {
        let mut stack = vec![folder.join(dir)];
        while let Some(current) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_some_and(|ext| ext == "partial") {
                    return Ok(None);
                }
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let relative = path.strip_prefix(folder).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                files.push(format!("{relative}:{size}"));
            }
        }
    }
    files.sort();
    parts.extend(files);
    Ok(Some(format!("{:016x}", fnv1a(parts.join("\n").as_bytes()))))
}

/// Fingerprint of a `.flowcapture` archive: its manifest (without the export time) and size.
fn archive_fingerprint(manifest_bytes: &[u8], archive: &Path) -> Result<String> {
    let manifest = comparable_manifest(manifest_bytes).context("manifest.json non valido")?;
    let size = archive.metadata().map(|m| m.len()).unwrap_or(0);
    let text = format!("{}\narchive:{size}", serde_json::to_string(&manifest)?);
    Ok(format!("{:016x}", fnv1a(text.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::mirror::{set_test_admin_mode, sync_session, FULL_COPY_DIR_SETTING, MIRROR_DIR_SETTING};

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flowcapture-import-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn recorded_session(db: &Database, title: &str) -> String {
        let session = db.create_session(Some(title.into())).unwrap();
        db.finish_session(&session.id, None, 5).unwrap();
        let shots_dir = db.session_dir(&session.id).join("screenshots");
        std::fs::create_dir_all(&shots_dir).unwrap();
        let shot_path = shots_dir.join("a.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])).save(&shot_path).unwrap();
        db.insert_screenshot(&crate::storage::models::Screenshot {
            id: format!("shot-{}", session.id),
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
        session.id
    }

    #[test]
    fn imports_only_new_and_changed_sessions() {
        let user_data = temp_dir("user");
        let admin_data = temp_dir("admin");
        let share = temp_dir("share");
        let user_root = share.join("mario.rossi");

        set_test_admin_mode(false);
        // A user records two sessions: one .flowcapture archive each in their folder on the share.
        let user = Database::new(user_data.clone()).unwrap();
        user.set_setting(MIRROR_DIR_SETTING, &user_root.to_string_lossy()).unwrap();
        let first = recorded_session(&user, "Primo");
        let second = recorded_session(&user, "Secondo");
        let sync_user = |id: &str| {
            set_test_admin_mode(false);
            sync_session(&user, id).unwrap().unwrap()
        };
        assert_eq!(sync_user(&first), user_root.join("Primo.flowcapture"));
        sync_user(&second);

        // The admin's own session, in the admin's full copy (here on the share too, to check the
        // import never replaces a session recorded on this computer with its own copy).
        set_test_admin_mode(true);
        let admin = Database::new(admin_data.clone()).unwrap();
        admin.set_setting(MIRROR_DIR_SETTING, &share.to_string_lossy()).unwrap();
        admin.set_setting(FULL_COPY_DIR_SETTING, &share.join("admin").to_string_lossy()).unwrap();
        let own = recorded_session(&admin, "Admin");
        assert!(sync_session(&admin, &own).unwrap().unwrap().join("manifest.json").is_file());

        let summary = import_changed_sessions(&admin, &share).unwrap();
        assert_eq!((summary.imported, summary.updated, summary.unchanged), (2, 0, 1), "{summary:?}");
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        assert_eq!(admin.get_session(&first).unwrap().unwrap().title, "Primo");

        // Nothing changed: nothing is imported again (rewriting the folder with the same content
        // only changes the export time).
        sync_user(&first);
        let summary = import_changed_sessions(&admin, &share).unwrap();
        assert_eq!((summary.imported, summary.updated, summary.unchanged), (0, 0, 3), "{summary:?}");

        // The user edits the guide: only that session is replaced, with the same id.
        user.save_documentation(&first, "# Guida aggiornata\n", "[]", "[]").unwrap();
        sync_user(&first);
        let summary = import_changed_sessions(&admin, &share).unwrap();
        assert_eq!((summary.imported, summary.updated, summary.unchanged), (0, 1, 2), "{summary:?}");
        let updated = admin.get_session(&first).unwrap().unwrap();
        assert!(updated.documentation_md.unwrap().contains("aggiornata"));

        // Imported sessions are never written back by the admin's mirror.
        set_test_admin_mode(true);
        assert!(sync_session(&admin, &first).unwrap().is_none());
        set_test_admin_mode(false);

        drop(user);
        drop(admin);
        let _ = std::fs::remove_dir_all(user_data);
        let _ = std::fs::remove_dir_all(admin_data);
        let _ = std::fs::remove_dir_all(share);
    }
}
