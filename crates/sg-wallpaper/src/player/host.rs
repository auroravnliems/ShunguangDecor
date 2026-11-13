// Ported from TachyonWallpaper (idea originating from Lively Wallpaper, GPL-3.0).
//! The "host" window that holds the wallpaper content, a child of the desktop layer.

use std::num::NonZeroIsize;
use std::sync::Once;

use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, FillRect, GetStockObject, BLACK_BRUSH, HBRUSH, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IsWindow, LoadCursorW, MoveWindow, RegisterClassExW,
    SetLayeredWindowAttributes, ShowWindow, CS_HREDRAW, CS_VREDRAW, IDC_ARROW, LWA_ALPHA, SW_SHOWNA,
    WINDOW_EX_STYLE, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_PAINT, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_VISIBLE, MA_NOACTIVATE,
};

use sg_desktop::DesktopLayer as Desktop;

const HOST_CLASS: windows::core::PCWSTR = w!("ShunguangDecor.WallpaperHost");

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            FillRect(hdc, &ps.rcPaint, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        // Do not steal focus from Explorer when the user clicks the desktop.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(host_proc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: HOST_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
}

/// Creates the host window at `rect` (client coordinates of the desktop parent window).
pub fn create(desktop: &Desktop, rect: RECT) -> Result<HWND, String> {
    register();
    unsafe {
        let mut ex = WS_EX_NOACTIVATE;
        if desktop.raised {
            // Following Microsoft's guidance for the raised desktop: a layered child, alpha 255.
            ex |= WS_EX_LAYERED;
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(ex.0),
            HOST_CLASS,
            w!("ShunguangDecor Wallpaper"),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            Some(desktop.parent()),
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        )
        .map_err(|e| format!("Không tạo được cửa sổ hình nền: {e}"))?;
        if desktop.raised {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        }
        desktop.fix_zorder(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        logln!(
            "Host {:?} tại ({}, {}) {}x{}",
            hwnd.0,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top
        );
        Ok(hwnd)
    }
}

pub fn move_to(hwnd: HWND, rect: RECT) {
    unsafe {
        let _ = MoveWindow(hwnd, rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, true);
    }
}

pub fn destroy(hwnd: HWND) {
    unsafe {
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
    }
}

/// Wraps the HWND to hand it to wry (raw-window-handle).
pub struct HwndRef(pub HWND);

impl HasWindowHandle for HwndRef {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        let handle = Win32WindowHandle::new(raw);
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}
