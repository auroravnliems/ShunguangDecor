//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Captures system tray icons like YASB's `SystrayMonitor` (legacy mode): a dedicated thread creates an always-topmost
//! window of class `Shell_TrayWnd`, so every application's `Shell_NotifyIcon` sends its `WM_COPYDATA` to us
//! first. We read the icon data, then forward it unchanged to Explorer's real tray, so the Windows tray
//! keeps working normally. When turned off, the window is destroyed and everything goes straight to Explorer again.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use windows::core::{w, GUID, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::engine::app::N_SYSTRAY;
use crate::engine::win::post_notify;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETVERSION: u32 = 4;
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_STATE: u32 = 0x08;
const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 0x01;
const NIN_SELECT: u32 = WM_USER;
/// NIN_CONTEXTMENU = WM_CONTEXTMENU
const NIN_CONTEXTMENU: u32 = 0x007B;

/// The 32-bit NOTIFYICONDATA the shell sends in WM_COPYDATA (handles are always 32-bit).
#[repr(C)]
#[derive(Clone, Copy)]
struct Nid32 {
    cb_size: u32,
    hwnd: u32,
    uid: u32,
    flags: u32,
    callback: u32,
    hicon: u32,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: GUID,
    balloon_icon: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ShellTrayData {
    magic: u32,
    message: u32,
    nid: Nid32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IconIdentifier {
    magic: u32,
    message: u32,
    callback_size: u32,
    padding: u32,
    hwnd: u32,
    uid: u32,
    guid: GUID,
}

/// One tray icon. The image is premultiplied-alpha BGRA (size `icon_size`); `rev` increases each time the image changes.
#[derive(Clone, Debug)]
pub struct TrayIcon {
    pub hwnd: isize,
    pub uid: u32,
    pub guid: Option<GUID>,
    pub callback: u32,
    pub version: u32,
    pub tip: String,
    pub exe: String,
    pub hidden: bool,
    pub image: Option<Arc<(u32, u32, Vec<u8>)>>,
    pub rev: u64,
}

impl TrayIcon {
    pub fn key(&self) -> String {
        match self.guid {
            Some(g) => format!("{g:?}"),
            None => format!("{}:{}", self.hwnd, self.uid),
        }
    }
    pub fn exe_name(&self) -> String {
        self.exe.rsplit(['\\', '/']).next().unwrap_or("").to_string()
    }
}

static ICONS: Mutex<Vec<TrayIcon>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);
static HWND_SELF: AtomicIsize = AtomicIsize::new(0);
static REAL_TRAY: AtomicIsize = AtomicIsize::new(0);
static GEN: AtomicU64 = AtomicU64::new(1);
/// icon image size (real px) – follows the largest DPI among the monitors
static ICON_PX: AtomicU64 = AtomicU64::new(32);

pub fn snapshot() -> Vec<TrayIcon> {
    ICONS.lock().map(|v| v.clone()).unwrap_or_default()
}

static THREAD: Mutex<Option<(std::thread::JoinHandle<()>, std::sync::mpsc::Receiver<()>)>> = Mutex::new(None);

pub fn ensure_started(needed: bool, icon_px: u32) {
    if !needed || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    ICON_PX.store(icon_px.clamp(16, 64) as u64, Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        run(tx);
        let _ = done_tx.send(());
    });
    // wait until the fake window is created so a `stop()` right afterwards can still find it
    let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
    if let Ok(mut t) = THREAD.lock() {
        *t = Some((handle, done_rx));
    }
}

/// Stops the tray-capture thread, destroys the fake `Shell_TrayWnd` window and clears the state so the next start begins from scratch.
pub fn stop() {
    let h = HWND_SELF.swap(0, Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    let handle = THREAD.lock().ok().and_then(|mut t| t.take());
    if let Some((handle, done)) = handle {
        // bounded wait so the bar's shutdown does not exceed 3 seconds; past the deadline, abandon the thread and let it keep running
        match done.recv_timeout(std::time::Duration::from_millis(1200)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = handle.join();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                crate::engine::log::warn("Systray: luồng bắt khay chưa thoát sau 1,2 giây, bỏ qua không chờ");
            }
        }
    }
    STARTED.store(false, Ordering::SeqCst);
    REAL_TRAY.store(0, Ordering::SeqCst);
    if let Ok(mut v) = ICONS.lock() {
        v.clear();
    }
}

