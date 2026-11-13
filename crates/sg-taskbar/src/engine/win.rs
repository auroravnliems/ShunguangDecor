//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! Win32 layer: window creation, event queue, AppBar, DWM blur/rounded corners, monitors & DPI.
//! WndProc only pushes events into the queue; App handles them after each dispatch so there are no nested calls.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABM_WINDOWPOSCHANGED, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::*;

/// notification from a background thread: wparam = code, lparam = parameter
pub const WM_NOTIFY_APP: u32 = WM_APP + 2;
pub const WM_APPBAR: u32 = WM_USER + 100;
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Windows belonging to the engine itself (bar, popup, menu, tooltip, notification window, the fake `Shell_TrayWnd` inside
/// this process). Other windows of the app (including the main ShunguangDecor window) count as ordinary applications.
pub fn is_engine_window(class: &str, pid: u32, me: u32) -> bool {
    class.starts_with("TachyonTaskbar") || (class == "Shell_TrayWnd" && pid == me)
}

pub const APP_TITLE: &str = "YasbBar";

#[derive(Debug, Clone, Copy)]
pub enum MouseBtn {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone)]
pub enum WinEvent {
    MouseMove { hwnd: HWND, x: i32, y: i32 },
    MouseLeave { hwnd: HWND },
    Button { hwnd: HWND, x: i32, y: i32, btn: MouseBtn, down: bool },
    /// screen coordinates
    Wheel { hwnd: HWND, x: i32, y: i32, delta: i32 },
    Timer { id: usize },
    Activate { hwnd: HWND, active: bool },
    Key { hwnd: HWND, vk: u32 },
    DisplayChange,
    SettingChange,
    /// `WM_SETTINGCHANGE` with `SPI_SETWORKAREA`: someone (usually Explorer) just changed the work area
    WorkAreaChange,
    AccentChange,
    AppBar { hwnd: HWND, code: u32 },
    Notify { code: usize, param: isize },
    /// mouse pressed somewhere on screen (global hook while a popup is open)
    GlobalPress { x: i32, y: i32 },
    Hook { event: u32, hwnd: HWND, id_object: i32 },
    EndSession,
}

thread_local! {
    static QUEUE: RefCell<VecDeque<WinEvent>> = const { RefCell::new(VecDeque::new()) };
}

static MSG_HWND: AtomicIsize = AtomicIsize::new(0);

pub fn push(e: WinEvent) {
    QUEUE.with(|q| q.borrow_mut().push_back(e));
}

pub fn pop() -> Option<WinEvent> {
    QUEUE.with(|q| q.borrow_mut().pop_front())
}

/// Post a notification from another thread back to the UI thread.
pub fn post_notify(code: usize, param: isize) {
    let h = MSG_HWND.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_NOTIFY_APP, WPARAM(code), LPARAM(param));
        }
    }
}

pub fn msg_hwnd() -> HWND {
    HWND(MSG_HWND.load(Ordering::Relaxed) as *mut _)
}

