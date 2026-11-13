//! The DLL's entry point inside Explorer and the state of the desktop thread (the thread that owns `SHELLDLL_DefView`).
//!
//! How it is loaded (like TranslucentTB, but without writing anything into Explorer's memory):
//! 1. The app calls `SetWindowsHookExW(WH_CALLWNDPROC, sg_hook_proc, <this DLL>, <desktop thread>)` and then
//!    `SendMessageTimeoutW(DefView, <private message>, CMD_INIT)`. Windows loads the DLL into Explorer itself and calls
//!    [`sg_hook_proc`] **on the desktop thread**.
//! 2. First run: verify this is `explorer.exe` + the right `SHELLDLL_DefView`, pin the DLL (`GET_MODULE_HANDLE_EX_FLAG_PIN`,
//!    so the DLL stays loaded after the app removes the hook), start the pipe thread. The app removes the hook as soon as the DLL connects the pipe.
//! 3. Every UI change runs on the desktop thread: the pipe thread sets the desired paint table ([`DESIRED`]) and then pushes
//!    the private message `CMD_SYNC` to DefView (`PostMessage` if the subclass is attached; otherwise it temporarily installs a local
//!    hook inside Explorer itself and uses `SendMessageTimeout`). [`sync`] on the desktop thread attaches/detaches the subclass to match.
//!
//! There is no `DllMain` (CRT default is used): nothing runs when the DLL is loaded/unloaded.
//! The subclass is attached only while a paint table exists; off / pipe lost / error → remove subclass + repaint = exactly Windows defaults.

use crate::guard::guarded;
use crate::{labels_paint, log};
use sg_explorer_proto::{CMD_INIT, CMD_SYNC, LabelsPaint, WINDOW_MESSAGE};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, GetProcAddress,
};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId, SetEvent};
use windows::Win32::UI::Shell::SUBCLASSPROC;
use windows::Win32::UI::WindowsAndMessaging::{
    CWPSTRUCT, CallNextHookEx, FindWindowExW, GWLP_WNDPROC, GetClassNameW, GetWindowLongPtrW,
    GetWindowThreadProcessId, IsWindow, PostMessageW, RegisterWindowMessageW, SMTO_ABORTIFHUNG, SMTO_ERRORONEXIT,
    SendMessageTimeoutW, SetWindowsHookExW, UnhookWindowsHookEx, WH_CALLWNDPROC,
};
use windows::core::{BOOL, PCSTR, PCWSTR, s, w};

/// Our subclass ID on DefView (any number that differs from other subclasses with the same function; the function is private to the DLL).
pub const SUBCLASS_ID: usize = 0x5347_4C42; // "SGLB"

static SG_MSG: AtomicU32 = AtomicU32::new(0);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
pub(crate) static DEFVIEW: AtomicIsize = AtomicIsize::new(0);
pub(crate) static LISTVIEW: AtomicIsize = AtomicIsize::new(0);
static DESKTOP_TID: AtomicU32 = AtomicU32::new(0);
static SELF_MODULE: AtomicIsize = AtomicIsize::new(0);
/// Whether the subclass is attached (written only by the desktop thread).
pub(crate) static INSTALLED: AtomicBool = AtomicBool::new(false);
/// A severe error occurred: never attach the subclass again (until Explorer restarts).
pub(crate) static RESTING: AtomicBool = AtomicBool::new(false);
static FAULT: Mutex<Option<String>> = Mutex::new(None);
/// Desired paint table (written by the pipe thread, read by the desktop thread; the lock is held only while swapping the `Arc` pointer).
static DESIRED: Mutex<Option<Arc<LabelsPaint>>> = Mutex::new(None);
pub(crate) static LAYOUT_DIRTY: AtomicBool = AtomicBool::new(false);
/// Event that wakes the pipe thread (created by the pipe thread; the desktop thread only calls `SetEvent`).
pub(crate) static WAKE_EVENT: AtomicIsize = AtomicIsize::new(0);

type SetSubclassFn = unsafe extern "system" fn(HWND, SUBCLASSPROC, usize, usize) -> BOOL;
type RemoveSubclassFn = unsafe extern "system" fn(HWND, SUBCLASSPROC, usize) -> BOOL;
type GetSubclassFn = unsafe extern "system" fn(HWND, SUBCLASSPROC, usize, *mut usize) -> BOOL;
type DefSubclassFn = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// The four subclass functions from the exact comctl32 build DefView is using (Explorer uses comctl32 v6).
pub(crate) struct Comctl {
    pub set: SetSubclassFn,
    pub remove: RemoveSubclassFn,
    pub get: GetSubclassFn,
    pub def: DefSubclassFn,
}

