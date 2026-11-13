//! Watches for Explorer restarts: a hidden top-level window (to receive the `TaskbarCreated` broadcast) + every 5 s a check
//! of the desktop's DefView/PID. On change → call `on_change` on a separate thread (reloading the DLL can take a few seconds).
//!
//! `TaskbarCreated` alone is NOT enough to conclude anything: any app can broadcast it (the app's own taskbar
//! tray does exactly that every time it is enabled, YASB-style). Only treat it as an Explorer restart
//! when Explorer's "identity" changes: the desktop's DefView/PID or the real taskbar windows (`native_taskbars`).
//! Simulated messages from the debug command take their own path (the `FORCE` flag) so they still work.

use sg_core::explorer_identity::{ExplorerIdentity, IdentityTracker};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowThreadProcessId, MSG,
    MSGFLT_ALLOW, PostMessageW, RegisterClassW, RegisterWindowMessageW, SetTimer, TranslateMessage, WM_TIMER,
    WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

const CHECK_MS: u32 = 5000;

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
type OnChange = Arc<dyn Fn() + Send + Sync>;
static CALLBACK: Mutex<Option<OnChange>> = Mutex::new(None);
static LAST: Mutex<IdentityTracker> = Mutex::new(IdentityTracker::new());
/// `TaskbarCreated` message simulated by the debug command: always counted as an Explorer restart.
static FORCE: AtomicBool = AtomicBool::new(false);

fn current_identity() -> ExplorerIdentity {
    ExplorerIdentity::measure(desktop_identity())
}

fn desktop_identity() -> Option<(isize, u32)> {
    let dv = sg_desktop::DesktopLayer::locate()?.icon_view()?;
    let mut pid = 0u32;
    // SAFETY: dv is a live window.
    unsafe { GetWindowThreadProcessId(dv, Some(&mut pid)) };
    Some((dv.0 as isize, pid))
}

fn fire(reason: &str) {
    let cb = CALLBACK.lock().ok().and_then(|c| c.clone());
    if let Some(cb) = cb {
        tracing::info!(target: "sg::explorer", "explorer change detected ({reason})");
        let _ = std::thread::Builder::new().name("sg-explorer-reload".into()).spawn(move || cb());
    }
}

/// Re-measure, update the baseline, return `true` if the identity changed compared to the previous baseline.
fn refresh_identity() -> bool {
    let now = current_identity();
    LAST.lock().unwrap_or_else(|e| e.into_inner()).observe(now)
}

fn check() {
    if refresh_identity() {
        fire("desktop window changed");
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tc = TASKBAR_CREATED.load(Ordering::Relaxed);
    let r = std::panic::catch_unwind(|| {
        if tc != 0 && msg == tc {
            // Update the baseline so the 5 s check doesn't report it again.
            let changed = refresh_identity();
            if FORCE.swap(false, Ordering::SeqCst) {
                fire("TaskbarCreated (simulated)");
            } else if changed {
                fire("TaskbarCreated");
            } else {
                tracing::debug!(target: "sg::explorer", "TaskbarCreated ignored: the Explorer taskbar/desktop did not change (another app broadcast it)");
            }
            return Some(LRESULT(0));
        }
        if msg == WM_TIMER {
            check();
            return Some(LRESULT(0));
        }
        None
    });
    match r {
        Ok(Some(v)) => v,
        // SAFETY: forwards the parameters unchanged.
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

/// Run the watcher thread (once per app lifetime).
pub fn spawn(on_change: impl Fn() + Send + Sync + 'static) {
    if let Ok(mut c) = CALLBACK.lock() {
        *c = Some(Arc::new(on_change));
    }
    if WINDOW.load(Ordering::SeqCst) != 0 {
        return;
    }
    let _ = std::thread::Builder::new().name("sg-explorer-watch".into()).spawn(|| {
        // SAFETY: the window class + hidden window belong to this thread alone; the message loop runs until the app exits.
        unsafe {
            let tc = RegisterWindowMessageW(w!("TaskbarCreated"));
            TASKBAR_CREATED.store(tc, Ordering::Relaxed);
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: w!("ShunguangDecorExplorerWatch"), ..Default::default() };
            RegisterClassW(&wc);
            let Ok(h) = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("ShunguangDecorExplorerWatch"),
                w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(inst.into()),
                None,
            ) else {
                tracing::warn!(target: "sg::explorer", "cannot create the explorer watch window");
                return;
            };
            // The app running as admin can still receive messages from Explorer (normal privileges).
            let _ = ChangeWindowMessageFilterEx(h, tc, MSGFLT_ALLOW, None);
            WINDOW.store(h.0 as isize, Ordering::SeqCst);
            if let Ok(mut l) = LAST.lock() {
                l.seed(current_identity());
            }
            SetTimer(Some(h), 1, CHECK_MS, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

/// Simulate `TaskbarCreated` (posted only to our own watcher window, not broadcast).
pub fn simulate_taskbar_created() {
    let h = WINDOW.load(Ordering::SeqCst);
    let tc = TASKBAR_CREATED.load(Ordering::Relaxed);
    if h != 0 && tc != 0 {
        FORCE.store(true, Ordering::SeqCst);
        // SAFETY: our own window.
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut core::ffi::c_void)), tc, WPARAM(0), LPARAM(0));
        }
    }
}
