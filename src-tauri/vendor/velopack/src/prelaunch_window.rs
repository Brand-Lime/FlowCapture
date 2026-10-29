// Native pre-launch update dialog, drawn with plain GDI so it has no UI framework dependency.
// Rust port of lib-csharp UI/VelopackPreLaunchWindow.cs: keep the two in sync (layout, palette, texts, behaviour).
// Follows the Windows light/dark app theme, scales with the monitor DPI and animates the progress bar
// (indeterminate while checking, smooth fill while downloading). All public members are thread safe.

use std::cell::RefCell;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{s, w, PCSTR};
use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::wide_strings::{string_to_wide, WideString};

const BASE_WIDTH: i32 = 480;
const BASE_HEIGHT: i32 = 190;
const TIMER_ID: usize = 1;
const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

// COLORREF values are 0x00BBGGRR
mod palette {
    pub const ACCENT_START: u32 = 0x00FF5C7C; // #7C5CFF
    pub const ACCENT_END: u32 = 0x00EEB822; // #22B8EE

    pub fn background(dark: bool) -> u32 {
        if dark {
            0x00251E1B
        } else {
            0x00FFFFFF
        }
    }
    pub fn text(dark: bool) -> u32 {
        if dark {
            0x00F0EAE8
        } else {
            0x00251E1B
        }
    }
    pub fn muted(dark: bool) -> u32 {
        if dark {
            0x00AB9C95
        } else {
            0x0076665F
        }
    }
    pub fn track(dark: bool) -> u32 {
        if dark {
            0x003E322D
        } else {
            0x00EEE8E6
        }
    }
    pub fn accent_text(dark: bool) -> u32 {
        if dark {
            0x00FF9C8F
        } else {
            0x00E0483F
        }
    }
}

// state written by any thread, read by the window thread while painting
struct State {
    status: String,
    target_version: Option<String>,
    download_size: u64,
    target_progress: i32, // -1 = indeterminate
    download_timer: Option<Instant>,
}

struct Shared {
    app_name: String,
    app_version: String,
    custom_title: Option<String>,
    user_closed: Box<dyn Fn() + Send + Sync>,
    state: Mutex<State>,
    // orders the creation of the window on the window thread with close() on the caller thread
    lifecycle: Mutex<()>,
    hwnd: AtomicIsize,
    closed: AtomicBool,
    thread_started: AtomicBool,
}

// window thread only
struct UiState {
    shown_progress: f64,
    marquee_tick: i32,
    dpi: i32,
    dark: bool,
    font_title: HFONT,
    font_body: HFONT,
    font_small: HFONT,
    font_percent: HFONT,
    app_icon: HICON,
}

struct WindowData {
    shared: Arc<Shared>,
    ui: RefCell<UiState>,
}

/// The pre-launch update window. Nothing is shown until `show` is called.
pub(crate) struct PreLaunchWindow {
    shared: Arc<Shared>,
}

impl PreLaunchWindow {
    /// - `app_name`: name displayed as the window heading.
    /// - `app_version`: the currently installed version.
    /// - `custom_title`: optional caption of the window, defaults to "{app_name} - Aggiornamento".
    /// - `user_closed`: called on the window thread when the user closes the window (not when `close` is called).
    pub fn new(app_name: &str, app_version: &str, custom_title: Option<&str>, user_closed: Box<dyn Fn() + Send + Sync>) -> Self {
        PreLaunchWindow {
            shared: Arc::new(Shared {
                app_name: app_name.to_owned(),
                app_version: app_version.to_owned(),
                custom_title: custom_title.map(|t| t.to_owned()),
                user_closed,
                state: Mutex::new(State {
                    status: String::new(),
                    target_version: None,
                    download_size: 0,
                    target_progress: -1,
                    download_timer: None,
                }),
                lifecycle: Mutex::new(()),
                hwnd: AtomicIsize::new(0),
                closed: AtomicBool::new(false),
                thread_started: AtomicBool::new(false),
            }),
        }
    }

