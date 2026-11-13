//! Loads the DLL into Explorer with a `WH_CALLWNDPROC` hook on the desktop thread only (the approach used by TranslucentTB,
//! `ExplorerTAP/api.cpp`, GPL-3.0), without writing anything into Explorer's memory.
//!
//! 1. Find `SHELLDLL_DefView` (`sg_desktop::DesktopLayer`), get its owning thread + process, and check it is really `explorer.exe`.
//! 2. `LoadLibraryW` the DLL inside the app itself to get the address of `sg_hook_proc` (the DLL has no `DllMain`: loading runs nothing).
//! 3. `SetWindowsHookExW(WH_CALLWNDPROC, sg_hook_proc, hmod, tid)` → `SendMessageTimeoutW(DefView, <msg>, CMD_INIT)`:
//!    Windows loads the DLL into Explorer and calls the hook function on the desktop thread; the DLL pins itself and then connects the pipe.
//! 4. The caller waits for the DLL to connect, then drops the [`HookGuard`] → `UnhookWindowsHookEx` + `FreeLibrary` (the copy in the app).

use sg_explorer_proto::{CMD_INIT, LOADED_MARKER, WINDOW_MESSAGE};
use std::path::Path;
use windows::Win32::Foundation::{CloseHandle, FreeLibrary, HMODULE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{
    OpenMutexW, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
    SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowThreadProcessId, HHOOK, HOOKPROC, PostMessageW, RegisterWindowMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_CALLWNDPROC, WM_USER,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, s};

#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub defview: HWND,
    pub tid: u32,
    pub pid: u32,
}

pub fn sg_msg() -> u32 {
    let wide: Vec<u16> = WINDOW_MESSAGE.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the string is NUL-terminated.
    unsafe { RegisterWindowMessageW(PCWSTR(wide.as_ptr())) }
}

/// Whether the exe name of process `pid` is `explorer.exe` (only asks for the name, never reads memory).
pub fn is_explorer_pid(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: minimal query rights; the handle is closed.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok && String::from_utf16_lossy(&buf[..len as usize])
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|f| f.eq_ignore_ascii_case("explorer.exe"))
    }
}

/// The desktop's DefView and the thread/process that owns it (must be `explorer.exe`).
pub fn explorer_target() -> Result<Target, String> {
    let layer = sg_desktop::DesktopLayer::locate().ok_or("desktop window not found")?;
    let defview = layer.icon_view().ok_or("SHELLDLL_DefView not found")?;
    let mut pid = 0u32;
    // SAFETY: defview is a live window.
    let tid = unsafe { GetWindowThreadProcessId(defview, Some(&mut pid)) };
    if tid == 0 || pid == 0 {
        return Err("cannot read the desktop window's thread".into());
    }
    if !is_explorer_pid(pid) {
        return Err(format!("desktop window belongs to pid {pid}, which is not explorer.exe"));
    }
    Ok(Target { defview, tid, pid })
}

/// Whether Explorer already has our DLL (the DLL creates a named mutex once it is inside Explorer).
pub fn marker_exists() -> bool {
    let wide: Vec<u16> = LOADED_MARKER.encode_utf16().chain(Some(0)).collect();
    // SAFETY: only opened to check existence; closed immediately.
    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(wide.as_ptr())) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

/// The installed hook + the copy of the DLL loaded in the app. Dropping it removes the hook and calls `FreeLibrary` (the DLL inside Explorer has pinned itself).
pub struct HookGuard {
    hook: HHOOK,
    module: HMODULE,
}

impl Drop for HookGuard {
    fn drop(&mut self) {
        // SAFETY: the hook and module were created by `inject`.
        unsafe {
            let _ = UnhookWindowsHookEx(self.hook);
            let _ = FreeLibrary(self.module);
        }
    }
}

pub fn inject(dll: &Path, t: &Target) -> Result<HookGuard, String> {
    // SAFETY: loads the app's own DLL (no DllMain); looks up the function by name; the hook covers exactly one Explorer thread.
    unsafe {
        let module = LoadLibraryW(&HSTRING::from(dll.as_os_str())).map_err(|e| format!("LoadLibrary: {e}"))?;
        let Some(proc_addr) = GetProcAddress(module, s!("sg_hook_proc")) else {
            let _ = FreeLibrary(module);
            return Err("sg_hook_proc not exported".into());
        };
        let hook_proc: HOOKPROC = Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn(i32, WPARAM, LPARAM) -> windows::Win32::Foundation::LRESULT>(proc_addr));
        let hook = match SetWindowsHookExW(WH_CALLWNDPROC, hook_proc, Some(module.into()), t.tid) {
            Ok(h) => h,
            Err(e) => {
                let _ = FreeLibrary(module);
                return Err(format!("SetWindowsHookEx: {e}"));
            }
        };
        let guard = HookGuard { hook, module };
        // Trigger the hook: a private message to DefView; Explorer does not know this message and ignores it.
        let r = SendMessageTimeoutW(t.defview, sg_msg(), WPARAM(CMD_INIT), LPARAM(0), SMTO_ABORTIFHUNG, 2000, None);
        if r.0 == 0 {
            return Err("explorer did not process the trigger message within 2 s".into());
        }
        Ok(guard)
    }
}

/// Exits Explorer the standard way (same as "Exit Explorer" via Ctrl+Shift+right-click on the taskbar: `WM_USER + 436`
/// sent to `Shell_TrayWnd`), waits for it to exit, then starts `explorer.exe` again. Every open File Explorer window will close.
/// Only call this when the user pressed the button and confirmed.
pub fn restart_explorer() -> Result<(), String> {
    // SAFETY: finds the taskbar, opens the process only to wait on it (SYNCHRONIZE), and sends exactly Explorer's exit message.
    unsafe {
        // Do not use a bare FindWindow: the custom bar of the YASB taskbar part creates a fake `Shell_TrayWnd` inside this very app.
        let tray = sg_core::platform::native_taskbars()
            .into_iter()
            .find(|t| t.primary)
            .map(|t| HWND(t.hwnd as *mut core::ffi::c_void))
            .ok_or_else(|| "taskbar not found".to_string())?;
        let mut pid = 0u32;
        GetWindowThreadProcessId(tray, Some(&mut pid));
        if !is_explorer_pid(pid) {
            return Err("taskbar does not belong to explorer.exe".into());
        }
        let proc_h = OpenProcess(PROCESS_SYNCHRONIZE, false, pid).map_err(|e| e.to_string())?;
        tracing::warn!(target: "sg::explorer", "restarting explorer (pid {pid}) at the user's request");
        let posted = PostMessageW(Some(tray), WM_USER + 436, WPARAM(0), LPARAM(0));
        let exited = posted.is_ok() && WaitForSingleObject(proc_h, 10_000) == WAIT_OBJECT_0;
        let _ = CloseHandle(proc_h);
        if !exited {
            return Err("explorer did not exit within 10 s".into());
        }
    }
    std::process::Command::new("explorer.exe").spawn().map(|_| ()).map_err(|e| e.to_string())
}