fn run(ready: std::sync::mpsc::Sender<()>) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW { lpfnWndProc: Some(tray_proc), hInstance: hinst.into(), lpszClassName: w!("Shell_TrayWnd"), ..Default::default() };
        RegisterClassW(&wc);
        let Ok(h) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("Shell_TrayWnd"),
            PCWSTR::null(),
            WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            crate::engine::log::warn("Systray: không tạo được cửa sổ Shell_TrayWnd");
            let _ = UnregisterClassW(w!("Shell_TrayWnd"), Some(hinst.into()));
            STARTED.store(false, Ordering::SeqCst);
            let _ = ready.send(());
            return;
        };
        HWND_SELF.store(h.0 as isize, Ordering::SeqCst);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        // stay topmost so we are always found before the real tray
        SetTimer(Some(h), 1, 100, None);
        // ask every application to re-add its icon (like YASB's "Refresh Systray" button)
        let created = RegisterWindowMessageW(w!("TaskbarCreated"));
        let _ = SendNotifyMessageW(HWND_BROADCAST, created, WPARAM(0), LPARAM(0));
        crate::engine::log::info("Systray: đang nhận icon khay hệ thống");
        let _ = ready.send(());
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // the window has been destroyed (WM_CLOSE → WM_DESTROY → PostQuitMessage): unregister the class so the next registration is clean
        let _ = UnregisterClassW(w!("Shell_TrayWnd"), Some(hinst.into()));
    }
}

fn find_real_tray() -> HWND {
    let me = HWND_SELF.load(Ordering::SeqCst);
    let cached = REAL_TRAY.load(Ordering::SeqCst);
    unsafe {
        if cached != 0 && IsWindow(Some(HWND(cached as *mut _))).as_bool() {
            return HWND(cached as *mut _);
        }
        let mut cur: Option<HWND> = None;
        loop {
            let Ok(h) = FindWindowExW(None, cur, w!("Shell_TrayWnd"), PCWSTR::null()) else { break };
            if h.0.is_null() {
                break;
            }
            cur = Some(h);
            if h.0 as isize == me {
                continue;
            }
            let pid = crate::engine::sys::process::window_pid(h);
            let exe = crate::engine::sys::process::exe_path(pid).unwrap_or_default().to_ascii_lowercase();
            if exe.ends_with("\\explorer.exe") {
                REAL_TRAY.store(h.0 as isize, Ordering::SeqCst);
                return h;
            }
        }
    }
    HWND::default()
}