    /// Shows the window on a dedicated UI thread and returns once it is visible (or after 3 seconds).
    pub fn show(&self, initial_status: &str) {
        lock(&self.shared.state).status = initial_status.to_owned();
        {
            let _lifecycle = lock(&self.shared.lifecycle);
            if self.shared.closed.load(Ordering::SeqCst) || self.shared.thread_started.load(Ordering::SeqCst) {
                return;
            }
            self.shared.thread_started.store(true, Ordering::SeqCst);
        }

        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let shared = self.shared.clone();
        let spawned = thread::Builder::new()
            .name("velopack-prelaunch-window".to_owned())
            .spawn(move || window_thread(shared, ready_tx));
        if spawned.is_ok() {
            let _ = ready_rx.recv_timeout(Duration::from_secs(3));
        }
    }

    /// Updates the status line.
    pub fn set_status(&self, status: &str) {
        lock(&self.shared.state).status = status.to_owned();
        self.invalidate();
    }

    /// Sets the download progress (0 to 100). A negative value switches back to the indeterminate animation.
    pub fn set_progress(&self, percent: i32) {
        {
            let mut state = lock(&self.shared.state);
            if percent < 0 {
                state.target_progress = -1;
            } else {
                state.target_progress = percent.clamp(0, 100);
                if state.download_timer.is_none() {
                    state.download_timer = Some(Instant::now());
                }
            }
        }
        self.invalidate();
    }

    /// Shows the version being installed and, when known, the size of the download.
    pub fn set_update_info(&self, target_version: &str, download_size_bytes: u64) {
        {
            let mut state = lock(&self.shared.state);
            state.target_version = Some(target_version.to_owned());
            state.download_size = download_size_bytes;
        }
        self.invalidate();
    }

