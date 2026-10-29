// Pre-launch update check: Rust port of VelopackPreLaunchOptions and VelopackApp.PerformPreLaunchUpdateCheck in lib-csharp.
// The two implementations must behave the same (statuses, mandatory / offline / cancel rules, window).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    download,
    locator::VelopackLocatorConfig,
    manager::{UpdateCheck, UpdateManager},
    sources::UpdateSource,
    Error,
};

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Callback receiving the new status text of the pre-launch update check.
pub type StatusChangedCallback<'a> = Box<dyn Fn(&str) + 'a>;
/// Callback receiving the download progress percentage (0 to 100).
pub type ProgressChangedCallback<'a> = Box<dyn Fn(i16) + 'a>;
/// Callback receiving the error of a failed update check, or None when the user cancelled it.
pub type ErrorOrCancelledCallback<'a> = Box<dyn Fn(Option<&Error>) + 'a>;

/// Configuration options for the pre-launch update checker dialog.
pub struct PreLaunchOptions<'a> {
    /// Custom window title for the pre-launch checking window.
    /// If None, defaults to "{AppId} - Aggiornamento".
    pub window_title: Option<String>,
    /// If true, forces the update to be mandatory regardless of release manifest settings.
    /// When an update is mandatory, closing the update window or cancelling will exit the process
    /// immediately without launching the old version of the app.
    /// See `allow_offline_launch` to still launch the installed version when the update server is unreachable.
    pub is_mandatory: bool,
    /// If true, the latest installed version of the app is launched when the update cannot be checked
    /// (no connection, DNS failure, `check_timeout` elapsed, server error or invalid feed),
    /// or when the connection drops while downloading the update, even if the update is mandatory.
    /// Other failures (e.g. a corrupt download or a failed apply) still abort startup when the update is mandatory.
    /// Disabled by default.
    pub allow_offline_launch: bool,
    /// If true, the installed version of the app is launched when the user closes the update window, even if the
    /// update is mandatory (otherwise a mandatory update exits the process). The update is checked again at the
    /// next start. Disabled by default.
    pub allow_launch_on_cancel: bool,
    /// Timeout for checking remote update feed (defaults to 15 seconds).
    pub check_timeout: Duration,
    /// Maximum time for downloading the update. When it elapses the download counts as a lost connection
    /// (see `allow_offline_launch`). None (the default) means no limit.
    pub download_timeout: Option<Duration>,
    /// Whether to display a GUI window during the update check and download (Windows only).
    /// Enabled by default.
    pub show_ui: bool,
    /// Optional callback invoked when the update check status text changes.
    pub on_status_changed: Option<StatusChangedCallback<'a>>,
    /// Optional callback invoked with download progress percentage (0 to 100).
    pub on_progress_changed: Option<ProgressChangedCallback<'a>>,
    /// Optional callback invoked if an update check fails (with the error) or is cancelled by the user (with None).
    pub on_error_or_cancelled: Option<ErrorOrCancelledCallback<'a>>,
}

impl Default for PreLaunchOptions<'_> {
    fn default() -> Self {
        PreLaunchOptions {
            window_title: None,
            is_mandatory: false,
            allow_offline_launch: false,
            allow_launch_on_cancel: false,
            check_timeout: Duration::from_secs(15),
            download_timeout: None,
            show_ui: true,
            on_status_changed: None,
            on_progress_changed: None,
            on_error_or_cancelled: None,
        }
    }
}

/// What the application should do once the pre-launch check is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreLaunchOutcome {
    /// Continue starting the installed version.
    Launch,
    /// Exit the process with this code (an update is being applied, or a mandatory update could not be installed).
    Exit(i32),
}

#[derive(Default)]
struct CheckState {
    is_mandatory: bool,
    downloading: bool,
}

enum Wait<T> {
    Done(T),
    Cancelled,
    TimedOut,
    Lost,
}

