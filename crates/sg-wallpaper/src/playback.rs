// Ported from TachyonWallpaper (idea originally from Lively Wallpaper, GPL-3.0).
//! Rules for auto-pausing the wallpaper so it does not eat CPU/GPU/battery when nobody can see it.

use std::collections::HashSet;

use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONULL};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, IsZoomed, GWL_EXSTYLE, SM_REMOTESESSION, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

use sg_desktop::layer::class_name;
use sg_desktop::DesktopLayer as Desktop;
use sg_desktop::Monitor;
use crate::config::WallpaperConfig;

/// Result of a single check.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// Pause every wallpaper (battery, locked machine, listed application...).
    pub system_pause: bool,
    /// Monitors (by device name) that are fully covered or that the "focus" rule asks to pause.
    pub covered: HashSet<String>,
}

pub fn power_status() -> (bool, bool) {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe {
        if GetSystemPowerStatus(&mut s).is_err() {
            return (false, false);
        }
    }
    // ACLineStatus: 0 = running on battery; SystemStatusFlag: 1 = Battery Saver is on.
    (s.ACLineStatus == 0, s.SystemStatusFlag == 1)
}

pub fn is_remote_session() -> bool {
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4).is_ok() && cloaked != 0
    }
}

pub fn process_name(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit('\\').next().map(|s| s.to_lowercase())
    }
}

/// Shell window classes are never considered to "cover" the wallpaper.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "ShunguangDecor.WallpaperMain",
    "ShunguangDecor.WallpaperHost",
    "TachyonTaskbar.Bar",
    "NotifyIconOverflowWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

struct Ctx<'a> {
    monitors: &'a [Monitor],
    covered: HashSet<String>,
    own_pid: u32,
}

fn covers(r: &RECT, m: &RECT) -> bool {
    r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
}

unsafe extern "system" fn enum_cover(hwnd: HWND, data: LPARAM) -> BOOL {
    let ctx = &mut *(data.0 as *mut Ctx);
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() || is_cloaked(hwnd) {
        return true.into();
    }
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW.0 != 0 || ex & WS_EX_TRANSPARENT.0 != 0 {
        return true.into();
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == ctx.own_pid {
        return true.into();
    }
    let class = class_name(hwnd);
    if SHELL_CLASSES.contains(&class.as_str()) {
        return true.into();
    }
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_err() {
        return true.into();
    }
    let zoomed = IsZoomed(hwnd).as_bool();
    let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
    for m in ctx.monitors {
        let on_this = hmon.0 as isize == m.handle;
        if (zoomed && on_this) || covers(&r, &m.rect.into()) {
            ctx.covered.insert(m.device.clone());
        }
    }
    true.into()
}

pub fn check(settings: &WallpaperConfig, monitors: &[Monitor], desktop: Option<&Desktop>, locked: bool) -> Snapshot {
    let mut snap = Snapshot::default();
    let (on_battery, saver) = power_status();
    if locked
        || (settings.pause_on_battery && on_battery)
        || (settings.pause_on_battery_saver && saver)
        || (settings.pause_on_remote_desktop && is_remote_session())
    {
        snap.system_pause = true;
        return snap;
    }

    let fg = unsafe { GetForegroundWindow() };
    let fg_is_desktop = fg.is_invalid() || desktop.map(|d| d.is_desktop_window(fg)).unwrap_or(false);

    if !fg_is_desktop && !settings.pause_apps.is_empty() {
        if let Some(name) = process_name(fg) {
            if settings.pause_apps.iter().any(|a| a.trim().eq_ignore_ascii_case(&name)) {
                snap.system_pause = true;
                return snap;
            }
        }
    }

    if settings.pause_when_covered {
        let mut ctx = Ctx { monitors, covered: HashSet::new(), own_pid: std::process::id() };
        unsafe {
            let _ = EnumWindows(Some(enum_cover), LPARAM(&mut ctx as *mut _ as isize));
        }
        snap.covered = ctx.covered;
    }

    if settings.pause_when_app_focused && !fg_is_desktop {
        let class = class_name(fg);
        if !SHELL_CLASSES.contains(&class.as_str()) {
            let hmon = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONULL) };
            for m in monitors {
                if hmon.0 as isize == m.handle {
                    snap.covered.insert(m.device.clone());
                }
            }
        }
    }

    if !settings.pause_per_monitor && !snap.covered.is_empty() {
        // One monitor covered → pause everything.
        for m in monitors {
            snap.covered.insert(m.device.clone());
        }
    }
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_checks_full_rect() {
        let m = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        assert!(covers(&RECT { left: -8, top: -8, right: 1928, bottom: 1088 }, &m));
        assert!(!covers(&RECT { left: 0, top: 0, right: 1920, bottom: 1040 }, &m));
    }

    #[test]
    fn check_runs_on_this_machine() {
        let monitors = sg_desktop::monitors();
        let s = WallpaperConfig::default();
        let _ = check(&s, &monitors, None, false);
        let locked = check(&s, &monitors, None, true);
        assert!(locked.system_pause);
    }
}