    /// Closes the window without calling `user_closed`.
    pub fn close(&self) {
        let hwnd = {
            let _lifecycle = lock(&self.shared.lifecycle);
            if self.shared.closed.swap(true, Ordering::SeqCst) {
                return;
            }
            self.shared.hwnd.load(Ordering::SeqCst)
        };

        if hwnd != 0 {
            unsafe {
                let _ = PostMessageW(Some(to_hwnd(hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        // otherwise the window thread is still creating the window (show waited at most 3 seconds):
        // it sees `closed` once the window exists and closes it without showing it
    }

    fn invalidate(&self) {
        let hwnd = self.shared.hwnd.load(Ordering::SeqCst);
        if hwnd != 0 && !self.shared.closed.load(Ordering::SeqCst) {
            unsafe {
                let _ = InvalidateRect(Some(to_hwnd(hwnd)), None, false);
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn to_hwnd(value: isize) -> HWND {
    HWND(value as *mut c_void)
}

// ------------------------------------------------------------------ window thread

fn window_thread(shared: Arc<Shared>, ready: mpsc::Sender<()>) {
    let data = WindowData {
        shared,
        ui: RefCell::new(UiState {
            shown_progress: 0.0,
            marquee_tick: 0,
            dpi: 96,
            dark: false,
            font_title: HFONT::default(),
            font_body: HFONT::default(),
            font_small: HFONT::default(),
            font_percent: HFONT::default(),
            app_icon: HICON::default(),
        }),
    };

    unsafe {
        let created = create_native_window(&data);
        let _ = ready.send(());
        if let Some((_, class_name, hinstance)) = &created {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnregisterClassW(class_name.as_pcwstr(), Some(*hinstance));
        }
        data.ui.borrow_mut().free_resources();
    }
}

unsafe fn create_native_window(data: &WindowData) -> Option<(HWND, WideString, HINSTANCE)> {
    let shared = &data.shared;
    let dark = is_dark_theme();
    let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
    let class_name = string_to_wide(format!("VelopackPreLaunchWindow_{}", uuid::Uuid::new_v4().simple()));

    let wnd_class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH::default(), // painted in WM_PAINT to avoid flicker
        lpszClassName: class_name.as_pcwstr(),
        ..Default::default()
    };
    if RegisterClassExW(&wnd_class) == 0 {
        error!("Pre-launch window: RegisterClassEx failed.");
        return None;
    }

    let title = string_to_wide(
        shared
            .custom_title
            .clone()
            .unwrap_or_else(|| format!("{} - Aggiornamento", shared.app_name)),
    );
    let hwnd = match CreateWindowExW(
        WS_EX_APPWINDOW,
        class_name.as_pcwstr(),
        title.as_pcwstr(),
        STYLE,
        0,
        0,
        BASE_WIDTH,
        BASE_HEIGHT,
        None,
        None,
        Some(hinstance),
        None,
    ) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            error!("Pre-launch window: CreateWindowEx failed: {}", e);
            let _ = UnregisterClassW(class_name.as_pcwstr(), Some(hinstance));
            return None;
        }
    };
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, data as *const WindowData as isize);

    let closed_meanwhile = {
        let _lifecycle = lock(&shared.lifecycle);
        shared.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
        shared.closed.load(Ordering::SeqCst)
    };
    if closed_meanwhile {
        // close() ran before the window existed: never show it, the message loop destroys it
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        return Some((hwnd, class_name, hinstance));
    }

    let dpi = {
        let mut ui = data.ui.borrow_mut();
        ui.dark = dark;
        ui.dpi = get_window_dpi(hwnd);
        ui.create_fonts();
        ui.load_app_icon(hwnd);
        ui.dpi
    };
    apply_title_bar_theme(hwnd, dark);
    position_window(hwnd, dpi);

    SetTimer(Some(hwnd), TIMER_ID, 16, None);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = UpdateWindow(hwnd);
    Some((hwnd, class_name, hinstance))
}

unsafe fn position_window(hwnd: HWND, dpi: i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale(BASE_WIDTH, dpi),
        bottom: scale(BASE_HEIGHT, dpi),
    };
    if !adjust_window_rect_for_dpi(&mut rect, dpi) {
        let _ = AdjustWindowRectEx(&mut rect, STYLE, false, WS_EX_APPWINDOW);
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    // center on the monitor the user is working on
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let (x, y) = if !monitor.is_invalid() && GetMonitorInfoW(monitor, &mut info).as_bool() {
        let work = info.rcWork;
        (
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 2,
        )
    } else {
        ((GetSystemMetrics(SM_CXSCREEN) - width) / 2, (GetSystemMetrics(SM_CYSCREEN) - height) / 2)
    };
    let _ = SetWindowPos(hwnd, None, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
}

// GetDpiForWindow / AdjustWindowRectExForDpi only exist on Windows 10 1607+, so they are resolved at runtime
unsafe fn user32_proc(name: PCSTR) -> Option<unsafe extern "system" fn() -> isize> {
    let module = GetModuleHandleW(w!("user32.dll")).ok()?;
    GetProcAddress(module, name)
}

unsafe fn adjust_window_rect_for_dpi(rect: &mut RECT, dpi: i32) -> bool {
    type AdjustWindowRectExForDpiFn = unsafe extern "system" fn(*mut RECT, u32, i32, u32, u32) -> i32;
    match user32_proc(s!("AdjustWindowRectExForDpi")) {
        Some(proc) => {
            let adjust: AdjustWindowRectExForDpiFn = std::mem::transmute(proc);
            adjust(rect, STYLE.0, 0, WS_EX_APPWINDOW.0, dpi as u32) != 0
        }
        None => false,
    }
}

unsafe fn get_window_dpi(hwnd: HWND) -> i32 {
    type GetDpiForWindowFn = unsafe extern "system" fn(HWND) -> u32;
    if let Some(proc) = user32_proc(s!("GetDpiForWindow")) {
        let get_dpi: GetDpiForWindowFn = std::mem::transmute(proc);
        let dpi = get_dpi(hwnd) as i32;
        if dpi > 0 {
            return dpi;
        }
    }
    let hdc = GetDC(None);
    let dpi = GetDeviceCaps(Some(hdc), LOGPIXELSX);
    ReleaseDC(None, hdc);
    if dpi > 0 {
        dpi
    } else {
        96
    }
}

unsafe fn is_dark_theme() -> bool {
    let mut value: u32 = 1;
    let mut size: u32 = std::mem::size_of::<u32>() as u32;
    let result = RegGetValueW(
        HKEY_CURRENT_USER,
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("AppsUseLightTheme"),
        RRF_RT_REG_DWORD,
        None,
        Some(&mut value as *mut u32 as *mut c_void),
        Some(&mut size),
    );
    result == ERROR_SUCCESS && value == 0
}

unsafe fn apply_title_bar_theme(hwnd: HWND, dark: bool) {
    // dwmapi attributes are cosmetic and missing on older systems
    let value: i32 = dark as i32;
    let size = std::mem::size_of::<i32>() as u32;
    if DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &value as *const i32 as *const c_void, size).is_err() {
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(19), &value as *const i32 as *const c_void, size);
    }
    let caption = palette::background(dark) as i32;
    let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(35), &caption as *const i32 as *const c_void, size);
}

// ------------------------------------------------------------------ message loop

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowData;
    if !data.is_null() {
        // a panic must never unwind into user32
        if let Ok(Some(result)) = catch_unwind(AssertUnwindSafe(|| handle_message(&*data, hwnd, msg, wparam, lparam))) {
            return result;
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn handle_message(data: &WindowData, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_PAINT => {
            data.paint(hwnd);
            Some(LRESULT(0))
        }
        WM_TIMER => {
            if data.animate() {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            {
                let mut ui = data.ui.try_borrow_mut().ok()?;
                ui.dpi = (wparam.0 & 0xFFFF) as i32;
                ui.create_fonts();
                ui.load_app_icon(hwnd);
            }
            let suggested = *(lparam.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                None,
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            Some(LRESULT(0))
        }
        WM_SETTINGCHANGE => {
            let dark = is_dark_theme();
            let changed = match data.ui.try_borrow_mut() {
                Ok(mut ui) if ui.dark != dark => {
                    ui.dark = dark;
                    true
                }
                _ => false,
            };
            if changed {
                apply_title_bar_theme(hwnd, dark);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            None
        }
        WM_CLOSE => {
            if !data.shared.closed.swap(true, Ordering::SeqCst) {
                // a panic in the callback must not escape a native window callback
                let _ = catch_unwind(AssertUnwindSafe(|| (data.shared.user_closed)()));
            }
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            let _ = DestroyWindow(hwnd);
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            Some(LRESULT(0))
        }
        WM_NCDESTROY => {
            data.shared.hwnd.store(0, Ordering::SeqCst);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            None
        }
        _ => None,
    }
}

impl WindowData {
    // returns true when a repaint is needed
    fn animate(&self) -> bool {
        let target = lock(&self.shared.state).target_progress;
        let Ok(mut ui) = self.ui.try_borrow_mut() else {
            return false;
        };

        if target < 0 {
            ui.marquee_tick = ui.marquee_tick.wrapping_add(1);
            return true;
        }
        let target = target as f64;
        let diff = target - ui.shown_progress;
        if diff.abs() < 0.05 {
            if ui.shown_progress != target {
                ui.shown_progress = target;
                return true;
            }
            return false;
        }
        ui.shown_progress += diff * 0.18;
        true
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client).is_ok() && client.right > 0 && client.bottom > 0 {
            let (w, h) = (client.right, client.bottom);
            // double buffered: draw everything off screen, then blit once
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old_bmp = SelectObject(mem, bmp.into());
            if let Ok(ui) = self.ui.try_borrow() {
                self.draw(mem, w, h, &ui);
            }
            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            SelectObject(mem, old_bmp);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
        }
        let _ = EndPaint(hwnd, &ps);
    }

    unsafe fn draw(&self, hdc: HDC, w: i32, h: i32, ui: &UiState) {
        let (status, target, size, progress, elapsed) = {
            let state = lock(&self.shared.state);
            (
                state.status.clone(),
                state.target_version.clone(),
                state.download_size,
                state.target_progress,
                state.download_timer.map(|t| t.elapsed()),
            )
        };
        let app_name = &self.shared.app_name;
        let app_version = &self.shared.app_version;
        let dark = ui.dark;
        let s = |value: i32| ui.s(value);

        fill_rect(hdc, 0, 0, w, h, palette::background(dark));
        // brand accent strip
        fill_gradient(hdc, 0, 0, w, s(3), palette::ACCENT_START, palette::ACCENT_END);

        SetBkMode(hdc, TRANSPARENT);
        let pad = s(24);

        // icon tile
        let icon_size = s(44);
        let icon_y = s(26);
        if !ui.app_icon.is_invalid() {
            let _ = DrawIconEx(hdc, pad, icon_y, ui.app_icon, icon_size, icon_size, 0, None, DI_NORMAL);
        } else {
            fill_rounded(
                hdc,
                pad,
                icon_y,
                icon_size,
                icon_size,
                s(10),
                palette::ACCENT_START,
                palette::ACCENT_END,
                None,
            );
            let initial = app_name
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect::<String>())
                .unwrap_or_else(|| "?".to_owned());
            draw_text(
                hdc,
                &initial,
                ui.font_title,
                0x00FFFFFF,
                (pad, icon_y, pad + icon_size, icon_y + icon_size),
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }

        // heading
        let text_x = pad + icon_size + s(16);
        let single_line = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS;
        draw_text(
            hdc,
            app_name,
            ui.font_title,
            palette::text(dark),
            (text_x, s(24), w - pad, s(52)),
            single_line,
        );

        let version_line = match &target {
            None => format!("Versione installata {}", app_version),
            Some(target) if size > 0 => format!("{}  →  {}   ·   {}", app_version, target, format_size(size)),
            Some(target) => format!("{}  →  {}", app_version, target),
        };
        draw_text(
            hdc,
            &version_line,
            ui.font_body,
            palette::muted(dark),
            (text_x, s(52), w - pad, s(72)),
            single_line,
        );

        // status + percentage
        let status_top = s(96);
        let percent = if progress >= 0 {
            format!("{}%", round_half_even(ui.shown_progress))
        } else {
            String::new()
        };
        let percent_width = s(48);
        draw_text(
            hdc,
            &status,
            ui.font_body,
            palette::text(dark),
            (pad, status_top, w - pad - percent_width, status_top + s(22)),
            single_line,
        );
        if !percent.is_empty() {
            draw_text(
                hdc,
                &percent,
                ui.font_percent,
                palette::accent_text(dark),
                (w - pad - percent_width, status_top, w - pad, status_top + s(22)),
                DT_RIGHT | DT_VCENTER | DT_SINGLELINE,
            );
        }

        // progress track
        let bar_top = status_top + s(30);
        let bar_height = s(8);
        let bar_width = w - 2 * pad;
        let track = palette::track(dark);
        fill_rounded(hdc, pad, bar_top, bar_width, bar_height, bar_height, track, track, None);
        if progress >= 0 {
            let fill = (bar_width as f64 * ui.shown_progress.clamp(0.0, 100.0) / 100.0) as i32;
            if fill > 0 {
                fill_rounded(
                    hdc,
                    pad,
                    bar_top,
                    fill.max(bar_height),
                    bar_height,
                    bar_height,
                    palette::ACCENT_START,
                    palette::ACCENT_END,
                    Some((pad, bar_width)),
                );
            }
        } else {
            // indeterminate: a segment that sweeps across the track
            let segment = bar_width / 3;
            let travel = bar_width + segment;
            if travel > 0 {
                let pos = ((ui.marquee_tick as i64 * s(4) as i64) % travel as i64) as i32 - segment;
                let left = pos.max(0);
                let right = (pos + segment).min(bar_width);
                if right > left {
                    fill_rounded(
                        hdc,
                        pad + left,
                        bar_top,
                        right - left,
                        bar_height,
                        bar_height,
                        palette::ACCENT_START,
                        palette::ACCENT_END,
                        Some((pad + pos, segment)),
                    );
                }
            }
        }

        // footer hint
        let mut hint = if (0..100).contains(&progress) {
            estimate_remaining(elapsed, ui.shown_progress)
        } else {
            String::new()
        };
        if hint.is_empty() {
            hint = "L'applicazione si avvierà automaticamente al termine.".to_owned();
        }
        draw_text(
            hdc,
            &hint,
            ui.font_small,
            palette::muted(dark),
            (pad, bar_top + s(18), w - pad, bar_top + s(38)),
            single_line,
        );
    }
}

impl UiState {
    fn s(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }

    unsafe fn create_fonts(&mut self) {
        self.delete_fonts();
        self.font_title = self.create_font(20, 600);
        self.font_body = self.create_font(14, 400);
        self.font_small = self.create_font(12, 400);
        self.font_percent = self.create_font(14, 600);
    }

    unsafe fn create_font(&self, pixel_height: i32, weight: i32) -> HFONT {
        CreateFontW(
            -self.s(pixel_height),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Segoe UI"),
        )
    }

    unsafe fn delete_fonts(&mut self) {
        for font in [self.font_title, self.font_body, self.font_small, self.font_percent] {
            if !font.is_invalid() {
                let _ = DeleteObject(font.into());
            }
        }
        self.font_title = HFONT::default();
        self.font_body = HFONT::default();
        self.font_small = HFONT::default();
        self.font_percent = HFONT::default();
    }

    unsafe fn load_app_icon(&mut self, hwnd: HWND) {
        if !self.app_icon.is_invalid() {
            let _ = DestroyIcon(self.app_icon);
        }
        self.app_icon = HICON::default();

        // the application icon is decorative only
        if let Ok(exe) = std::env::current_exe() {
            let wide: Vec<u16> = exe.as_os_str().encode_wide().collect();
            let mut path = [0u16; 260];
            if wide.len() < path.len() {
                path[..wide.len()].copy_from_slice(&wide);
                let mut icons = [HICON::default()];
                let size = self.s(44);
                if PrivateExtractIconsW(&path, 0, size, size, Some(&mut icons), None, 0) > 0 && !icons[0].is_invalid() {
                    self.app_icon = icons[0];
                }
            }
        }

        if !self.app_icon.is_invalid() {
            let icon = LPARAM(self.app_icon.0 as isize);
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(icon));
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_SMALL as usize)), Some(icon));
        }
    }

    unsafe fn free_resources(&mut self) {
        self.delete_fonts();
        if !self.app_icon.is_invalid() {
            let _ = DestroyIcon(self.app_icon);
            self.app_icon = HICON::default();
        }
    }
}

// ------------------------------------------------------------------ painting helpers

fn scale(value: i32, dpi: i32) -> i32 {
    // same rounding as Win32 MulDiv(value, dpi, 96)
    let product = value as i64 * dpi as i64;
    let rounded = if product >= 0 {
        (product + 48) / 96
    } else {
        (product - 48) / 96
    };
    rounded as i32
}

// .NET Math.Round rounds half to even
fn round_half_even(value: f64) -> i32 {
    let rounded = value.round();
    if (value - value.trunc()).abs() == 0.5 && rounded % 2.0 != 0.0 {
        (rounded - value.signum()) as i32
    } else {
        rounded as i32
    }
}

fn estimate_remaining(elapsed: Option<Duration>, progress: f64) -> String {
    let Some(elapsed) = elapsed else {
        return String::new();
    };
    let seconds = elapsed.as_secs_f64();
    if progress < 3.0 || seconds < 2.0 {
        return String::new();
    }
    let remaining = seconds * (100.0 - progress) / progress;
    if remaining < 5.0 {
        return "Quasi terminato...".to_owned();
    }
    if remaining < 60.0 {
        return format!("Circa {:.0} secondi rimanenti", (remaining / 5.0).ceil() * 5.0);
    }
    format!("Circa {:.0} minuti rimanenti", (remaining / 60.0).ceil())
}

fn format_size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{} KB", (bytes / 1024).max(1))
    }
}