/// Checks for updates before the application starts: shows the progress window, downloads an available update
/// and applies it (restarting the app). Mirrors VelopackApp.PerformPreLaunchUpdateCheck in lib-csharp.
pub(crate) fn perform_pre_launch_update_check(
    source: Box<dyn UpdateSource>,
    opts: &PreLaunchOptions,
    locator: Option<VelopackLocatorConfig>,
    auto_apply: bool,
    args: &[String],
) -> PreLaunchOutcome {
    let mut state = CheckState {
        is_mandatory: opts.is_mandatory,
        downloading: false,
    };
    let user_cancel = Arc::new(AtomicBool::new(false));
    let mut ui = Ui::default();

    let mgr = match UpdateManager::new_boxed(source, None, locator) {
        Ok(mgr) => mgr,
        Err(Error::NotInstalled(_)) => {
            info!("Pre-launch update check skipped: the application is not installed.");
            return PreLaunchOutcome::Launch;
        }
        Err(e) => return handle_failure(e, opts, &state, &user_cancel, &ui),
    };

    info!("Executing pre-launch update check...");
    if opts.show_ui {
        ui.show(&mgr, opts.window_title.as_deref(), &user_cancel);
    }

    match check_download_and_apply(&mgr, opts, &ui, &user_cancel, auto_apply, args, &mut state) {
        Ok(outcome) => outcome,
        Err(e) => handle_failure(e, opts, &state, &user_cancel, &ui),
    }
}

fn check_download_and_apply(
    mgr: &UpdateManager,
    opts: &PreLaunchOptions,
    ui: &Ui,
    user_cancel: &Arc<AtomicBool>,
    auto_apply: bool,
    args: &[String],
    state: &mut CheckState,
) -> Result<PreLaunchOutcome, Error> {
    // the check runs on a worker thread so the timeout and the window can interrupt it; an abandoned check
    // simply finishes in the background
    let (check_tx, check_rx) = mpsc::channel();
    let check_mgr = mgr.clone();
    thread::spawn(move || {
        let _ = check_tx.send(check_mgr.check_for_updates());
    });

    let check = match wait_for(&check_rx, Some(opts.check_timeout), user_cancel) {
        Wait::Done(result) => result?,
        Wait::Cancelled => return Err(cancelled_error()),
        Wait::TimedOut => {
            return Err(timeout_error(format!(
                "Pre-launch update check did not complete within {:?}.",
                opts.check_timeout
            )));
        }
        Wait::Lost => return Err(Error::Other("The pre-launch update check stopped unexpectedly.".to_owned())),
    };

    let update = match check {
        UpdateCheck::UpdateAvailable(update) => *update,
        UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => {
            info!("Pre-launch check: No update available.");
            ui.close();
            return Ok(PreLaunchOutcome::Launch);
        }
    };

    let target_version = update.TargetFullRelease.Version.clone();
    state.is_mandatory = opts.is_mandatory || update.TargetFullRelease.IsMandatory;
    info!("Pre-launch check: Update available {}. Mandatory: {}", target_version, state.is_mandatory);

    let download_msg = format!("Download aggiornamento v{}... Attendi l'avvio.", target_version);
    if let Some(cb) = &opts.on_status_changed {
        cb(&download_msg);
    }
    // deltas are downloaded instead of the full package when available
    let download_size: u64 = if !update.DeltasToTarget.is_empty() {
        update.DeltasToTarget.iter().map(|d| d.Size).sum()
    } else {
        update.TargetFullRelease.Size
    };
    ui.set_update_info(&target_version, download_size);
    ui.set_status("Download dell'aggiornamento in corso...");
    ui.set_progress(0);

    state.downloading = true;
    let download_cancel = Arc::new(AtomicBool::new(false));
    let (progress_tx, progress_rx) = mpsc::channel::<i16>();
    let (done_tx, done_rx) = mpsc::channel();
    {
        let mgr = mgr.clone();
        let update = update.clone();
        let download_cancel = download_cancel.clone();
        thread::spawn(move || {
            let result = download::run_cancellable(download_cancel, || mgr.download_updates(&update, Some(progress_tx)));
            let _ = done_tx.send(result);
        });
    }

    let report_progress = |progress: i16| {
        if let Some(cb) = &opts.on_progress_changed {
            cb(progress);
        }
        ui.set_progress(progress);
    };

    let started = Instant::now();
    loop {
        while let Ok(progress) = progress_rx.try_recv() {
            report_progress(progress);
        }

        if user_cancel.load(Ordering::SeqCst) {
            download_cancel.store(true, Ordering::SeqCst);
            return Err(cancelled_error());
        }

        if let Some(limit) = opts.download_timeout.filter(|l| !l.is_zero()) {
            if started.elapsed() >= limit {
                // a download too slow to finish counts as a lost connection (see allow_offline_launch)
                download_cancel.store(true, Ordering::SeqCst);
                return Err(timeout_error(format!("Pre-launch update download did not complete within {:?}.", limit)));
            }
        }

        match done_rx.recv_timeout(POLL_INTERVAL) {
            Ok(result) => {
                while let Ok(progress) = progress_rx.try_recv() {
                    report_progress(progress);
                }
                result?;
                break;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::Other("The pre-launch update download stopped unexpectedly.".to_owned()));
            }
        }
    }

    ui.set_status("Installazione aggiornamento in corso...");
    ui.set_progress(100);
    info!(
        "Pre-launch check: Update downloaded successfully. Mandatory: {}. Auto apply: {}",
        state.is_mandatory, auto_apply
    );
    if auto_apply {
        info!("Auto apply is true, applying {} and restarting...", target_version);
        mgr.wait_exit_then_apply_updates(&update.TargetFullRelease, false, true, args)?;
        ui.close();
        return Ok(PreLaunchOutcome::Exit(0));
    }

    info!("Auto apply disabilitato: l'update resta scaricato ma non viene applicato ora.");
    ui.close();
    Ok(PreLaunchOutcome::Launch)
}

