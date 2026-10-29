use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use dbus::blocking::Connection;
use percent_encoding::percent_decode;
use xcap::Monitor;

/// Capture the primary monitor on Linux.
///
/// On Wayland (GNOME/Ubuntu 22.04), xcap crops portal screenshots using X11 monitor
/// coordinates that are wrong under Wayland, producing solid-color images. We use
/// full-screen GNOME/portal capture first, then fall back to xcap on X11.
pub fn capture_primary_monitor(output_path: PathBuf) -> Result<PathBuf> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut errors = Vec::new();

    if is_wayland() {
        for attempt in [
            try_gnome_shell_screenshot,
            try_gnome_screenshot_cli,
            try_portal_screenshot,
        ] {
            match attempt(&output_path) {
                Ok(()) if image_has_content(&output_path) => return Ok(output_path),
                Ok(()) => errors.push("capture returned a blank image".to_string()),
                Err(err) => errors.push(err.to_string()),
            }
        }
    }

    match capture_with_xcap(&output_path) {
        Ok(()) if image_has_content(&output_path) => return Ok(output_path),
        Ok(()) => errors.push("xcap returned a blank image".to_string()),
        Err(err) => errors.push(err.to_string()),
    }

    anyhow::bail!(
        "Linux screen capture failed. {}",
        errors.join("; ")
    )
}

fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|session| session.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
}

fn try_gnome_shell_screenshot(output_path: &Path) -> Result<()> {
    let conn = Connection::new_session().context("failed to connect to session dbus")?;
    let proxy = conn.with_proxy(
        "org.gnome.Shell.Screenshot",
        "/org/gnome/Shell/Screenshot",
        Duration::from_secs(10),
    );

    let filename = output_path
        .to_str()
        .context("screenshot path is not valid UTF-8")?
        .to_string();

    proxy
        .method_call::<(), (bool, String), &str, &str>(
            "org.gnome.Shell.Screenshot",
            "Screenshot",
            (false, filename),
        )
        .context("org.gnome.Shell.Screenshot.Screenshot failed")?;

    ensure_file(output_path)
}

fn try_gnome_screenshot_cli(output_path: &Path) -> Result<()> {
    if !command_exists("gnome-screenshot") {
        anyhow::bail!("gnome-screenshot not installed");
    }

    let status = Command::new("gnome-screenshot")
        .arg("-f")
        .arg(output_path)
        .status()
        .context("failed to run gnome-screenshot")?;

    if !status.success() {
        anyhow::bail!("gnome-screenshot exited with {status}");
    }

    ensure_file(output_path)
}

fn try_portal_screenshot(output_path: &Path) -> Result<()> {
    use dbus::arg::{AppendAll, Iter, IterAppend, PropMap, ReadAll, RefArg, TypeMismatchError, Variant};
    use dbus::message::{MatchRule, SignalArgs};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct PortalResponse {
        status: u32,
        results: PropMap,
    }

    impl AppendAll for PortalResponse {
        fn append(&self, i: &mut IterAppend) {
            RefArg::append(&self.status, i);
            RefArg::append(&self.results, i);
        }
    }

    impl ReadAll for PortalResponse {
        fn read(i: &mut Iter) -> Result<Self, TypeMismatchError> {
            Ok(PortalResponse {
                status: i.read()?,
                results: i.read()?,
            })
        }
    }

    impl SignalArgs for PortalResponse {
        const NAME: &'static str = "Response";
        const INTERFACE: &'static str = "org.freedesktop.portal.Request";
    }

    let conn = Connection::new_session().context("failed to connect to session dbus")?;
    let status: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
    let status_res = status.clone();
    let uri: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let uri_res = uri.clone();

    let match_rule = MatchRule::new_signal("org.freedesktop.portal.Request", "Response");
    conn.add_match(match_rule, move |response: PortalResponse, _, _| {
        if let Ok(mut guard) = status_res.lock() {
            *guard = Some(response.status);
        }
        if let Some(value) = response.results.get("uri").and_then(|item| item.as_str()) {
            if let Ok(mut guard) = uri_res.lock() {
                *guard = value.to_string();
            }
        }
        true
    })?;

    let proxy = conn.with_proxy(
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        Duration::from_secs(10),
    );

    let mut options: PropMap = HashMap::new();
    options.insert(
        "handle_token".to_string(),
        Variant(Box::new("flowcapture".to_string())),
    );
    options.insert("interactive".to_string(), Variant(Box::new(false)));
    options.insert("modal".to_string(), Variant(Box::new(false)));

    proxy.method_call::<(), (&str, PropMap), &str, &str>(
        "org.freedesktop.portal.Screenshot",
        "Screenshot",
        ("", options),
    )?;

    for _ in 0..30 {
        let _ = conn.process(Duration::from_millis(500))?;
        let done = status
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false);
        if done {
            break;
        }
    }

    let portal_status = status
        .lock()
        .map(|guard| *guard)
        .unwrap_or(None)
        .unwrap_or(1);
    let uri_value = uri.lock().map(|guard| guard.clone()).unwrap_or_default();

    if portal_status != 0 || uri_value.is_empty() {
        anyhow::bail!("xdg-desktop-portal screenshot failed or was denied");
    }

    let decoded = percent_decode(uri_value.trim_start_matches("file://").as_bytes())
        .decode_utf8()
        .context("failed to decode portal screenshot URI")?;

    std::fs::copy(decoded.as_ref(), output_path).context("failed to copy portal screenshot")?;
    let _ = std::fs::remove_file(decoded.as_ref());

    ensure_file(output_path)
}

fn capture_with_xcap(output_path: &Path) -> Result<()> {
    let monitor = Monitor::all()
        .context("xcap could not list monitors")?
        .into_iter()
        .next()
        .context("nessuno schermo trovato")?;

    let image = monitor
        .capture_image()
        .context("xcap monitor capture failed")?;

    image
        .save(output_path)
        .map_err(|err| anyhow::anyhow!("failed to save xcap screenshot: {err}"))?;

    Ok(())
}

fn ensure_file(path: &Path) -> Result<()> {
    if path.is_file() && path.metadata()?.len() > 0 {
        Ok(())
    } else {
        anyhow::bail!("screenshot file was not created at {}", path.display())
    }
}

fn image_has_content(path: &Path) -> bool {
    let Ok(image) = image::open(path) else {
        return false;
    };

    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width < 8 || height < 8 {
        return false;
    }

    let step = ((width as usize * height as usize) / 512).max(1);
    let mut samples = Vec::new();
    for (index, pixel) in rgba.pixels().enumerate() {
        if index % step == 0 {
            samples.push([pixel[0], pixel[1], pixel[2]]);
        }
    }

    if samples.len() < 8 {
        return false;
    }

    let first = samples[0];
    !samples.iter().all(|sample| colors_similar(*sample, first))
}

fn colors_similar(a: [u8; 3], b: [u8; 3]) -> bool {
    (a[0] as i16 - b[0] as i16).abs() <= 8
        && (a[1] as i16 - b[1] as i16).abs() <= 8
        && (a[2] as i16 - b[2] as i16).abs() <= 8
}

fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {command}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