fn lo(l: LPARAM) -> i32 {
    (l.0 & 0xffff) as i16 as i32
}
fn hi(l: LPARAM) -> i32 {
    ((l.0 >> 16) & 0xffff) as i16 as i32
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            let _ = TrackMouseEvent(&mut tme);
            push(WinEvent::MouseMove { hwnd, x: lo(lp), y: hi(lp) });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            push(WinEvent::MouseLeave { hwnd });
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP | WM_LBUTTONDBLCLK
        | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK => {
            let (btn, down) = match msg {
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => (MouseBtn::Left, true),
                WM_LBUTTONUP => (MouseBtn::Left, false),
                WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => (MouseBtn::Right, true),
                WM_RBUTTONUP => (MouseBtn::Right, false),
                WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => (MouseBtn::Middle, true),
                _ => (MouseBtn::Middle, false),
            };
            if down {
                SetCapture(hwnd);
            } else {
                let _ = ReleaseCapture();
            }
            push(WinEvent::Button { hwnd, x: lo(lp), y: hi(lp), btn, down });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xffff) as i16 as i32;
            push(WinEvent::Wheel { hwnd, x: lo(lp), y: hi(lp), delta });
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => {
            // the bar doesn't steal focus (WS_EX_NOACTIVATE)
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_NOACTIVATE.0 != 0 {
                LRESULT(MA_NOACTIVATE as isize)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_ACTIVATE => {
            push(WinEvent::Activate { hwnd, active: (wp.0 & 0xffff) as u32 != WA_INACTIVE });
            LRESULT(0)
        }
        WM_KEYDOWN => {
            push(WinEvent::Key { hwnd, vk: wp.0 as u32 });
            LRESULT(0)
        }
        WM_TIMER => {
            push(WinEvent::Timer { id: wp.0 });
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            push(WinEvent::DisplayChange);
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            push(if wp.0 == SPI_SETWORKAREA.0 as usize { WinEvent::WorkAreaChange } else { WinEvent::SettingChange });
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DWMCOLORIZATIONCOLORCHANGED => {
            push(WinEvent::AccentChange);
            LRESULT(0)
        }
        WM_APPBAR => {
            push(WinEvent::AppBar { hwnd, code: wp.0 as u32 });
            LRESULT(0)
        }
        WM_NOTIFY_APP => {
            push(WinEvent::Notify { code: wp.0, param: lp.0 });
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            push(WinEvent::EndSession);
            LRESULT(0)
        }
        WM_CLOSE => {
            // taskkill (without /F) sends WM_CLOSE: exit gracefully to unregister the AppBar
            push(WinEvent::EndSession);
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(None, IDC_ARROW).ok());
            LRESULT(1)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn register_classes() {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let icon = HICON::default();
        for (name, style) in [(w!("TachyonTaskbarWnd"), CS_DBLCLKS), (w!("TachyonTaskbarMsg"), CS_HREDRAW)] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                hIcon: icon,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: name,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }
    }
}

/// Hidden window that receives the timer, system tray and notifications from background threads.
pub fn create_message_window() -> Result<HWND, String> {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        // hidden top-level window (not HWND_MESSAGE, so it can receive broadcasts like WM_SETTINGCHANGE)
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("TachyonTaskbarMsg"),
            w!("TachyonTaskbar"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .map_err(|e| format!("không tạo được cửa sổ thông báo: {e}"))?;
        MSG_HWND.store(h.0 as isize, Ordering::Relaxed);
        Ok(h)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Bar { topmost: bool },
    Popup,
    Tooltip,
}

pub fn create_layered(kind: Kind, title: &str) -> HWND {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW;
        match kind {
            Kind::Bar { topmost } => {
                ex |= WS_EX_NOACTIVATE;
                if topmost {
                    ex |= WS_EX_TOPMOST;
                }
            }
            Kind::Popup => ex |= WS_EX_TOPMOST,
            Kind::Tooltip => ex |= WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        }
        let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        CreateWindowExW(
            ex,
            w!("TachyonTaskbarWnd"),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .expect("không tạo được cửa sổ")
    }
}

pub fn destroy(h: HWND) {
    unsafe {
        let _ = DestroyWindow(h);
    }
}

pub fn show_noactivate(h: HWND) {
    unsafe {
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
    }
}

pub fn is_visible(h: HWND) -> bool {
    unsafe { IsWindowVisible(h).as_bool() }
}

pub fn hide(h: HWND) {
    unsafe {
        let _ = ShowWindow(h, SW_HIDE);
    }
}

pub fn activate(h: HWND) {
    unsafe {
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
        let _ = SetFocus(Some(h));
    }
}

pub fn move_window(h: HWND, x: i32, y: i32, w: i32, hgt: i32, topmost: bool) {
    unsafe {
        let after = if topmost { Some(HWND_TOPMOST) } else { None };
        let mut flags = SWP_NOACTIVATE;
        if !topmost {
            flags |= SWP_NOZORDER;
        }
        let _ = SetWindowPos(h, after, x, y, w, hgt, flags);
    }
}

pub fn set_timer(id: usize, ms: u32) {
    unsafe {
        SetTimer(Some(msg_hwnd()), id, ms.max(1), None);
    }
}

pub fn kill_timer(id: usize) {
    unsafe {
        let _ = KillTimer(Some(msg_hwnd()), id);
    }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

// ------------------------------------------------------------------ blur / DWM

#[repr(C)]
struct AccentPolicy {
    state: i32,
    flags: i32,
    gradient: u32,
    anim: i32,
}

#[repr(C)]
struct CompositionData {
    attrib: i32,
    data: *mut std::ffi::c_void,
    size: usize,
}

type SetWca = unsafe extern "system" fn(HWND, *mut CompositionData) -> BOOL;

fn set_accent(h: HWND, state: i32, gradient: u32) {
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return };
        let Some(f) = GetProcAddress(user32, windows::core::s!("SetWindowCompositionAttribute")) else { return };
        let f: SetWca = std::mem::transmute(f);
        let mut acc = AccentPolicy { state, flags: 0, gradient, anim: 0 };
        let mut data = CompositionData { attrib: 19, data: &mut acc as *mut _ as *mut _, size: std::mem::size_of::<AccentPolicy>() };
        let _ = f(h, &mut data);
    }
}

pub fn windows_build() -> u32 {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut buf = [0u16; 32];
        let mut len = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        );
        if r.is_ok() {
            let s = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
            return s.trim().parse().unwrap_or(22000);
        }
    }
    22000
}

