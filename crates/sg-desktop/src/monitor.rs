//! Ported from TachyonWallpaper (idea originally from Lively Wallpaper, GPL-3.0).
//!
//! Enumerates monitors. The number (1, 2, …) follows the device name `\\.\DISPLAYn`, so it matches the number
//! Windows shows when you click "Identify" in Display settings.

/// Rectangle in screen coordinates (pixels).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { left, top, right, bottom }
    }
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    /// Bounding box of two rectangles.
    pub fn union(&self, o: &Rect) -> Rect {
        Rect::new(self.left.min(o.left), self.top.min(o.top), self.right.max(o.right), self.bottom.max(o.bottom))
    }
}

impl From<windows::Win32::Foundation::RECT> for Rect {
    fn from(r: windows::Win32::Foundation::RECT) -> Rect {
        Rect::new(r.left, r.top, r.right, r.bottom)
    }
}

impl From<Rect> for windows::Win32::Foundation::RECT {
    fn from(r: Rect) -> windows::Win32::Foundation::RECT {
        windows::Win32::Foundation::RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    /// User-facing number, starting at 1 (from `\\.\DISPLAYn`).
    pub number: u32,
    /// Device name, e.g. `\\.\DISPLAY1`.
    pub device: String,
    pub rect: Rect,
    /// Work area (excluding the taskbar).
    pub work: Rect,
    pub primary: bool,
    pub dpi: u32,
    /// `HMONITOR` as a number (0 when built by hand in tests).
    pub handle: isize,
}

/// Finds a monitor by string: its number ("2"), "primary"/"chinh"/"chính" (the Vietnamese words for "main"), or the device name.
pub fn find<'a>(list: &'a [Monitor], key: &str) -> Option<&'a Monitor> {
    let k = key.trim();
    if k.eq_ignore_ascii_case("primary") || k.eq_ignore_ascii_case("chinh") || k.eq_ignore_ascii_case("chính") {
        return list.iter().find(|m| m.primary);
    }
    if let Ok(n) = k.parse::<u32>() {
        return list.iter().find(|m| m.number == n);
    }
    list.iter().find(|m| m.device.eq_ignore_ascii_case(k))
}

/// Number in the device name `\\.\DISPLAY12` → 12; no number → `u32::MAX` (sorted last).
pub fn display_number(device: &str) -> u32 {
    device
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(u32::MAX)
}

mod win {
    use super::{Monitor, Rect, display_number};
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };
    use windows::core::BOOL;

    const MONITORINFOF_PRIMARY: u32 = 1;

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    unsafe extern "system" fn enum_proc(hmon: HMONITOR, _hdc: HDC, _rc: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is a pointer to the Vec<Monitor> passed in by `monitors()` and stays alive during the enumeration.
        let list = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize is set to the correct, larger size.
        if unsafe { GetMonitorInfoW(hmon, (&raw mut info).cast::<MONITORINFO>()) }.as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            // SAFETY: hmon is valid during the enumeration; both destination variables exist.
            let _ = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
            list.push(Monitor {
                number: 0,
                device: from_wide(&info.szDevice),
                rect: info.monitorInfo.rcMonitor.into(),
                work: info.monitorInfo.rcWork.into(),
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                dpi: dx,
                handle: hmon.0 as isize,
            });
        }
        true.into()
    }

    pub fn monitors() -> Vec<Monitor> {
        let mut list: Vec<Monitor> = Vec::new();
        // SAFETY: the callback only runs during this call and `list` outlives it.
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(enum_proc), LPARAM((&raw mut list) as isize));
        }
        list.sort_by_key(|m| (display_number(&m.device), m.rect.left, m.rect.top));
        for (i, m) in list.iter_mut().enumerate() {
            m.number = i as u32 + 1;
        }
        list
    }

    /// The whole virtual screen (all monitors combined).
    pub fn virtual_screen() -> Rect {
        // SAFETY: GetSystemMetrics has no preconditions.
        unsafe {
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
            Rect::new(x, y, x + GetSystemMetrics(SM_CXVIRTUALSCREEN), y + GetSystemMetrics(SM_CYVIRTUALSCREEN))
        }
    }
}

pub use win::{monitors, virtual_screen};

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(number: u32, device: &str, primary: bool) -> Monitor {
        let r = Rect::new(0, 0, 1920, 1080);
        Monitor { number, device: device.into(), rect: r, work: r, primary, dpi: 96, handle: 0 }
    }

    #[test]
    fn rect_math() {
        let r = Rect::new(-1920, 0, 0, 1080);
        assert_eq!((r.width(), r.height()), (1920, 1080));
        let u = r.union(&Rect::new(0, 0, 2560, 1440));
        assert_eq!(u, Rect::new(-1920, 0, 2560, 1440));
        assert_eq!(Rect::default().width(), 0);
    }

    #[test]
    fn find_by_number_and_primary() {
        let list = vec![mon(1, r"\\.\DISPLAY1", false), mon(2, r"\\.\DISPLAY2", true)];
        assert_eq!(find(&list, "2").map(|m| m.number), Some(2));
        assert_eq!(find(&list, " primary ").map(|m| m.number), Some(2));
        assert_eq!(find(&list, "chính").map(|m| m.number), Some(2));
        assert_eq!(find(&list, r"\\.\display1").map(|m| m.number), Some(1));
        assert!(find(&list, "3").is_none());
        assert!(find(&list, "").is_none());
        assert!(find(&[], "primary").is_none());
    }

    #[test]
    fn display_numbers() {
        assert_eq!(display_number(r"\\.\DISPLAY1"), 1);
        assert_eq!(display_number(r"\\.\DISPLAY12"), 12);
        assert_eq!(display_number("abc"), u32::MAX);
    }

    #[test]
    #[ignore]
    fn real_monitors_nonempty() {
        let list = monitors();
        assert!(!list.is_empty());
        assert_eq!(list[0].number, 1);
        assert!(list.iter().any(|m| m.primary));
        assert!(find(&list, "1").is_some());
        assert!(virtual_screen().width() > 0);
    }
}