unsafe fn forward(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let real = find_real_tray();
    if real.0.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_USER + 372 {
        let _ = PostMessageW(Some(real), msg, wp, lp);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    SendMessageW(real, msg, Some(wp), Some(lp))
}

fn wstr(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

fn pack(lo: i32, hi: i32) -> isize {
    ((lo as u32 & 0xffff) | ((hi as i16 as i32 as u32) << 16)) as i32 as isize
}

fn icon_image(h: u32) -> Option<Arc<(u32, u32, Vec<u8>)>> {
    if h == 0 {
        return None;
    }
    let px = ICON_PX.load(Ordering::Relaxed) as u32;
    let p = crate::engine::sys::image::from_hicon(HICON(h as usize as *mut _), px)?;
    Some(Arc::new((p.w, p.h, p.data.clone())))
}

fn on_icon(kind: u32, n: &Nid32) {
    let hwnd = n.hwnd as i32 as isize;
    let guid = (n.flags & NIF_GUID != 0).then_some(n.guid);
    let Ok(mut list) = ICONS.lock() else { return };
    let pos = list.iter().position(|i| match (guid, i.guid) {
        (Some(g), Some(ig)) => g == ig,
        _ => i.hwnd == hwnd && i.uid == n.uid,
    });
    if kind == NIM_DELETE {
        if let Some(p) = pos {
            list.remove(p);
        }
        return;
    }
    let idx = match pos {
        Some(p) => p,
        None => {
            if kind == NIM_SETVERSION {
                return;
            }
            let pid = crate::engine::sys::process::window_pid(HWND(hwnd as *mut _));
            list.push(TrayIcon {
                hwnd,
                uid: n.uid,
                guid,
                callback: 0,
                version: 0,
                tip: String::new(),
                exe: crate::engine::sys::process::exe_path(pid).unwrap_or_default(),
                hidden: false,
                image: None,
                rev: 0,
            });
            list.len() - 1
        }
    };
    let it = &mut list[idx];
    // same hwnd/uid but the app changed windows (restarted) → update
    it.hwnd = hwnd;
    if (kind == NIM_SETVERSION || (1..=4).contains(&n.version))
        && (1..=4).contains(&n.version) {
            it.version = n.version;
        }
    if n.flags & NIF_MESSAGE != 0 {
        it.callback = n.callback;
    }
    if n.flags & NIF_TIP != 0 {
        it.tip = wstr(&n.tip);
    }
    if n.flags & NIF_STATE != 0 && n.state_mask & NIS_HIDDEN != 0 {
        it.hidden = n.state & NIS_HIDDEN != 0;
    }
    if n.flags & NIF_ICON != 0 {
        it.image = icon_image(n.hicon);
        it.rev = GEN.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        WM_TIMER => {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            return LRESULT(0);
        }
        WM_COPYDATA => {
            let cds = &*(lp.0 as *const COPYDATASTRUCT);
            if cds.dwData == 1 && cds.cbData as usize >= 8 + 24 && !cds.lpData.is_null() {
                // copy the part that is actually there (older apps send a shorter struct)
                let mut d: ShellTrayData = std::mem::zeroed();
                let n = (cds.cbData as usize).min(std::mem::size_of::<ShellTrayData>());
                std::ptr::copy_nonoverlapping(cds.lpData as *const u8, &mut d as *mut _ as *mut u8, n);
                let result = forward(hwnd, msg, wp, lp);
                if matches!(d.message, NIM_ADD | NIM_MODIFY | NIM_SETVERSION | NIM_DELETE) {
                    on_icon(d.message, &d.nid);
                    post_notify(N_SYSTRAY, 0);
                }
                return result;
            }
            if cds.dwData == 3 && !cds.lpData.is_null() && cds.cbData as usize >= std::mem::size_of::<IconIdentifier>() - 16 {
                // Shell_NotifyIconGetRect: returns the icon's position (where it was just clicked)
                let id = &*(cds.lpData as *const IconIdentifier);
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                return LRESULT(match id.message {
                    1 => pack(p.x, p.y + 1),
                    2 => pack(p.x + 1, p.y - 1),
                    _ => 0,
                });
            }
            return forward(hwnd, msg, wp, lp);
        }
        _ => {}
    }
    let created = RegisterWindowMessageW(w!("TaskbarCreated"));
    if msg == created {
        let _ = SetPropW(hwnd, w!("TaskbandHWND"), Some(HANDLE(hwnd.0)));
        return LRESULT(0);
    }
    if msg == WM_ACTIVATEAPP || msg == WM_COMMAND || msg >= WM_USER {
        return forward(hwnd, msg, wp, lp);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

// ------------------------------------------------------------------ sending mouse input to the app (UI thread)

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    Left,
    Right,
    Middle,
}

fn notify(icon: &TrayIcon, message: u32) {
    if icon.callback == 0 {
        return;
    }
    let (wp, lp) = if icon.version > 3 {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        (pack(p.x, p.y) as usize, pack(message as i32, icon.uid as i32))
    } else {
        (icon.uid as usize, pack(message as i32, 0))
    };
    unsafe {
        let _ = SendNotifyMessageW(HWND(icon.hwnd as *mut _), icon.callback, WPARAM(wp), LPARAM(lp));
    }
}

/// Clicks the icon like the real tray: DOWN then UP, plus NIN_SELECT/NIN_CONTEXTMENU for apps using uVersion ≥ 3.
pub fn click(icon: &TrayIcon, btn: Click) {
    let h = HWND(icon.hwnd as *mut _);
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return;
        }
        let pid = crate::engine::sys::process::window_pid(h);
        let _ = AllowSetForegroundWindow(pid);
    }
    match btn {
        Click::Left => {
            notify(icon, WM_LBUTTONDOWN);
            notify(icon, WM_LBUTTONUP);
            if icon.version >= 3 {
                notify(icon, NIN_SELECT);
            }
        }
        Click::Right => {
            notify(icon, WM_RBUTTONDOWN);
            notify(icon, WM_RBUTTONUP);
            if icon.version >= 3 {
                notify(icon, NIN_CONTEXTMENU);
            }
        }
        Click::Middle => {
            notify(icon, WM_MBUTTONDOWN);
            notify(icon, WM_MBUTTONUP);
        }
    }
}

/// Hovers the mouse over the icon (some apps update their tooltip/image when they receive WM_MOUSEMOVE).
#[allow(dead_code)]
pub fn hover(icon: &TrayIcon) {
    notify(icon, WM_MOUSEMOVE);
}

/// Is the icon still alive (if the app exited without NIM_DELETE, it gets cleaned up).
pub fn prune() -> bool {
    let Ok(mut list) = ICONS.lock() else { return false };
    let before = list.len();
    list.retain(|i| unsafe { IsWindow(Some(HWND(i.hwnd as *mut _))).as_bool() });
    before != list.len()
}
