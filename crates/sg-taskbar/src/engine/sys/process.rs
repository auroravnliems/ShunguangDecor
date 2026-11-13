//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Window/process information: title, class, exe, application name (FileDescription), icon.

use std::collections::HashMap;
use std::cell::RefCell;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug, Default)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub class_name: String,
    pub pid: u32,
    pub process_name: String,
    pub exe_path: String,
    pub monitor: isize,
}

pub fn window_title(h: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

pub fn class_name(h: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetClassNameW(h, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

pub fn window_pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid));
    }
    pid
}

pub fn exe_path(pid: u32) -> Option<String> {
    unsafe {
        let hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(hp, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(hp);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn window_info(h: HWND) -> Option<WindowInfo> {
    unsafe {
        if h.0.is_null() || !IsWindow(Some(h)).as_bool() {
            return None;
        }
    }
    let pid = window_pid(h);
    let exe = exe_path(pid).unwrap_or_default();
    let process_name = exe.rsplit(['\\', '/']).next().unwrap_or("").to_string();
    Some(WindowInfo {
        hwnd: h.0 as isize,
        title: window_title(h),
        class_name: class_name(h),
        pid,
        process_name,
        exe_path: exe,
        monitor: crate::engine::win::monitor_of_window(h),
    })
}

/// The FileDescription string from the exe file's version info.
pub fn file_description(path: &str) -> Option<String> {
    unsafe {
        let wpath: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), Some(0), size, data.as_mut_ptr() as *mut _).ok()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let mut lang = (0x0409u16, 0x04B0u16);
        if VerQueryValueW(data.as_ptr() as *const _, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool() && len >= 4 {
            let p = ptr as *const u16;
            lang = (*p, *p.add(1));
        }
        let q = format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", lang.0, lang.1);
        let wq: Vec<u16> = q.encode_utf16().chain(Some(0)).collect();
        if VerQueryValueW(data.as_ptr() as *const _, PCWSTR(wq.as_ptr()), &mut ptr, &mut len).as_bool() && len > 0 {
            let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
            let s = String::from_utf16_lossy(s).trim_end_matches('\0').trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }
}

thread_local! {
    static NAMES: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
}

/// Display name of the application by pid (like `get_app_name_from_pid`).
pub fn app_name(pid: u32) -> String {
    if let Some(n) = NAMES.with(|m| m.borrow().get(&pid).cloned()) {
        return n;
    }
    let exe = exe_path(pid).unwrap_or_default();
    let name = file_description(&exe).unwrap_or_else(|| {
        let f = exe.rsplit(['\\', '/']).next().unwrap_or("").to_string();
        f.strip_suffix(".exe").map(str::to_string).unwrap_or(f)
    });
    NAMES.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 500 {
            m.clear();
        }
        m.insert(pid, name.clone());
    });
    name
}

/// The window's HICON (small or large) – not destroyed by us, it belongs to the window/class.
pub fn window_hicon(h: HWND, big: bool) -> Option<HICON> {
    unsafe {
        let order: &[usize] = if big { &[ICON_BIG as usize, ICON_SMALL2 as usize, ICON_SMALL as usize] } else { &[ICON_SMALL2 as usize, ICON_SMALL as usize, ICON_BIG as usize] };
        for &kind in order {
            let mut res = 0usize;
            let r = SendMessageTimeoutW(h, WM_GETICON, WPARAM(kind), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 100, Some(&mut res));
            if r.0 != 0 && res != 0 {
                return Some(HICON(res as *mut _));
            }
        }
        let c = GetClassLongPtrW(h, if big { GCLP_HICON } else { GCLP_HICONSM });
        if c != 0 {
            return Some(HICON(c as *mut _));
        }
        let c = GetClassLongPtrW(h, if big { GCLP_HICONSM } else { GCLP_HICON });
        if c != 0 {
            return Some(HICON(c as *mut _));
        }
        None
    }
}
