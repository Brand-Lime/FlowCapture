use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RecordingPermissions {
    pub screen_recording_granted: bool,
    pub accessibility_granted: bool,
    pub input_monitoring_granted: bool,
    pub can_record: bool,
    pub process_name: String,
    pub executable_path: String,
    pub is_dev_mode: bool,
    pub settings_app_names: Vec<String>,
    pub message: String,
    pub help_steps: Vec<String>,
}

pub fn check_recording_permissions() -> RecordingPermissions {
    #[cfg(target_os = "macos")]
    {
        return macos::check();
    }

    #[cfg(not(target_os = "macos"))]
    {
        RecordingPermissions {
            screen_recording_granted: true,
            accessibility_granted: true,
            input_monitoring_granted: true,
            can_record: true,
            process_name: current_process_name(),
            executable_path: current_executable_path(),
            is_dev_mode: cfg!(debug_assertions),
            settings_app_names: vec![current_process_name()],
            message: "Permissions look good.".to_string(),
            help_steps: Vec::new(),
        }
    }
}

pub fn prepare_recording_permissions() -> RecordingPermissions {
    #[cfg(target_os = "macos")]
    {
        macos::trigger_permission_prompts();
    }

    check_recording_permissions()
}

pub fn request_accessibility_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        return macos::request_accessibility();
    }

    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

pub fn reveal_executable_in_finder() -> anyhow::Result<()> {
    let executable = current_executable_path();
    std::process::Command::new("open")
        .arg("-R")
        .arg(&executable)
        .spawn()
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!("failed to reveal executable in Finder: {err}"))
}

pub fn open_screen_recording_settings() -> anyhow::Result<()> {
    open_settings("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
}

pub fn open_accessibility_settings() -> anyhow::Result<()> {
    open_settings("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
}

fn open_settings(url: &str) -> anyhow::Result<()> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!("failed to open system settings: {err}"))
}