pub(crate) static COMCTL: OnceLock<Comctl> = OnceLock::new();

/// The private message `ShunguangDecor.Explorer` (same value across the whole Windows session).
pub fn sg_msg() -> u32 {
    let m = SG_MSG.load(Ordering::Relaxed);
    if m != 0 {
        return m;
    }
    let wide: Vec<u16> = WINDOW_MESSAGE.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the string is null-terminated and lives for the whole call.
    let m = unsafe { RegisterWindowMessageW(PCWSTR(wide.as_ptr())) };
    SG_MSG.store(m, Ordering::Relaxed);
    m
}

pub(crate) fn hwnd(v: isize) -> HWND {
    HWND(v as *mut core::ffi::c_void)
}

pub(crate) fn is_window(h: HWND) -> bool {
    // SAFETY: IsWindow accepts any value.
    !h.is_invalid() && unsafe { IsWindow(Some(h)).as_bool() }
}

pub(crate) fn class_is(h: HWND, name: &str) -> bool {
    let mut buf = [0u16; 64];
    // SAFETY: the local buffer is valid.
    let n = unsafe { GetClassNameW(h, &mut buf) };
    n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == name
}

/// The only hook function the DLL exports. Called on Explorer's desktop thread; in every case it forwards to `CallNextHookEx`.
///
/// # Safety
/// Only Windows calls this function (`WH_CALLWNDPROC` hook): `lparam` points to a valid `CWPSTRUCT` when `code >= 0`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn sg_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && lparam.0 != 0 {
        guarded("hook", fault_here, || {
            // SAFETY: per the WH_CALLWNDPROC contract, lparam points to a CWPSTRUCT that lives for the whole call.
            let cwp = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
            on_hook(cwp);
        });
    }
    // SAFETY: the arguments are forwarded unchanged to the next hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn on_hook(cwp: &CWPSTRUCT) {
    let m = sg_msg();
    if m == 0 || cwp.message != m {
        return;
    }
    let cmd = cwp.wParam.0;
    if cmd != CMD_INIT && cmd != CMD_SYNC {
        return;
    }
    if !class_is(cwp.hwnd, "SHELLDLL_DefView") {
        return;
    }
    if !INITIALIZED.load(Ordering::SeqCst) && !init(cwp.hwnd) {
        return;
    }
    if DEFVIEW.load(Ordering::SeqCst) != cwp.hwnd.0 as isize && !INSTALLED.load(Ordering::SeqCst) {
        // A new DefView (Explorer rebuilt the desktop without restarting).
        DEFVIEW.store(cwp.hwnd.0 as isize, Ordering::SeqCst);
        LISTVIEW.store(0, Ordering::SeqCst);
    }
    sync();
}

fn process_is_explorer() -> bool {
    let mut buf = [0u16; 1024];
    // SAFETY: None = the exe of the current process; local buffer.
    let n = unsafe { GetModuleFileNameW(None, &mut buf) } as usize;
    let path = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    path.rsplit(['\\', '/']).next().is_some_and(|f| f.eq_ignore_ascii_case("explorer.exe"))
}

/// The DLL's first run on the desktop thread. Returns `false` (does nothing at all) if anything is wrong.
fn init(defview: HWND) -> bool {
    if !process_is_explorer() {
        log::push("hook called outside explorer.exe; ignoring");
        return false;
    }
    let Some(cc) = resolve_comctl(defview) else {
        log::push("comctl32 subclass functions not found; staying idle");
        return false;
    };
    let _ = COMCTL.set(cc);
    // Pin the DLL before starting the thread: after the app removes the hook, Windows must not unload the DLL while the pipe thread still runs its code.
    let mut me = HMODULE::default();
    // SAFETY: the address of the hook function itself lies inside this DLL.
    let pinned = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            PCWSTR(sg_hook_proc as *const u16),
            &mut me,
        )
    };
    if pinned.is_err() || me.is_invalid() {
        log::push("cannot pin the DLL; staying idle");
        return false;
    }
    SELF_MODULE.store(me.0 as isize, Ordering::SeqCst);
    DEFVIEW.store(defview.0 as isize, Ordering::SeqCst);
    // SAFETY: no parameters.
    DESKTOP_TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    crate::guard::install_panic_hook();
    INITIALIZED.store(true, Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("sg-explorer-pipe".into()).spawn(crate::pipe::thread_main);
    // SAFETY: no parameters.
    let pid = unsafe { GetCurrentProcessId() };
    match spawned {
        Ok(_) => log::push(format!("loaded into explorer.exe pid {pid}, desktop thread {}", DESKTOP_TID.load(Ordering::SeqCst))),
        Err(e) => log::push(format!("cannot start pipe thread: {e}")),
    }
    true
}