fn handle_failure(error: Error, opts: &PreLaunchOptions, state: &CheckState, user_cancel: &AtomicBool, ui: &Ui) -> PreLaunchOutcome {
    ui.close();

    if user_cancel.load(Ordering::SeqCst) {
        warn!("Pre-launch update check cancelled by the user. Mandatory: {}", state.is_mandatory);
        if let Some(cb) = &opts.on_error_or_cancelled {
            cb(None);
        }
        if state.is_mandatory && !opts.allow_launch_on_cancel {
            return PreLaunchOutcome::Exit(1);
        } else if state.is_mandatory {
            warn!("Launching the installed version (AllowLaunchOnCancel is enabled).");
        }
        return PreLaunchOutcome::Launch;
    }

    error!("Pre-launch update check failed: {}", error);
    if let Some(cb) = &opts.on_error_or_cancelled {
        cb(Some(&error));
    }

    // a failure while checking always means the feed could not be verified; while downloading
    // only a lost connection counts as offline, a corrupt package must not be bypassed.
    let offline = !state.downloading || is_network_failure(&error);
    if state.is_mandatory && opts.allow_offline_launch && offline {
        warn!("Update server unreachable, launching the installed version (AllowOfflineLaunch is enabled).");
    } else if state.is_mandatory {
        return PreLaunchOutcome::Exit(1);
    }
    PreLaunchOutcome::Launch
}

fn wait_for<T>(rx: &Receiver<T>, timeout: Option<Duration>, cancel: &AtomicBool) -> Wait<T> {
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Wait::Cancelled;
        }
        if timeout.is_some_and(|t| started.elapsed() >= t) {
            return Wait::TimedOut;
        }
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(value) => return Wait::Done(value),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Wait::Lost,
        }
    }
}

