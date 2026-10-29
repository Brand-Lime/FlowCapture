mod common;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) mod linux_capture;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_recorder;
mod permissions;
mod traits;
#[cfg(target_os = "windows")]
mod windows;

pub mod browser_bridge;
pub mod browser_extensions;
pub mod windows_uia;

pub use permissions::{
    check_recording_permissions, open_accessibility_settings, open_screen_recording_settings,
    preflight_recording_start, prepare_recording_permissions, request_accessibility_permission,
    reveal_executable_in_finder, RecordingPermissions,
};
#[cfg(all(debug_assertions, target_os = "macos"))]
pub use macos::refresh_dev_dock_icon;
pub use traits::*;

pub use common::find_monitor;
pub use common::{list_monitors, SharedScreenshotCapturer};

use anyhow::Result;

// One `return` per platform block: only one of them is compiled in.
#[allow(clippy::needless_return)]
pub fn create_platform_services() -> Result<PlatformServices> {
    #[cfg(target_os = "macos")]
    {
        return macos::MacPlatform::new();
    }
    #[cfg(target_os = "windows")]
    {
        Ok(windows::WindowsPlatform::new()?.into_services())
    }
    #[cfg(target_os = "linux")]
    {
        return Ok(linux::LinuxPlatform::new()?.into_services());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!("unsupported platform")
    }
}

pub struct PlatformServices {
    pub recorder: Box<dyn ScreenRecorder>,
    pub input: Box<dyn InputEventSource>,
    pub windows: Box<dyn WindowTracker>,
    pub platform_name: String,
}
