//! Windows.

use ::windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, MAX_PATH, WPARAM};
use ::windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
use ::windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, REG_ROUTINE_FLAGS, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use ::windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, SMTO_ABORTIFHUNG, SPI_GETDESKWALLPAPER, SW_HIDE, SW_SHOWNOACTIVATE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SendMessageTimeoutW, ShowWindowAsync, SystemParametersInfoW,
};
use ::windows::core::{HSTRING, PCWSTR, w};
use std::io;
use std::path::PathBuf;

/// One of Explorer's native taskbars (primary, or secondary on another monitor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeTaskbarWnd {
    /// `HWND` as an integer (so it can be used in code that does not depend on Win32).
    pub hwnd: isize,
    /// `Shell_TrayWnd` (primary monitor) rather than `Shell_SecondaryTrayWnd`.
    pub primary: bool,
    /// Rectangle on screen (even while hidden): left, top, right, bottom.
    pub rect: (i32, i32, i32, i32),
}

/// Every native taskbar: `Shell_TrayWnd` + every `Shell_SecondaryTrayWnd`. Skips windows owned by this process:
/// the custom bar's tray (engine) creates a fake window of class `Shell_TrayWnd` that sits topmost in the app, so
/// a bare `FindWindow` could return our own window and we would hide/show the wrong one.
pub fn native_taskbars() -> Vec<NativeTaskbarWnd> {
    use ::windows::Win32::Foundation::RECT;
    use ::windows::Win32::System::Threading::GetCurrentProcessId;
    use ::windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, GetWindowThreadProcessId};
    let me = unsafe { GetCurrentProcessId() };
    let mut out = Vec::new();
    for (class, primary) in [(w!("Shell_TrayWnd"), true), (w!("Shell_SecondaryTrayWnd"), false)] {
        let mut after: Option<HWND> = None;
        // Bound the loop: a few windows per class is already plenty.
        for _ in 0..32 {
            // SAFETY: only enumerates top-level windows by class and reads their pid/rectangle.
            let h = match unsafe { FindWindowExW(None, after, class, PCWSTR::null()) } {
                Ok(h) if !h.is_invalid() => h,
                _ => break,
            };
            after = Some(h);
            let mut pid = 0u32;
            // SAFETY: `pid` lives for the whole call.
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            if pid == me {
                continue;
            }
            let mut r = RECT::default();
            // SAFETY: `r` lives for the whole call.
            let rect = if unsafe { GetWindowRect(h, &mut r) }.is_ok() { (r.left, r.top, r.right, r.bottom) } else { (0, 0, 0, 0) };
            out.push(NativeTaskbarWnd { hwnd: h.0 as isize, primary, rect });
        }
    }
    out
}

/// Returns the number of taskbars the request was sent to (0 = no taskbar found).
fn set_native_taskbars(cmd: ::windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD) -> usize {
    let bars = native_taskbars();
    for t in &bars {
        // SAFETY: the HWND was just enumerated. `ShowWindowAsync` only posts a request to the window's owner thread queue,
        // so it does not block when Explorer hangs; if the window has disappeared it simply returns an error.
        unsafe {
            let _ = ShowWindowAsync(HWND(t.hwnd as *mut _), cmd);
        }
    }
    bars.len()
}

/// Shows the native taskbar again on every monitor (called by the watchdog process when the main app died while hiding it). Does not take focus.
/// Finding no taskbar at all (Explorer is dead/restarting) is an ERROR, not success: the caller must keep this task
/// in the journal and retry once a taskbar exists.
pub fn show_native_taskbar() -> io::Result<()> {
    if set_native_taskbars(SW_SHOWNOACTIVATE) == 0 {
        return Err(io::Error::new(io::ErrorKind::NotFound, "native taskbar not found"));
    }
    Ok(())
}

/// Hides the native taskbar on every monitor. The caller must write `RestoreAction::ShowNativeTaskbar` to the journal FIRST.
pub fn hide_native_taskbars() -> io::Result<()> {
    let _ = set_native_taskbars(SW_HIDE);
    Ok(())
}

const ABS_AUTOHIDE: u32 = 0x1;

/// Reads the auto-hide bit from an `ABM_GETSTATE` result. Microsoft docs: "As of Windows 7, ABS_ALWAYSONTOP is no longer returned
/// because the taskbar is always in that state", so we do NOT check that bit (measured on Windows 11: 0 = normal,
/// 1 = auto-hide). The only error is a missing taskbar (handled by the caller).
fn parse_autohide(state: u32) -> bool {
    state & ABS_AUTOHIDE != 0
}

/// The new state to write so auto-hide becomes `on` (other bits preserved); `None` = already correct, nothing to write.
fn plan_autohide(state: u32, on: bool) -> Option<u32> {
    let next = if on { state | ABS_AUTOHIDE } else { state & !ABS_AUTOHIDE };
    (next != state).then_some(next)
}

