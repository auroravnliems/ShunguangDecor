//! DWM thumbnail popup of a window, shown when hovering an app in the bar (the "Open apps" widget).
//! The popup is a regular DWM window (`WS_POPUP | TOOLWINDOW | TOPMOST | NOACTIVATE`, class `TachyonTaskbarThumb`
//! so `is_engine_window` filters it out of the app list), not a layered window drawn with
//! `UpdateLayeredWindow` (DWM does not draw thumbnails into that kind). The thumbnail is unregistered in `Drop`,
//! so every exit path (mouse leaves, click, window closes, bar reload/shutdown) only needs to drop the `Thumb` value.

use std::sync::Once;

use windows::core::{w, BOOL};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES,
    DWMWCP_ROUND, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
};
use windows::Win32::Graphics::Gdi::{GetStockObject, DKGRAY_BRUSH, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, LoadCursorW, RegisterClassExW, SetWindowPos, ShowWindow, HTTRANSPARENT, IDC_ARROW, MA_NOACTIVATE,
    SWP_NOACTIVATE, SW_SHOWNOACTIVATE, WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    HWND_TOPMOST,
};

/// Maximum thumbnail size (logical px) and the margin around it.
const MAX_W: f32 = 240.0;
const MAX_H: f32 = 160.0;
const PAD: f32 = 6.0;
/// Gap between the bar and the popup (logical px).
const GAP: f32 = 8.0;

/// Scales `src` down into the `max` box keeping the aspect ratio, never enlarging beyond the original size; each side is at least 1 px.
pub fn fit(src: (i32, i32), max: (i32, i32)) -> (i32, i32) {
    let (sw, sh) = (src.0.max(1) as f32, src.1.max(1) as f32);
    let k = (max.0 as f32 / sw).min(max.1 as f32 / sh).min(1.0);
    (((sw * k).round() as i32).max(1), ((sh * k).round() as i32).max(1))
}

/// Top-left position of a popup of `size`, centered on `anchor` (x, y, w, h), placed above (`above`) or below
/// it at distance `gap`, clamped inside the monitor `mon`.
pub fn place(anchor: (i32, i32, i32, i32), size: (i32, i32), above: bool, gap: i32, mon: RECT) -> (i32, i32) {
    let (ax, ay, aw, ah) = anchor;
    let x = ax + (aw - size.0) / 2;
    let y = if above { ay - gap - size.1 } else { ay + ah + gap };
    (x.clamp(mon.left, (mon.right - size.0).max(mon.left)), y.clamp(mon.top, (mon.bottom - size.1).max(mon.top)))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // the mouse passes through the popup: hover/click still belong to the bar underneath
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn ensure_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(GetStockObject(DKGRAY_BRUSH).0),
            lpszClassName: w!("TachyonTaskbarThumb"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
}

/// The thumbnail popup currently shown. `Drop` unregisters the thumbnail and then closes the window.
pub struct Thumb {
    hwnd: HWND,
    thumb: isize,
    source: isize,
}

impl Thumb {
    /// Opens the thumbnail popup of `source`, anchored to `anchor` (the app's rectangle on the bar, screen px).
    /// `above`: the bar is on the bottom edge so the popup sits above it. `None` if DWM refuses (the window already closed...).
    pub fn open(source: HWND, anchor: (i32, i32, i32, i32), above: bool, scale: f32) -> Option<Thumb> {
        ensure_class();
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                w!("TachyonTaskbarThumb"),
                w!("TachyonTaskbarThumb"),
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
            .ok()?;
            let mut t = Thumb { hwnd, thumb: 0, source: source.0 as isize };
            // from here every exit path (including `?`) drops `t` → full cleanup
            t.thumb = DwmRegisterThumbnail(hwnd, source).ok()?;
            let src: SIZE = DwmQueryThumbnailSourceSize(t.thumb).ok()?;
            let pad = (PAD * scale).round() as i32;
            let (tw, th) = fit((src.cx, src.cy), ((MAX_W * scale).round() as i32, (MAX_H * scale).round() as i32));
            let size = (tw + 2 * pad, th + 2 * pad);
            let mon = crate::engine::win::monitors()
                .into_iter()
                .map(|m| m.rect)
                .find(|r| {
                    let (cx, cy) = (anchor.0 + anchor.2 / 2, anchor.1 + anchor.3 / 2);
                    cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom
                })
                .unwrap_or(RECT { left: i32::MIN / 2, top: i32::MIN / 2, right: i32::MAX / 2, bottom: i32::MAX / 2 });
            let (x, y) = place(anchor, size, above, (GAP * scale).round() as i32, mon);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, size.0, size.1, SWP_NOACTIVATE);
            if crate::engine::win::windows_build() >= 22000 {
                crate::engine::win::set_corners(hwnd, DWMWCP_ROUND, "System");
            }
            let props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
                rcDestination: RECT { left: pad, top: pad, right: pad + tw, bottom: pad + th },
                opacity: 255,
                fVisible: BOOL(1),
                fSourceClientAreaOnly: BOOL(0),
                ..Default::default()
            };
            DwmUpdateThumbnailProperties(t.thumb, &props).ok()?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Some(t)
        }
    }

    /// The thumbnail's source window (so the popup can be closed when that window closes).
    pub fn source(&self) -> isize {
        self.source
    }

    #[cfg(test)]
    fn source_size(&self) -> Option<SIZE> {
        unsafe { DwmQueryThumbnailSourceSize(self.thumb).ok() }
    }
}

impl Drop for Thumb {
    fn drop(&mut self) {
        unsafe {
            if self.thumb != 0 {
                let _ = DwmUnregisterThumbnail(self.thumb);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit((1920, 1080), (240, 160)), (240, 135));
        assert_eq!(fit((400, 800), (240, 160)), (80, 160));
        assert_eq!(fit((100, 50), (240, 160)), (100, 50));
        assert_eq!(fit((0, 0), (240, 160)), (1, 1));
    }

    #[test]
    fn place_centers_above_or_below_and_clamps() {
        let mon = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        // bar at the bottom: the popup sits above the app
        assert_eq!(place((500, 1044, 40, 36), (252, 172), true, 8, mon), (394, 864));
        // bar at the top: it sits below the app
        assert_eq!(place((500, 0, 40, 36), (252, 172), false, 8, mon), (394, 44));
        // near the left/right edge it gets clamped
        assert_eq!(place((0, 1044, 40, 36), (252, 172), true, 8, mon).0, 0);
        assert_eq!(place((1900, 1044, 40, 36), (252, 172), true, 8, mon).0, 1920 - 252);
    }

    /// Needs a display + at least one open application window: `cargo test -p sg-taskbar -- --ignored thumbnail_popup_real`.
    #[test]
    #[ignore = "cần máy có giao diện và cửa sổ ứng dụng đang mở"]
    fn thumbnail_popup_real() {
        let wins = crate::engine::sys::apps::windows();
        let t = wins
            .iter()
            .find_map(|w| Thumb::open(HWND(w.hwnd as *mut _), (100, 1000, 48, 36), true, 1.0))
            .expect("không đăng ký được thumbnail cho cửa sổ nào");
        let size = t.source_size().expect("DwmQueryThumbnailSourceSize");
        assert!(size.cx > 0 && size.cy > 0);
        let popup = t.hwnd;
        assert!(unsafe { IsWindow(Some(popup)).as_bool() });
        drop(t);
        assert!(!unsafe { IsWindow(Some(popup)).as_bool() }, "popup phải bị đóng sau khi thả Thumb");
    }
}