fn hex_to_colorref(s: &str) -> Option<u32> {
    let c = crate::engine::css::values::parse_color(s)?;
    let (r, g, b) = ((c.r * 255.0) as u32, (c.g * 255.0) as u32, (c.b * 255.0) as u32);
    Some(r | (g << 8) | (b << 16))
}

/// The acrylic accent's `gradient` color (AABBGGRR): takes the RGB of the bar background with a fixed moderate opacity so
/// the bar's D2D background layer (with its own alpha) still decides the final intensity.
pub fn acrylic_tint(bg: Option<crate::engine::css::Color>) -> u32 {
    let (r, g, b) = match bg {
        Some(c) => ((c.r.clamp(0.0, 1.0) * 255.0).round() as u32, (c.g.clamp(0.0, 1.0) * 255.0).round() as u32, (c.b.clamp(0.0, 1.0) * 255.0).round() as u32),
        None => (0x20, 0x20, 0x20),
    };
    0x66 << 24 | b << 16 | g << 8 | r
}

/// Like YASB's `enable_blur`: Win11 uses ACCENT_ENABLE_BLURBEHIND + DWM rounded corners, Win10 uses acrylic.
/// `acrylic` = true (the `blur_effect.acrylic` option) forces `ACCENT_ENABLE_ACRYLICBLURBEHIND` (state 4) with the
/// `tint` color, keeping DWM rounded corners on Win11.
pub fn enable_blur(h: HWND, dark: bool, round: bool, round_small: bool, border_color: &str, acrylic: Option<u32>) {
    unsafe {
        if let Some(tint) = acrylic {
            set_accent(h, 4, tint);
            if windows_build() >= 22000 {
                if dark {
                    let v: i32 = 1;
                    let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const _ as *const _, 4);
                }
                if round {
                    set_corners(h, if round_small { DWMWCP_ROUNDSMALL } else { DWMWCP_ROUND }, border_color);
                }
            }
        } else if windows_build() >= 22000 {
            set_accent(h, 3, 0x0120_2020);
            if dark {
                let v: i32 = 1;
                let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const _ as *const _, 4);
            }
            if round {
                set_corners(h, if round_small { DWMWCP_ROUNDSMALL } else { DWMWCP_ROUND }, border_color);
            }
        } else {
            set_accent(h, 4, 0x0120_2020);
        }
    }
}

pub fn set_corners(h: HWND, pref: DWM_WINDOW_CORNER_PREFERENCE, border_color: &str) {
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const _, 4);
        let color: u32 = if border_color.eq_ignore_ascii_case("none") {
            DWMWA_COLOR_NONE
        } else if border_color.eq_ignore_ascii_case("system") {
            DWMWA_COLOR_DEFAULT
        } else {
            hex_to_colorref(border_color).unwrap_or(DWMWA_COLOR_DEFAULT)
        };
        let _ = DwmSetWindowAttribute(h, DWMWA_BORDER_COLOR, &color as *const _ as *const _, 4);
    }
}

/// System accent color from `AccentPalette` (8 shades, like palette.accent() of Qt windows11:
/// dark backgrounds use Light2, light backgrounds use Dark1).
pub fn accent_color(dark: bool) -> Option<crate::engine::css::Color> {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut buf = [0u8; 32];
        let mut len = 32u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        );
        if r.is_err() || len < 32 {
            return None;
        }
        let i = if dark { 1 } else { 4 } * 4;
        Some(crate::engine::css::Color::rgba8(buf[i], buf[i + 1], buf[i + 2], 255))
    }
}

pub fn apps_use_dark() -> bool {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut v: u32 = 0;
        let mut len = 4u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut _ as *mut _),
            Some(&mut len),
        );
        r.is_err() || v == 0
    }
}

// ------------------------------------------------------------------ AppBar
// Every command goes through `appbar::message` rather than calling `SHAppBarMessage` directly: when the engine captures tray icons, the
// fake `Shell_TrayWnd` window of this very process sits in front of the real taskbar and loses the QUERYPOS/SETPOS results (see appbar.rs).