/// Gets the subclass functions from the right comctl32 build: prefer the module containing DefView's current window proc
/// (Explorer already subclassed DefView with comctl32 v6, so we join the same chain), otherwise the `comctl32.dll`
/// picked by the desktop thread's activation context.
fn resolve_comctl(defview: HWND) -> Option<Comctl> {
    // SAFETY: only reads module/function info; function pointers come from GetProcAddress with the declared signature.
    unsafe {
        let proc_addr = GetWindowLongPtrW(defview, GWLP_WNDPROC);
        let mut m = HMODULE::default();
        let from_wndproc = proc_addr != 0
            && GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                PCWSTR(proc_addr as *const u16),
                &mut m,
            )
            .is_ok()
            && module_name_is(m, "comctl32.dll");
        if !from_wndproc {
            m = GetModuleHandleW(w!("comctl32.dll")).ok()?;
        }
        // By name first; older comctl32 exports a few functions only by ordinal (410–413, fixed since comctl32 5.8).
        let get = |name: PCSTR, ordinal: usize| GetProcAddress(m, name).or_else(|| GetProcAddress(m, PCSTR(ordinal as *const u8)));
        let set = get(s!("SetWindowSubclass"), 410)?;
        let getsc = get(s!("GetWindowSubclass"), 411)?;
        let remove = get(s!("RemoveWindowSubclass"), 412)?;
        let def = get(s!("DefSubclassProc"), 413)?;
        log::push(format!("using {} for subclassing (from DefView wndproc: {from_wndproc})", module_path(m)));
        Some(Comctl {
            set: std::mem::transmute::<unsafe extern "system" fn() -> isize, SetSubclassFn>(set),
            remove: std::mem::transmute::<unsafe extern "system" fn() -> isize, RemoveSubclassFn>(remove),
            get: std::mem::transmute::<unsafe extern "system" fn() -> isize, GetSubclassFn>(getsc),
            def: std::mem::transmute::<unsafe extern "system" fn() -> isize, DefSubclassFn>(def),
        })
    }
}