unsafe fn fill_rect(hdc: HDC, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let rect = RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    let brush = CreateSolidBrush(COLORREF(color));
    FillRect(hdc, &rect, brush);
    let _ = DeleteObject(brush.into());
}

unsafe fn fill_gradient(hdc: HDC, x: i32, y: i32, w: i32, h: i32, from: u32, to: u32) {
    let vertices = [vertex(x, y, from), vertex(x + w, y + h, to)];
    let mesh = GRADIENT_RECT { UpperLeft: 0, LowerRight: 1 };
    let _ = GradientFill(hdc, &vertices, &mesh as *const GRADIENT_RECT as *const c_void, 1, GRADIENT_FILL_RECT_H);
}

// `gradient` (x, width) lets a partial fill keep the colours of the full bar
#[allow(clippy::too_many_arguments)]
unsafe fn fill_rounded(hdc: HDC, x: i32, y: i32, w: i32, h: i32, radius: i32, from: u32, to: u32, gradient: Option<(i32, i32)>) {
    let region = CreateRoundRectRgn(x, y, x + w + 1, y + h + 1, radius, radius);
    SelectClipRgn(hdc, Some(region));
    if from == to {
        fill_rect(hdc, x, y, w, h, from);
    } else {
        let (gradient_x, gradient_width) = gradient.unwrap_or((x, w));
        fill_gradient(hdc, gradient_x, y, gradient_width, h, from, to);
    }
    SelectClipRgn(hdc, None);
    let _ = DeleteObject(region.into());
}