fn appbar_data(h: HWND) -> APPBARDATA {
    APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: h, ..Default::default() }
}

/// Register the AppBar. Returns `true` if Explorer accepted (ABM_NEW returned TRUE); `false` if already registered or rejected.
pub fn appbar_register(h: HWND) -> bool {
    let mut d = APPBARDATA { uCallbackMessage: WM_APPBAR, ..appbar_data(h) };
    crate::engine::appbar::message(ABM_NEW, &mut d) != 0
}

/// Re-register reliably: try ABM_NEW first; if Explorer returns FALSE (it considers us already registered, possibly with
/// stale state), do ABM_REMOVE and then ABM_NEW again. Returns `true` if registration finally succeeded.
pub fn appbar_reregister(h: HWND) -> bool {
    if appbar_register(h) {
        return true;
    }
    appbar_remove(h);
    appbar_register(h)
}

/// Tell Explorer the AppBar window was just moved/resized (`ABM_WINDOWPOSCHANGED`).
pub fn appbar_window_pos_changed(h: HWND) {
    crate::engine::appbar::message(ABM_WINDOWPOSCHANGED, &mut appbar_data(h));
}

/// Work area (`rcWork`) of the monitor containing rectangle `mon`.
pub fn work_area_of(mon: RECT) -> Option<RECT> {
    unsafe {
        let hm = MonitorFromRect(&mon, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(hm, &mut mi).as_bool().then_some(mi.rcWork)
    }
}

/// Reserve space on the screen (windows_app_bar). Returns the rectangle the system accepted.
pub fn appbar_set_pos(h: HWND, top: bool, mon: RECT, height_px: i32) -> RECT {
    let mut d = appbar_data(h);
    d.uEdge = if top { ABE_TOP } else { ABE_BOTTOM };
    d.rc = mon;
    if top {
        d.rc.bottom = mon.top + height_px;
    } else {
        d.rc.top = mon.bottom - height_px;
    }
    crate::engine::appbar::message(ABM_QUERYPOS, &mut d);
    if top {
        d.rc.bottom = d.rc.top + height_px;
    } else {
        d.rc.top = d.rc.bottom - height_px;
    }
    crate::engine::appbar::message(ABM_SETPOS, &mut d);
    d.rc
}

pub fn appbar_remove(h: HWND) {
    crate::engine::appbar::message(ABM_REMOVE, &mut appbar_data(h));
}

// ------------------------------------------------------------------ monitors

#[derive(Clone, Debug)]
pub struct Monitor {
    pub handle: isize,
    pub device: String,
    pub rect: RECT,
    pub scale: f32,
    pub primary: bool,
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn cb(hm: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let v = &mut *(lp.0 as *mut Vec<Monitor>);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hm, &mut mi.monitorInfo as *mut _).as_bool() {
            let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            v.push(Monitor {
                handle: hm.0 as isize,
                device: String::from_utf16_lossy(&mi.szDevice[..len]),
                rect: mi.monitorInfo.rcMonitor,
                scale: dx as f32 / 96.0,
                primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        TRUE
    }
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
    v
}

pub fn monitor_of_window(h: HWND) -> isize {
    unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST).0 as isize }
}

fn to_rect(r: RECT) -> crate::reveal::Rect {
    crate::reveal::Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

/// Measurements of the foreground window together with the monitor containing it: READ-only (doesn't touch the owning process). `None` when there is none.
pub fn foreground_window() -> Option<(isize, crate::reveal::ForegroundWindow)> {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(h, &mut r).ok()?;
        let hm = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(hm, &mut mi).as_bool() {
            return None;
        }
        let class = crate::engine::sys::process::class_name(h);
        let own = is_engine_window(&class, crate::engine::sys::process::window_pid(h), std::process::id());
        let style = GetWindowLongW(h, GWL_STYLE) as u32;
        let has_caption = style & WS_CAPTION.0 == WS_CAPTION.0;
        let zoomed = IsZoomed(h).as_bool();
        Some((hm.0 as isize, crate::reveal::ForegroundWindow { rect: to_rect(r), monitor: to_rect(mi.rcMonitor), class, has_caption, own, zoomed }))
    }
}

/// Is the foreground window a shell flyout/menu (Start, Search, hidden tray, calendar...)? Read-only.
pub fn foreground_is_shell_flyout() -> bool {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            return false;
        }
        let class = crate::engine::sys::process::class_name(h);
        let exe = crate::engine::sys::process::exe_path(crate::engine::sys::process::window_pid(h)).unwrap_or_default();
        crate::reveal::is_shell_flyout(&class, &exe)
    }
}