fn appbar_data(hwnd: HWND, lparam: isize) -> ::windows::Win32::UI::Shell::APPBARDATA {
    ::windows::Win32::UI::Shell::APPBARDATA {
        cbSize: std::mem::size_of::<::windows::Win32::UI::Shell::APPBARDATA>() as u32,
        hWnd: hwnd,
        lParam: LPARAM(lparam),
        ..Default::default()
    }
}

/// Explorer's primary taskbar window (not the app's fake window).
fn primary_taskbar_hwnd() -> Option<HWND> {
    native_taskbars().into_iter().find(|t| t.primary).map(|t| HWND(t.hwnd as *mut _))
}

/// Is the native taskbar's auto-hide bit on? (`ABM_GETSTATE`). `None` if there is no taskbar.
pub fn taskbar_autohide() -> Option<bool> {
    use ::windows::Win32::UI::Shell::{ABM_GETSTATE, SHAppBarMessage};
    let mut d = appbar_data(primary_taskbar_hwnd()?, 0);
    // SAFETY: `d` is valid, cbSize is correct.
    let state = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut d) } as u32;
    Some(parse_autohide(state))
}

/// Turns the native taskbar's auto-hide on/off, preserving the other bits (always-on-top...). With auto-hide on, the work area is not reserved.
pub fn set_taskbar_autohide(on: bool) -> io::Result<()> {
    use ::windows::Win32::UI::Shell::{ABM_GETSTATE, ABM_SETSTATE, SHAppBarMessage};
    let hwnd = primary_taskbar_hwnd().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "native taskbar not found"))?;
    let mut d = appbar_data(hwnd, 0);
    // SAFETY: as above.
    let state = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut d) } as u32;
    if let Some(next) = plan_autohide(state, on) {
        let mut d = appbar_data(hwnd, next as isize);
        // SAFETY: as above.
        unsafe { SHAppBarMessage(ABM_SETSTATE, &mut d) };
    }
    Ok(())
}

/// Watchdog task: turn auto-hide off (only the auto-hide bit) on the native taskbar. A missing taskbar is an ERROR (the task must
/// stay in the journal and be retried), not "done".
pub fn disable_taskbar_autohide() -> io::Result<()> {
    set_taskbar_autohide(false)
}

const CURRENT_VERSION: PCWSTR = w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");

fn reg_raw(value: &str, flags: REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    let value = HSTRING::from(value);
    let mut size: u32 = 0;
    // SAFETY: called twice using the standard pattern: the first call gets the size, the second reads into a correctly sized buffer.
    unsafe {
        if RegGetValueW(HKEY_LOCAL_MACHINE, CURRENT_VERSION, &value, flags, None, None, Some(&mut size)) != ERROR_SUCCESS
        {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            CURRENT_VERSION,
            &value,
            flags,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        buf.truncate(size as usize);
        Some(buf)
    }
}

fn reg_string(value: &str) -> Option<String> {
    let b = reg_raw(value, RRF_RT_REG_SZ)?;
    let wide: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&c| c != 0).collect();
    Some(String::from_utf16_lossy(&wide)).filter(|s| !s.is_empty())
}

fn reg_dword(value: &str) -> Option<u32> {
    let b = reg_raw(value, RRF_RT_REG_DWORD)?;
    Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?))
}

/// For example "Windows 11 Home 25H2 (build 26200.7171)". The registry still says "Windows 10" on Windows 11,
/// so we correct it by build number (>= 22000 is Windows 11).
pub fn os_description() -> String {
    let product = reg_string("ProductName").unwrap_or_else(|| "Windows".into());
    let build: u32 = reg_string("CurrentBuildNumber").and_then(|b| b.parse().ok()).unwrap_or(0);
    let product = if build >= 22000 { product.replacen("Windows 10", "Windows 11", 1) } else { product };
    let display = reg_string("DisplayVersion").map(|d| format!(" {d}")).unwrap_or_default();
    let ubr = reg_dword("UBR").map(|u| format!(".{u}")).unwrap_or_default();
    format!("{product}{display} (build {build}{ubr})")
}

/// Names of the graphics cards (excluding "Microsoft Basic Render Driver").
pub fn gpus() -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: DXGI COM only reads adapter descriptions.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        for i in 0..16 {
            let Ok(adapter) = factory.EnumAdapters1(i) else { break };
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue;
            }
            let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..end]);
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// The current static wallpaper. If the original file was deleted, falls back to the copy Windows keeps in Themes.
pub fn current_wallpaper_path() -> Option<PathBuf> {
    let mut buf = [0u16; MAX_PATH as usize];
    // SAFETY: a MAX_PATH-character buffer, as the SPI_GETDESKWALLPAPER documentation requires.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            MAX_PATH,
            Some(buf.as_mut_ptr().cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
    };
    if ok {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let p = PathBuf::from(String::from_utf16_lossy(&buf[..end]));
        if !p.as_os_str().is_empty() && p.is_file() {
            return Some(p);
        }
    }
    let transcoded = dirs::config_dir()?.join(r"Microsoft\Windows\Themes").join(crate::desktop::WINDOWS_TRANSCODED);
    transcoded.is_file().then_some(transcoded)
}