fn current_executable_path() -> String {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn current_process_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "FlowCapture".to_string())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{current_executable_path, current_process_name, RecordingPermissions};
    use core_graphics::access::ScreenCaptureAccess;
    use macos_accessibility_client::accessibility::{
        application_is_trusted, application_is_trusted_with_prompt,
    };
    use xcap::Monitor;

    pub fn check() -> RecordingPermissions {
        let process_name = current_process_name();
        let executable_path = current_executable_path();
        let is_dev_mode = cfg!(debug_assertions);
        let settings_app_names = settings_app_names(&process_name, is_dev_mode);
        let screen_recording_granted = screen_recording_granted();
        let accessibility_granted = application_is_trusted();
        let input_monitoring_granted = accessibility_granted;

        let mut help_steps = Vec::new();
        if !screen_recording_granted {
            help_steps.push(format!(
                "Open System Settings → Privacy & Security → Screen Recording, then enable `{process_name}`."
            ));
            help_steps.push(
                "Click \"Request Permissions\" in FlowCapture first. macOS only adds apps after they request access."
                    .to_string(),
            );
            if is_dev_mode {
                help_steps.push(format!(
                    "In dev mode the app appears as `{process_name}` (not \"FlowCapture\"). Use \"Reveal App in Finder\" and the + button if it is missing."
                ));
            }
            help_steps.push(
                "After enabling Screen Recording, fully quit FlowCapture (Cmd+Q) and reopen it before recording."
                    .to_string(),
            );
        }
        if !accessibility_granted {
            help_steps.push(format!(
                "Open System Settings → Privacy & Security → Accessibility, then enable `{process_name}`."
            ));
            help_steps.push(
                "If it is not listed, click \"Request Permissions\" in FlowCapture to show the macOS prompt, or use \"Reveal App in Finder\" and add the app with the + button."
                    .to_string(),
            );
            if is_dev_mode {
                help_steps.push(
                    "When running from Cursor or Terminal, you may also need to enable that app in Accessibility."
                        .to_string(),
                );
            }
        }
        if !screen_recording_granted && accessibility_granted {
            help_steps.push(
                "Screen Recording may already be enabled in System Settings, but this running process has not picked it up yet. Quit FlowCapture completely, restart it, then click Check Again."
                    .to_string(),
            );
            help_steps.push(
                "Or switch to another app (Safari, Finder), then click Check Again to verify screen capture."
                    .to_string(),
            );
        }

        let can_record = screen_recording_granted && accessibility_granted;
        let message = if can_record {
            "FlowCapture can record your screen and input.".to_string()
        } else if !screen_recording_granted && !accessibility_granted {
            "Screen Recording and Accessibility permissions are required before recording can start."
                .to_string()
        } else if !screen_recording_granted {
            "Screen Recording permission is required to capture other apps.".to_string()
        } else {
            "Accessibility permission is required to track windows, clicks, and keyboard input."
                .to_string()
        };

        RecordingPermissions {
            screen_recording_granted,
            accessibility_granted,
            input_monitoring_granted,
            can_record,
            process_name,
            executable_path,
            is_dev_mode,
            settings_app_names,
            message,
            help_steps,
        }
    }

    pub fn trigger_permission_prompts() {
        let access = ScreenCaptureAccess::default();
        if !access.preflight() {
            let _ = access.request();
        }

        warm_up_screen_capture();
        request_accessibility();
    }

    pub fn request_accessibility() -> bool {
        let trusted = application_is_trusted_with_prompt();
        let _ = active_win_pos_rs::get_active_window();
        trusted || application_is_trusted()
    }

    fn screen_recording_granted() -> bool {
        let access = ScreenCaptureAccess::default();
        if access.preflight() {
            return true;
        }

        warm_up_screen_capture();

        if access.preflight() {
            return true;
        }

        probe_foreign_window_capture()
    }

    fn warm_up_screen_capture() {
        let _ = Monitor::all().and_then(|monitors| {
            monitors
                .into_iter()
                .next()
                .ok_or_else(|| xcap::XCapError::new("nessuno schermo trovato"))
                .and_then(|monitor| monitor.capture_image())
        });
    }

    fn probe_foreign_window_capture() -> bool {
        let Ok(active) = active_win_pos_rs::get_active_window() else {
            return false;
        };
        if is_self_app(&active.app_name) {
            return false;
        }

        let Ok(monitors) = Monitor::all() else {
            return false;
        };
        let Some(monitor) = monitors.into_iter().next() else {
            return false;
        };
        let Ok(image) = monitor.capture_image() else {
            return false;
        };

        let cx = (active.position.x + active.position.width / 2.0).max(0.0) as u32;
        let cy = (active.position.y + active.position.height / 2.0).max(0.0) as u32;
        let (width, height) = image.dimensions();
        if cx >= width || cy >= height {
            return false;
        }

        image_has_local_variance(&image, cx, cy)
    }

    fn is_self_app(app_name: &str) -> bool {
        let self_name = current_process_name().to_lowercase();
        app_name.eq_ignore_ascii_case(&self_name) || app_name.eq_ignore_ascii_case("FlowCapture")
    }

    fn image_has_local_variance(image: &image::RgbaImage, cx: u32, cy: u32) -> bool {
        let offsets = [(20i32, 0), (-20, 0), (0, 20), (0, -20), (0, 0)];
        let mut samples = Vec::new();

        for (dx, dy) in offsets {
            let x = cx as i32 + dx;
            let y = cy as i32 + dy;
            if x >= 0 && y >= 0 && (x as u32) < image.width() && (y as u32) < image.height() {
                let pixel = image.get_pixel(x as u32, y as u32);
                samples.push(pixel.0[0] as u32 + pixel.0[1] as u32 + pixel.0[2] as u32);
            }
        }

        if samples.len() < 3 {
            return false;
        }

        let min = *samples.iter().min().unwrap_or(&0);
        let max = *samples.iter().max().unwrap_or(&0);
        max.saturating_sub(min) > 30
    }

    fn settings_app_names(process_name: &str, is_dev_mode: bool) -> Vec<String> {
        let mut names = vec![process_name.to_string()];
        if process_name != "FlowCapture" {
            names.push("FlowCapture".to_string());
        }
        if is_dev_mode {
            for candidate in ["Cursor", "Terminal", "iTerm", "iTerm2", "Code"] {
                if !names.iter().any(|name| name == candidate) {
                    names.push(candidate.to_string());
                }
            }
        }
        names
    }
}

pub fn preflight_recording_start() -> anyhow::Result<RecordingPermissions> {
    let permissions = prepare_recording_permissions();

    if !permissions.can_record {
        anyhow::bail!("{}", permissions.message);
    }

    Ok(permissions)
}