/// Simplified `SHQueryUserNotificationState` state (any other value or an error → `None`).
pub fn notification_state() -> crate::reveal::NotifyState {
    use crate::reveal::NotifyState;
    use windows::Win32::UI::Shell::{QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
    match unsafe { SHQueryUserNotificationState() } {
        Ok(s) if s == QUNS_BUSY => NotifyState::Busy,
        Ok(s) if s == QUNS_RUNNING_D3D_FULL_SCREEN => NotifyState::D3dFullscreen,
        Ok(s) if s == QUNS_PRESENTATION_MODE => NotifyState::Presentation,
        _ => NotifyState::None,
    }
}

// ------------------------------------------------------------------ global mouse hook (closes popups on outside click)

static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn ll_mouse(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let m = wp.0 as u32;
        if matches!(m, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN) {
            let info = &*(lp.0 as *const MSLLHOOKSTRUCT);
            push(WinEvent::GlobalPress { x: info.pt.x, y: info.pt.y });
            // wake the main loop to handle it immediately
            let _ = PostMessageW(Some(msg_hwnd()), WM_NULL, WPARAM(0), LPARAM(0));
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

pub fn set_mouse_hook(on: bool) {
    unsafe {
        let cur = MOUSE_HOOK.load(Ordering::Relaxed);
        if on && cur == 0 {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            if let Ok(h) = SetWindowsHookExW(WH_MOUSE_LL, Some(ll_mouse), Some(hinst.into()), 0) {
                MOUSE_HOOK.store(h.0 as isize, Ordering::Relaxed);
            }
        } else if !on && cur != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(cur as *mut _));
            MOUSE_HOOK.store(0, Ordering::Relaxed);
        }
    }
}

// ------------------------------------------------------------------ WinEvent hook (active window)

unsafe extern "system" fn win_event_proc(
    _: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // the engine lives inside the app process: only skip the engine's own windows; app windows are still tracked
    let class = crate::engine::sys::process::class_name(hwnd);
    let pid = crate::engine::sys::process::window_pid(hwnd);
    if is_engine_window(&class, pid, std::process::id()) {
        return;
    }
    push(WinEvent::Hook { event, hwnd, id_object });
}

thread_local! {
    static EVENT_HOOKS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
}

pub fn install_win_event_hooks() {
    use windows::Win32::UI::Accessibility::SetWinEventHook;
    unsafe {
        for (a, b) in [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
            (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
        ] {
            let h = SetWinEventHook(a, b, None, Some(win_event_proc), 0, 0, WINEVENT_OUTOFCONTEXT);
            if !h.is_invalid() {
                EVENT_HOOKS.with(|v| v.borrow_mut().push(h.0 as isize));
            }
        }
    }
}

/// Tear down everything tied to the engine thread: WinEvent hooks, global mouse hook, notification window, queue.
/// Call once when the thread exits so the next `spawn` starts from scratch within the same process.
pub fn cleanup() {
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, UnhookWinEvent};
    set_mouse_hook(false);
    EVENT_HOOKS.with(|v| {
        for h in v.borrow_mut().drain(..) {
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(h as *mut _));
            }
        }
    });
    let h = MSG_HWND.swap(0, Ordering::SeqCst);
    if h != 0 {
        destroy(HWND(h as *mut _));
    }
    QUEUE.with(|q| q.borrow_mut().clear());
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        for name in [w!("TachyonTaskbarWnd"), w!("TachyonTaskbarMsg")] {
            let _ = UnregisterClassW(name, Some(hinst.into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{acrylic_tint, is_engine_window};
    use crate::engine::css::Color;

    #[test]
    fn acrylic_tint_uses_bar_background() {
        assert_eq!(acrylic_tint(Some(Color::rgba8(0x10, 0x20, 0x30, 255))), 0x6630_2010);
        assert_eq!(acrylic_tint(None), 0x6620_2020);
    }

    #[test]
    fn engine_window_filter() {
        let me = 10;
        assert!(is_engine_window("TachyonTaskbarWnd", me, me));
        assert!(is_engine_window("TachyonTaskbarMsg", me, me));
        assert!(is_engine_window("Shell_TrayWnd", me, me));
        // Explorer's real taskbar (different pid) and the app's main window don't belong to the engine
        assert!(!is_engine_window("Shell_TrayWnd", 99, me));
        assert!(!is_engine_window("Tauri Window", me, me));
        assert!(!is_engine_window("Chrome_WidgetWin_1", me, me));
    }
}
