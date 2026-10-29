mod ai;
mod commands;
mod compression;
mod events;
mod export;
mod media;
mod platform;
mod recorder;
mod screenshots;
mod security;
mod state;
mod storage;
mod process_util;
mod thread_util;
pub mod logger;
pub mod updater;

use std::sync::Arc;

use tauri::Manager;

use crate::platform::create_platform_services;
use crate::state::AppState;
#[cfg(target_os = "macos")]
use crate::platform::prepare_recording_permissions;
use crate::storage::Database;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            // Installed builds can't write next to the executable (or to the working directory,
            // often System32): log to the platform log folder. Development keeps `<repo>/log`.
            if !cfg!(debug_assertions) {
                if let Ok(log_dir) = app.path().app_log_dir() {
                    logger::init(log_dir);
                }
            }
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|err| anyhow::anyhow!(err))?;
            let db = Arc::new(Database::new(app_data_dir)?);
            let platform = create_platform_services()?;
            let state = Arc::new(AppState::new(db, platform)?);
            state.cleanup_stale_recordings()?;
            let _ = crate::storage::mirror::apply_default_mirror_dir(&state.db);
            
            if crate::storage::mirror::mirror_write_root(&state.db).is_some() {
                let _ = crate::storage::mirror::sync_all(&state.db, &state.mirror);
            }
            app.manage(state);

            #[cfg(target_os = "macos")]
            {
                std::thread::spawn(|| {
                    let _ = prepare_recording_permissions();
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_recording_permissions,
            commands::prepare_recording_permissions,
            commands::request_accessibility_permission,
            commands::open_screen_recording_settings,
            commands::open_accessibility_settings,
            commands::reveal_executable_in_finder,
            commands::get_platform_name,
            commands::list_sessions,
            commands::get_session,
            commands::update_session_title,
            commands::get_session_mirror_dir,
            commands::set_session_mirror_dir,
            commands::sync_session_mirror,
            commands::open_session_mirror_dir,
            commands::get_session_mirror_status,
            commands::check_session_mirror_dir,
            commands::is_admin_mode,
            commands::get_browser_extension_status,
            commands::import_sessions_from_mirror,
            commands::update_session_documentation,
            commands::list_monitors,
            commands::start_recording,
            commands::stop_recording,
            commands::pause_recording,
            commands::resume_recording,
            commands::switch_recording_monitor,
            commands::capture_manual_screenshot,
            commands::list_events,
            commands::list_screenshots,
            commands::delete_screenshot,
            commands::list_providers,
            commands::update_provider,
            commands::generate_documentation,
            commands::estimate_generation_cost,
            commands::cancel_documentation_generation,
            commands::list_ai_jobs,
            commands::export_session,
            commands::list_exports,
            commands::get_setting,
            commands::set_setting,
            commands::get_compressed_timeline,
            commands::get_replay_steps,
            commands::save_session_audio,
            commands::transcribe_session_audio,
            commands::delete_session,
            commands::export_session_bundle,
            commands::import_session_bundle,
            commands::translate_documentation,
            commands::save_annotated_screenshot,
            commands::update_step_screenshot,
            commands::update_step_content,
            commands::update_documentation,
            commands::save_step_annotations,
            commands::find_duplicate_screenshots,
            commands::merge_duplicate_screenshots,
            commands::open_logs_folder,
            commands::open_browser_extension_folder,
            commands::get_browser_bridge_status,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Ready = event {
                #[cfg(all(debug_assertions, target_os = "macos"))]
                platform::refresh_dev_dock_icon();
            }
        });
}