fn cancelled_error() -> Error {
    Error::Other("The pre-launch update check was cancelled by the user.".to_owned())
}

fn timeout_error(message: String) -> Error {
    std::io::Error::new(std::io::ErrorKind::TimedOut, message).into()
}

/// True if the error means the update server could not be reached or the connection was lost
/// (the C# library treats HttpRequestException, WebException, SocketException, TimeoutException and
/// TaskCanceledException the same way).
pub(crate) fn is_network_failure(error: &Error) -> bool {
    match error {
        Error::Network(_) => true,
        Error::Io(e) => is_network_io_error(e),
        _ => false,
    }
}

fn is_network_io_error(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    if matches!(
        error.kind(),
        ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected | AddrNotAvailable | BrokenPipe | TimedOut | UnexpectedEof
    ) {
        return true;
    }

    // walk the wrapped errors (eg. a ureq::Error surfaced through a response body reader)
    let mut inner = error.get_ref().map(|e| e as &(dyn std::error::Error + 'static));
    while let Some(e) = inner {
        if e.is::<ureq::Error>() {
            return true;
        }
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if is_network_io_error(io) {
                return true;
            }
        }
        inner = e.source();
    }
    false
}

/// The progress window, when it is shown (Windows only).
#[derive(Default)]
struct Ui {
    #[cfg(target_os = "windows")]
    window: Option<crate::prelaunch_window::PreLaunchWindow>,
}

impl Ui {
    #[cfg(target_os = "windows")]
    fn show(&mut self, mgr: &UpdateManager, title: Option<&str>, user_cancel: &Arc<AtomicBool>) {
        let user_cancel = user_cancel.clone();
        let window = crate::prelaunch_window::PreLaunchWindow::new(
            &mgr.get_app_id(),
            &mgr.get_current_version_as_string(),
            title,
            Box::new(move || user_cancel.store(true, Ordering::SeqCst)),
        );
        window.show("Controllo aggiornamenti in corso...");
        self.window = Some(window);
    }

    #[cfg(not(target_os = "windows"))]
    fn show(&mut self, _mgr: &UpdateManager, _title: Option<&str>, _user_cancel: &Arc<AtomicBool>) {}

    fn set_status(&self, _status: &str) {
        #[cfg(target_os = "windows")]
        if let Some(window) = &self.window {
            window.set_status(_status);
        }
    }

    fn set_progress(&self, _progress: i16) {
        #[cfg(target_os = "windows")]
        if let Some(window) = &self.window {
            window.set_progress(_progress as i32);
        }
    }

    fn set_update_info(&self, _target_version: &str, _download_size: u64) {
        #[cfg(target_os = "windows")]
        if let Some(window) = &self.window {
            window.set_update_info(_target_version, _download_size);
        }
    }

    fn close(&self) {
        #[cfg(target_os = "windows")]
        if let Some(window) = &self.window {
            window.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::Manifest;
    use crate::{VelopackAsset, VelopackAssetFeed};
    use std::cell::RefCell;
    use std::path::Path;
    use std::sync::mpsc::Sender;

    #[test]
    fn velopack_asset_supports_mandatory_flag() {
        let json = r#"{
            "Assets": [
                {
                    "PackageId": "TestApp",
                    "Version": "1.2.3",
                    "Type": "Full",
                    "FileName": "TestApp-1.2.3-full.nupkg",
                    "SHA1": "abc",
                    "SHA256": "def",
                    "Size": 1024,
                    "IsMandatory": true
                }
            ]
        }"#;
        let feed: VelopackAssetFeed = serde_json::from_str(json).unwrap();
        assert_eq!(feed.Assets.len(), 1);
        assert_eq!(feed.Assets[0].PackageId, "TestApp");
        assert!(feed.Assets[0].IsMandatory);
    }

    #[test]
    fn pre_launch_options_default_correctly() {
        let opts = PreLaunchOptions::default();
        assert!(opts.show_ui);
        assert!(!opts.is_mandatory);
        assert_eq!(opts.check_timeout, Duration::from_secs(15));
        assert!(!opts.allow_offline_launch);
        assert!(!opts.allow_launch_on_cancel);
        assert!(opts.download_timeout.is_none());
    }

    #[test]
    fn check_failure_exits_only_when_mandatory_and_offline_launch_disabled() {
        for (mandatory, allow_offline, expected) in [
            (true, true, PreLaunchOutcome::Launch),
            (true, false, PreLaunchOutcome::Exit(1)),
            (false, false, PreLaunchOutcome::Launch),
        ] {
            let source = FakeSource {
                feed_error: Some(network_error),
                ..Default::default()
            };
            let (outcome, reported) = run_pre_launch(source, mandatory, allow_offline, None, None);
            assert_eq!(outcome, expected, "mandatory={} allow_offline={}", mandatory, allow_offline);
            assert!(reported.unwrap().contains("Network error"));
        }
    }

    #[test]
    fn check_timeout_launches_installed_version_when_offline_launch_allowed() {
        let source = FakeSource {
            feed_never_completes: true,
            ..Default::default()
        };
        let (outcome, reported) = run_pre_launch(source, true, true, Some(Duration::from_millis(200)), None);
        assert_eq!(outcome, PreLaunchOutcome::Launch);
        assert!(reported.unwrap().contains("did not complete within"));
    }

    #[test]
    fn invalid_feed_counts_as_offline_during_check() {
        let source = FakeSource {
            feed_error: Some(invalid_feed_error),
            ..Default::default()
        };
        let (outcome, _) = run_pre_launch(source, true, true, None, None);
        assert_eq!(outcome, PreLaunchOutcome::Launch);
    }

    #[test]
    fn connection_lost_during_download_launches_installed_version_when_offline_launch_allowed() {
        let source = FakeSource {
            download_error: Some(connection_reset_error),
            ..Default::default()
        };
        let (outcome, reported) = run_pre_launch(source, true, true, None, None);
        assert_eq!(outcome, PreLaunchOutcome::Launch);
        assert!(reported.unwrap().contains("connection reset"));
    }

    #[test]
    fn non_network_download_failure_still_exits_when_mandatory() {
        let source = FakeSource {
            download_error: Some(checksum_error),
            ..Default::default()
        };
        let (outcome, reported) = run_pre_launch(source, true, true, None, None);
        assert_eq!(outcome, PreLaunchOutcome::Exit(1));
        assert!(reported.unwrap().contains("Checksum"));
    }

    #[test]
    fn download_timeout_counts_as_offline() {
        for (allow_offline, expected) in [(true, PreLaunchOutcome::Launch), (false, PreLaunchOutcome::Exit(1))] {
            let source = FakeSource {
                download_never_completes: true,
                ..Default::default()
            };
            let (outcome, reported) = run_pre_launch(source, true, allow_offline, None, Some(Duration::from_millis(200)));
            assert_eq!(outcome, expected, "allow_offline={}", allow_offline);
            assert!(reported.unwrap().contains("download did not complete within"));
        }
    }

    #[test]
    fn network_failure_detection_walks_inner_errors() {
        let wrapped = std::io::Error::other(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "read"));
        assert!(is_network_failure(&Error::Io(wrapped)));
        assert!(is_network_failure(&timeout_error("slow".to_owned())));
        assert!(!is_network_failure(&checksum_error()));
        assert!(!is_network_failure(&Error::Io(std::io::Error::other("disk full"))));
    }

    #[test]
    fn cancellable_download_stops_when_token_is_set() {
        let token = Arc::new(AtomicBool::new(true));
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let result = download::run_cancellable(token, || download::download_url_to_file("http://127.0.0.1:9", tmp.path(), None, |_| {}));
        match result {
            Err(Error::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::Interrupted),
            other => panic!("expected a cancelled download, got {:?}", other.map(|_| ())),
        }
    }

    fn network_error() -> Error {
        url::Url::parse("not a url").unwrap_err().into()
    }

    fn invalid_feed_error() -> Error {
        serde_json::from_str::<VelopackAssetFeed>("<html>captive portal</html>")
            .unwrap_err()
            .into()
    }

    fn connection_reset_error() -> Error {
        std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset").into()
    }

    fn checksum_error() -> Error {
        Error::ChecksumInvalid("file".into(), "abc".to_owned(), "def".to_owned())
    }

    fn run_pre_launch(
        source: FakeSource,
        mandatory: bool,
        allow_offline: bool,
        timeout: Option<Duration>,
        download_timeout: Option<Duration>,
    ) -> (PreLaunchOutcome, Option<String>) {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = create_installed_app(tmp.path(), "MyCoolApp", "1.0.0");
        let reported = RefCell::new(None);
        let opts = PreLaunchOptions {
            is_mandatory: mandatory,
            allow_offline_launch: allow_offline,
            show_ui: false,
            check_timeout: timeout.unwrap_or(Duration::from_secs(10)),
            download_timeout,
            on_error_or_cancelled: Some(Box::new(|e: Option<&Error>| *reported.borrow_mut() = e.map(|e| e.to_string()))),
            ..Default::default()
        };
        let outcome = perform_pre_launch_update_check(Box::new(source), &opts, Some(config), true, &[]);
        drop(opts);
        (outcome, reported.into_inner())
    }

    fn create_installed_app(root: &Path, id: &str, version: &str) -> VelopackLocatorConfig {
        let current = root.join("current");
        std::fs::create_dir_all(&current).unwrap();
        let update_exe = root.join("Update.exe");
        std::fs::write(&update_exe, b"").unwrap();
        let manifest = current.join("sq.version");
        let nuspec = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2010/07/nuspec.xsd">
  <metadata>
    <id>{id}</id>
    <version>{version}</version>
    <title>{id}</title>
    <authors>test</authors>
    <description>test</description>
    <mainExe>test.exe</mainExe>
  </metadata>
</package>"#
        );
        std::fs::write(&manifest, nuspec).unwrap();
        VelopackLocatorConfig {
            RootAppDir: root.to_path_buf(),
            UpdateExePath: update_exe,
            PackagesDir: root.join("packages"),
            ManifestPath: manifest,
            CurrentBinaryDir: current,
            IsPortable: false,
        }
    }

    #[derive(Default)]
    struct FakeSource {
        feed_error: Option<fn() -> Error>,
        feed_never_completes: bool,
        download_error: Option<fn() -> Error>,
        download_never_completes: bool,
    }

    impl UpdateSource for FakeSource {
        fn get_release_feed(&self, _channel: &str, _app: &Manifest, _staged_user_id: &str) -> Result<VelopackAssetFeed, Error> {
            if self.feed_never_completes {
                thread::sleep(Duration::from_secs(60));
            }
            if let Some(error) = self.feed_error {
                return Err(error());
            }
            Ok(VelopackAssetFeed {
                Assets: vec![VelopackAsset {
                    PackageId: "MyCoolApp".to_owned(),
                    Version: "2.0.0".to_owned(),
                    Type: "Full".to_owned(),
                    FileName: "MyCoolApp-2.0.0-full.nupkg".to_owned(),
                    Size: 1,
                    ..Default::default()
                }],
            })
        }

        fn download_release_entry(&self, _asset: &VelopackAsset, _local_file: &Path, _progress: Option<Sender<i16>>) -> Result<(), Error> {
            if self.download_never_completes {
                thread::sleep(Duration::from_secs(60));
            }
            Err(self
                .download_error
                .map(|e| e())
                .unwrap_or_else(|| Error::NotSupported("download".to_owned())))
        }
    }
}