fn module_path(m: HMODULE) -> String {
    let mut buf = [0u16; 1024];
    // SAFETY: m is a loaded module; local buffer.
    let n = unsafe { GetModuleFileNameW(Some(m), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

fn module_name_is(m: HMODULE, name: &str) -> bool {
    module_path(m).rsplit(['\\', '/']).next().is_some_and(|f| f.eq_ignore_ascii_case(name))
}

/// Calls `DefSubclassProc` of the right comctl32 (used only inside the subclass proc).
pub(crate) fn def_subclass(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match COMCTL.get() {
        // SAFETY: we are inside a subclass proc called by this very comctl32.
        Some(cc) => unsafe { (cc.def)(h, msg, wp, lp) },
        None => LRESULT(0),
    }
}

/// Whether our subclass is currently attached to `h` (checked with `GetWindowSubclass`).
pub(crate) fn subclass_present(h: HWND) -> bool {
    let Some(cc) = COMCTL.get() else { return false };
    let mut data = 0usize;
    // SAFETY: read-only; must be called on the thread that owns h.
    unsafe { (cc.get)(h, Some(labels_paint::subclass_proc), SUBCLASS_ID, &mut data).as_bool() }
}

/// Attaches the subclass (calling it repeatedly still yields a single subclass: comctl32 replaces `dwRefData` of the existing one).
pub(crate) fn install_subclass(h: HWND) -> bool {
    let Some(cc) = COMCTL.get() else { return false };
    // SAFETY: on the thread that owns h (the desktop thread); the subclass function belongs to the pinned DLL.
    let ok = unsafe { (cc.set)(h, Some(labels_paint::subclass_proc), SUBCLASS_ID, 0).as_bool() } && subclass_present(h);
    INSTALLED.store(ok, Ordering::SeqCst);
    ok
}

pub(crate) fn remove_subclass(h: HWND) {
    if let Some(cc) = COMCTL.get() {
        // SAFETY: on the thread that owns h; removing from inside the subclass proc is fine too (comctl32 supports it).
        unsafe {
            let _ = (cc.remove)(h, Some(labels_paint::subclass_proc), SUBCLASS_ID);
        }
    }
    INSTALLED.store(false, Ordering::SeqCst);
}

pub(crate) fn listview_of(defview: HWND) -> Option<HWND> {
    let cached = hwnd(LISTVIEW.load(Ordering::SeqCst));
    if is_window(cached) {
        return Some(cached);
    }
    // SAFETY: looks up a child window; error → None.
    let lv = unsafe { FindWindowExW(Some(defview), None, w!("SysListView32"), None) }.ok().filter(|h| is_window(*h))?;
    LISTVIEW.store(lv.0 as isize, Ordering::SeqCst);
    Some(lv)
}

fn redraw_labels(defview: HWND) {
    if let Some(lv) = listview_of(defview) {
        // SAFETY: only marks the region for repaint; Explorer repaints itself in its current style.
        unsafe {
            let _ = InvalidateRect(Some(lv), None, true);
        }
    }
}

/// Desktop thread: attaches/detaches the subclass and repaints to match the desired paint table.
pub(crate) fn sync() {
    let dv = hwnd(DEFVIEW.load(Ordering::SeqCst));
    if !is_window(dv) {
        return;
    }
    let desired = if RESTING.load(Ordering::SeqCst) { None } else { DESIRED.lock().ok().and_then(|d| d.clone()) };
    let installed = INSTALLED.load(Ordering::SeqCst);
    match desired {
        Some(p) => {
            if !installed && !install_subclass(dv) {
                log::push("SetWindowSubclass failed");
                return;
            }
            if labels_paint::set_current(Some(p)) || !installed {
                redraw_labels(dv);
            }
        }
        None => {
            if installed {
                remove_subclass(dv);
                labels_paint::set_current(None);
                redraw_labels(dv);
                log::push("labels back to Windows defaults");
            } else {
                labels_paint::set_current(None);
            }
        }
    }
}

/// Severe error on the desktop thread (panic already caught): stop for good, remove the subclass, repaint with defaults, notify the pipe thread.
pub(crate) fn fault_here(msg: String) {
    RESTING.store(true, Ordering::SeqCst);
    if let Ok(mut d) = DESIRED.try_lock() {
        *d = None;
    }
    if let Ok(mut f) = FAULT.try_lock() {
        f.get_or_insert(msg);
    }
    let dv = hwnd(DEFVIEW.load(Ordering::SeqCst));
    if INSTALLED.load(Ordering::SeqCst) && is_window(dv) {
        // We are on the desktop thread (every caller of fault_here is).
        remove_subclass(dv);
        redraw_labels(dv);
    }
    labels_paint::set_current(None);
    wake_pipe();
}

pub(crate) fn take_fault() -> Option<String> {
    FAULT.lock().ok().and_then(|mut f| f.take())
}

pub(crate) fn mark_layout_dirty() {
    LAYOUT_DIRTY.store(true, Ordering::SeqCst);
    wake_pipe();
}

pub(crate) fn wake_pipe() {
    let e = WAKE_EVENT.load(Ordering::SeqCst);
    if e != 0 {
        // SAFETY: the event is created by the pipe thread and never closed.
        unsafe {
            let _ = SetEvent(HANDLE(e as *mut core::ffi::c_void));
        }
    }
}

// ---- Pipe thread side ----

/// The current DefView belongs to this very process; if the old DefView is gone, look it up again (Progman/WorkerW).
pub(crate) fn current_defview() -> Option<HWND> {
    let dv = hwnd(DEFVIEW.load(Ordering::SeqCst));
    if is_window(dv) && owned_by_me(dv) {
        return Some(dv);
    }
    let found = locate_defview()?;
    DEFVIEW.store(found.0 as isize, Ordering::SeqCst);
    LISTVIEW.store(0, Ordering::SeqCst);
    INSTALLED.store(false, Ordering::SeqCst);
    let mut pid = 0u32;
    // SAFETY: found is the window we just located.
    let tid = unsafe { GetWindowThreadProcessId(found, Some(&mut pid)) };
    DESKTOP_TID.store(tid, Ordering::SeqCst);
    log::push(format!("desktop view changed, now on thread {tid}"));
    Some(found)
}

fn owned_by_me(h: HWND) -> bool {
    let mut pid = 0u32;
    // SAFETY: h is a live window (checked with IsWindow).
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

fn locate_defview() -> Option<HWND> {
    // SAFETY: only looks up windows; an error result becomes None.
    unsafe {
        let progman = crate::own_window::find_own_window(None, w!("Progman"))?;
        if let Ok(dv) = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None)
            && is_window(dv)
            && owned_by_me(dv)
        {
            return Some(dv);
        }
        // Older desktop layout: DefView lives inside a WorkerW.
        let mut after: Option<HWND> = None;
        for _ in 0..64 {
            let ww = FindWindowExW(None, after, w!("WorkerW"), None).ok().filter(|h| !h.is_invalid())?;
            if let Ok(dv) = FindWindowExW(Some(ww), None, w!("SHELLDLL_DefView"), None)
                && is_window(dv)
                && owned_by_me(dv)
            {
                return Some(dv);
            }
            after = Some(ww);
        }
        None
    }
}

/// Pipe thread: sets the desired paint table, then pushes the work to the desktop thread. Never blocks for more than ~1 s.
pub(crate) fn set_desired(p: Option<Arc<LabelsPaint>>) {
    let want = p.is_some();
    if let Ok(mut d) = DESIRED.lock() {
        *d = p;
    }
    request_sync(want);
}

fn request_sync(want_paint: bool) {
    let Some(dv) = current_defview() else { return };
    let m = sg_msg();
    if INSTALLED.load(Ordering::SeqCst) {
        // SAFETY: a private message to DefView; our subclass handles and swallows it.
        unsafe {
            let _ = PostMessageW(Some(dv), m, WPARAM(CMD_SYNC), LPARAM(0));
        }
    } else if want_paint && !RESTING.load(Ordering::SeqCst) {
        sync_via_local_hook(dv, m);
    }
}

/// No subclass yet to receive the message: temporarily install a `WH_CALLWNDPROC` hook (inside Explorer itself, for the desktop thread),
/// send the private message so [`sg_hook_proc`] runs [`sync`] on the desktop thread, then remove the hook immediately.
fn sync_via_local_hook(dv: HWND, m: u32) {
    let tid = DESKTOP_TID.load(Ordering::SeqCst);
    let me = SELF_MODULE.load(Ordering::SeqCst);
    if tid == 0 || me == 0 {
        return;
    }
    // SAFETY: a hook for one thread of this very process, the hook function lives in the pinned DLL; removed right after sending.
    unsafe {
        let Ok(hook) = SetWindowsHookExW(WH_CALLWNDPROC, Some(sg_hook_proc), Some(HINSTANCE(me as *mut core::ffi::c_void)), tid) else {
            log::push("cannot set local hook to reach the desktop thread");
            return;
        };
        let r = SendMessageTimeoutW(dv, m, WPARAM(CMD_SYNC), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT, 1000, None);
        if r.0 == 0 {
            log::push("desktop thread did not answer within 1 s");
        }
        let _ = UnhookWindowsHookEx(hook);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! For tests: simulates the "desktop thread" with a window of the test process itself.
    use super::*;

    pub fn setup_for_window(h: HWND) {
        // The test does not run inside explorer.exe: get comctl32 directly (load it if not present).
        // SAFETY: loads a system library.
        unsafe {
            let _ = windows::Win32::System::LibraryLoader::LoadLibraryW(w!("comctl32.dll"));
        }
        if COMCTL.get().is_none() {
            let cc = resolve_comctl(h).expect("comctl32");
            let _ = COMCTL.set(cc);
        }
        DEFVIEW.store(h.0 as isize, Ordering::SeqCst);
        LISTVIEW.store(0, Ordering::SeqCst);
        RESTING.store(false, Ordering::SeqCst);
        INSTALLED.store(false, Ordering::SeqCst);
    }

    pub fn set_desired_only(p: Option<Arc<LabelsPaint>>) {
        *DESIRED.lock().unwrap() = p;
    }
}