fn vertex(x: i32, y: i32, color: u32) -> TRIVERTEX {
    TRIVERTEX {
        x,
        y,
        Red: ((color & 0xFF) << 8) as u16,
        Green: (((color >> 8) & 0xFF) << 8) as u16,
        Blue: (((color >> 16) & 0xFF) << 8) as u16,
        Alpha: 0,
    }
}

unsafe fn draw_text(hdc: HDC, text: &str, font: HFONT, color: u32, (left, top, right, bottom): (i32, i32, i32, i32), format: DRAW_TEXT_FORMAT) {
    let old = SelectObject(hdc, font.into());
    SetTextColor(hdc, COLORREF(color));
    let mut rect = RECT { left, top, right, bottom };
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    DrawTextW(hdc, &mut wide, &mut rect, format | DT_NOPREFIX);
    SelectObject(hdc, old);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_match_the_csharp_window() {
        assert_eq!(scale(24, 96), 24);
        assert_eq!(scale(24, 144), 36);
        assert_eq!(scale(3, 120), 4); // 3.75 rounds up like MulDiv
        assert_eq!(round_half_even(2.5), 2);
        assert_eq!(round_half_even(3.5), 4);
        assert_eq!(round_half_even(41.4), 41);
        assert_eq!(format_size(512), "1 KB");
        assert_eq!(format_size(5 * 1024 * 1024 + 300 * 1024), "5.3 MB");
        assert_eq!(estimate_remaining(Some(Duration::from_secs(1)), 50.0), "");
        assert_eq!(estimate_remaining(Some(Duration::from_secs(10)), 50.0), "Circa 10 secondi rimanenti");
        assert_eq!(estimate_remaining(Some(Duration::from_secs(60)), 10.0), "Circa 9 minuti rimanenti");
        assert_eq!(estimate_remaining(Some(Duration::from_secs(10)), 95.0), "Quasi terminato...");
    }

    #[test]
    fn window_can_be_closed_before_it_is_shown() {
        let window = PreLaunchWindow::new("App", "1.0.0", None, Box::new(|| {}));
        window.close();
        window.show("ignored");
        assert!(!window.shared.thread_started.load(Ordering::SeqCst));
    }
}