/// The 8.3 short name of a path (e.g. `C:\Users\NGUYEN~1`), if it differs from the long name.
pub fn short_path(p: &std::path::Path) -> Option<PathBuf> {
    use ::windows::Win32::Storage::FileSystem::GetShortPathNameW;
    let long = HSTRING::from(p.as_os_str());
    let mut buf = vec![0u16; 1024];
    // SAFETY: `long` is a NUL-terminated UTF-16 string that lives for the whole call; Windows writes at most `buf.len()` elements.
    let n = unsafe { GetShortPathNameW(&long, Some(&mut buf)) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let short = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    (short.as_os_str() != p.as_os_str()).then_some(short)
}

// ---- Desktop text, fallback method: read/set the desktop listview's text color (no pointers, so it can be sent across processes) ----

const LVM_GETTEXTCOLOR: u32 = 0x1023;
const LVM_SETTEXTCOLOR: u32 = 0x1024;
const LVM_GETTEXTBKCOLOR: u32 = 0x1025;
const LVM_SETTEXTBKCOLOR: u32 = 0x1026;

/// The `SysListView32` holding the desktop icons (inside the `SHELLDLL_DefView` of Progman or of a WorkerW).
pub fn desktop_listview() -> Option<HWND> {
    // SAFETY: only looks up windows; errors become None.
    unsafe {
        let mut defview = FindWindowW(w!("Progman"), PCWSTR::null())
            .ok()
            .and_then(|p| FindWindowExW(Some(p), None, w!("SHELLDLL_DefView"), PCWSTR::null()).ok())
            .filter(|h| !h.is_invalid());
        let mut after: Option<HWND> = None;
        for _ in 0..64 {
            if defview.is_some() {
                break;
            }
            let Ok(ww) = FindWindowExW(None, after, w!("WorkerW"), PCWSTR::null()) else { break };
            if ww.is_invalid() {
                break;
            }
            defview = FindWindowExW(Some(ww), None, w!("SHELLDLL_DefView"), PCWSTR::null()).ok().filter(|h| !h.is_invalid());
            after = Some(ww);
        }
        FindWindowExW(defview, None, w!("SysListView32"), PCWSTR::null()).ok().filter(|h| !h.is_invalid())
    }
}

fn lv_send(lv: HWND, msg: u32, lp: isize) -> Option<usize> {
    let mut out = 0usize;
    // SAFETY: the message carries no pointers; it has a 1 s timeout and is dropped if Explorer hangs.
    let r = unsafe { SendMessageTimeoutW(lv, msg, WPARAM(0), LPARAM(lp), SMTO_ABORTIFHUNG, 1000, Some(&mut out)) };
    (r.0 != 0).then_some(out)
}

pub fn desktop_label_colors() -> Option<crate::labels_backup::LabelColors> {
    let lv = desktop_listview()?;
    Some(crate::labels_backup::LabelColors {
        text: lv_send(lv, LVM_GETTEXTCOLOR, 0)? as u32,
        text_bk: lv_send(lv, LVM_GETTEXTBKCOLOR, 0)? as u32,
    })
}

pub fn set_desktop_label_colors(c: crate::labels_backup::LabelColors) -> io::Result<()> {
    let lv = desktop_listview().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "desktop listview not found"))?;
    let ok = lv_send(lv, LVM_SETTEXTCOLOR, c.text as i32 as isize).is_some()
        && lv_send(lv, LVM_SETTEXTBKCOLOR, c.text_bk as i32 as isize).is_some();
    // SAFETY: only marks the region for repaint.
    unsafe {
        let _ = ::windows::Win32::Graphics::Gdi::InvalidateRect(Some(lv), None, true);
    }
    if ok { Ok(()) } else { Err(io::Error::new(io::ErrorKind::TimedOut, "explorer did not answer")) }
}

#[cfg(test)]
mod autohide_tests {
    use super::*;

    #[test]
    fn getstate_bit_is_read_directly() {
        // Measured on Windows 11: 0 = normal, 1 = auto-hide (bit 0x2 never appears).
        assert!(!parse_autohide(0));
        assert!(parse_autohide(1));
        assert!(parse_autohide(0x3));
        assert!(!parse_autohide(0x2));
    }

    #[test]
    fn plan_autohide_keeps_other_bits_and_can_turn_it_off_from_state_1() {
        assert_eq!(plan_autohide(1, false), Some(0), "tự ẩn đang bật (GETSTATE = 1) phải tắt được");
        assert_eq!(plan_autohide(0, true), Some(1));
        assert_eq!(plan_autohide(0, false), None);
        assert_eq!(plan_autohide(1, true), None);
        assert_eq!(plan_autohide(0x3, false), Some(0x2));
        assert_eq!(plan_autohide(0x2, true), Some(0x3));
    }
}
