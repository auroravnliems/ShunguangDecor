// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! The wallpaper is an application (exe, Unity/Godot game...): launch the process, wait for the main window to appear,
//! strip its border, then attach it to the host window. Pause = suspend the whole process (NtSuspendProcess).

use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use windows::core::{s, w, BOOL};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, RECT};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, SetParent,
    SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, GW_OWNER, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOZORDER, WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW, WS_EX_DLGMODALFRAME, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX,
    WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};

use super::{host, rect_size, PauseReason, Player};
use sg_desktop::DesktopLayer as Desktop;

pub struct AppPlayer {
    host: HWND,
    child: Child,
    window: Option<HWND>,
    started: Instant,
    paused: bool,
}

impl AppPlayer {
    pub fn new(desktop: &Desktop, rect: RECT, exe: &str, args: Option<&str>) -> Result<AppPlayer, String> {
        let host = host::create(desktop, rect)?;
        let mut cmd = Command::new(exe);
        if let Some(dir) = std::path::Path::new(exe).parent() {
            cmd.current_dir(dir);
        }
        if let Some(a) = args.filter(|a| !a.trim().is_empty()) {
            use std::os::windows::process::CommandExt;
            // Keep the argument string as-is like Lively (e.g. "-popupwindow -screen-fullscreen 0").
            cmd.raw_arg(a);
        }
        let child = cmd.spawn().map_err(|e| {
            host::destroy(host);
            format!("Không chạy được {exe}: {e}")
        })?;
        crate::job::adopt(&child);
        logln!("Ứng dụng hình nền pid {} ({exe})", child.id());
        Ok(AppPlayer { host, child, window: None, started: Instant::now(), paused: false })
    }

    fn attach(&mut self, win: HWND) {
        unsafe {
            let style = GetWindowLongPtrW(win, GWL_STYLE) as u32;
            let new_style = (style & !(WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MAXIMIZEBOX.0 | WS_MINIMIZEBOX.0 | WS_POPUP.0))
                | WS_CHILD.0;
            SetWindowLongPtrW(win, GWL_STYLE, new_style as isize);
            let ex = GetWindowLongPtrW(win, GWL_EXSTYLE) as u32;
            let new_ex = ex & !(WS_EX_APPWINDOW.0 | WS_EX_DLGMODALFRAME.0 | WS_EX_WINDOWEDGE.0);
            SetWindowLongPtrW(win, GWL_EXSTYLE, new_ex as isize);
            let _ = SetParent(win, Some(self.host));
            let mut r = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(self.host, &mut r);
            let (w, h) = rect_size(&r);
            let _ = SetWindowPos(win, None, 0, 0, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
        self.window = Some(win);
        logln!("Đã gắn cửa sổ ứng dụng {:?}", win.0);
    }

    fn suspend(&self, suspend: bool) {
        type NtFn = unsafe extern "system" fn(HANDLE) -> i32;
        unsafe {
            let Ok(ntdll) = GetModuleHandleW(w!("ntdll.dll")) else { return };
            let name = if suspend { s!("NtSuspendProcess") } else { s!("NtResumeProcess") };
            if let Some(f) = GetProcAddress(ntdll, name) {
                let f: NtFn = std::mem::transmute(f);
                f(HANDLE(self.child.as_raw_handle()));
            }
        }
    }
}

struct Search {
    pid: u32,
    found: HWND,
}

unsafe extern "system" fn find_main(hwnd: HWND, data: LPARAM) -> BOOL {
    let s = &mut *(data.0 as *mut Search);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == s.pid && IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd, GW_OWNER).unwrap_or_default().is_invalid() {
        s.found = hwnd;
        return false.into();
    }
    true.into()
}

impl Player for AppPlayer {
    fn backend(&self) -> &'static str {
        "app"
    }

    fn host(&self) -> Option<HWND> {
        Some(self.host)
    }

    fn set_paused(&mut self, paused: bool, _reason: PauseReason) {
        if paused != self.paused {
            self.paused = paused;
            self.suspend(paused);
        }
    }

    fn set_volume(&mut self, _volume: u8) {}

    fn resize(&mut self, rect: RECT) {
        host::move_to(self.host, rect);
        if let Some(win) = self.window {
            let (w, h) = rect_size(&rect);
            unsafe {
                let _ = SetWindowPos(win, None, 0, 0, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }

    fn tick(&mut self) -> Result<(), String> {
        if let Ok(Some(status)) = self.child.try_wait() {
            return Err(format!("Ứng dụng hình nền đã thoát ({status})"));
        }
        match self.window {
            Some(win) => {
                if unsafe { !IsWindow(Some(win)).as_bool() } {
                    return Err("Cửa sổ ứng dụng hình nền đã đóng".into());
                }
            }
            None => {
                let mut s = Search { pid: self.child.id(), found: HWND::default() };
                unsafe {
                    let _ = EnumWindows(Some(find_main), LPARAM(&mut s as *mut _ as isize));
                }
                if !s.found.is_invalid() {
                    self.attach(s.found);
                } else if self.started.elapsed() > Duration::from_secs(30) {
                    return Err("Ứng dụng không mở cửa sổ nào sau 30 giây".into());
                }
            }
        }
        Ok(())
    }

    fn close(&mut self) {
        if self.paused {
            self.suspend(false);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        host::destroy(self.host);
    }
}
