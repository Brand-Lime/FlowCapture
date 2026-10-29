//! Velopack integration (Windows): installation hooks and the update check that runs before the
//! app window opens, with the same window and rules as the C# apps published with the fork.
//!
//! The update feed is chosen when the app is built, through the `FLOWCAPTURE_UPDATE_SOURCE`
//! environment variable (set by `scripts/publish-velopack.ps1`): an `http(s)://` URL or a folder,
//! also a network share (`\\server\share\FlowCapture`). Builds without it, and copies not installed
//! by Velopack (e.g. `tauri dev`), skip the check.

/// Must run first in `main`: Velopack starts the exe with special arguments while installing,
/// updating and uninstalling, and expects it to exit right away.
pub fn run_startup_hooks() {
    #[cfg(windows)]
    windows_impl::run();
}

#[cfg(windows)]
mod windows_impl {
    use std::time::Duration;

    use velopack::sources::{FileSource, HttpSource, UpdateSource};
    use velopack::{PreLaunchOptions, VelopackApp};

    const UPDATE_SOURCE: Option<&str> = option_env!("FLOWCAPTURE_UPDATE_SOURCE");

    pub fn run() {
        let mut app = VelopackApp::build();
        if !cfg!(debug_assertions) {
            if let Some(source) = update_source() {
                app = app.with_pre_launch_update_check_boxed(source, options());
            }
        }
        app.run();
    }

    fn update_source() -> Option<Box<dyn UpdateSource>> {
        let source = UPDATE_SOURCE.map(str::trim).filter(|value| !value.is_empty())?;
        let lower = source.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            Some(Box::new(HttpSource::new(source)))
        } else {
            Some(Box::new(FileSource::new(source)))
        }
    }

    fn options() -> PreLaunchOptions<'static> {
        PreLaunchOptions {
            window_title: Some("FlowCapture - Controllo Aggiornamenti".to_string()),
            // Whether an update is mandatory is decided per release (`vpk pack --mandatory`).
            is_mandatory: false,
            // Server unreachable (offline, VPN off, timeout): start the installed version.
            allow_offline_launch: true,
            allow_launch_on_cancel: false,
            check_timeout: Duration::from_secs(10),
            // A download that takes longer counts as offline (starts the installed version).
            download_timeout: Some(Duration::from_secs(10 * 60)),
            show_ui: true,
            ..Default::default()
        }
    }
}
