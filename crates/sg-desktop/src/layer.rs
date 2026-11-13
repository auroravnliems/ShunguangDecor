//! Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//!
//! Locates the desktop layer (Progman / WorkerW) so a window can be placed beneath the desktop icons.
//!
//! - Old style (Windows 10, Windows 11 before 24H2): send 0x052C to Progman so it creates a WorkerW
//!   behind the icons, then attach the window as a child of that WorkerW.
//! - "Raised desktop" (Windows 11 24H2+): Progman has WS_EX_NOREDIRECTIONBITMAP, SHELLDLL_DefView
//!   is a layered child of Progman, and WorkerW is a child of Progman at the very bottom. A
//!   WS_EX_LAYERED child window (alpha 255) of Progman is placed right below DefView and above WorkerW.

use crate::monitor::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GW_CHILD, GW_HWNDLAST, GWL_EXSTYLE, GetClassNameW, GetWindow,
    GetWindowLongPtrW, HWND_BOTTOM, IsWindow, SMTO_NORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SendMessageTimeoutW, SetWindowPos, WS_EX_NOREDIRECTIONBITMAP,
};
use windows::core::{BOOL, w};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopLayer {
    pub progman: HWND,
    pub workerw: HWND,
    pub defview: HWND,
    /// Windows 11 24H2+ ("raised desktop" with a layered ShellView).
    pub raised: bool,
}

struct Found {
    workerw: HWND,
    defview: HWND,
}

unsafe extern "system" fn find_defview(top: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` points to `Found` on the stack of `locate`, which lives for the whole EnumWindows call.
    let found = unsafe { &mut *(data.0 as *mut Found) };
    // SAFETY: the HWNDs are supplied by the system; the function tolerates windows that have disappeared.
    unsafe {
        if let Ok(p) = FindWindowExW(Some(top), None, w!("SHELLDLL_DefView"), None)
            && !p.is_invalid()
        {
            found.defview = p;
            // The WorkerW sits right after the window that contains DefView.
            if let Ok(ww) = FindWindowExW(None, Some(top), w!("WorkerW"), None) {
                found.workerw = ww;
            }
        }
    }
    true.into()
}

impl DesktopLayer {
    pub fn locate() -> Option<DesktopLayer> {
        // SAFETY: only calls window APIs with HWNDs just obtained; errors are all handled via Option.
        unsafe {
            let progman = FindWindowW(w!("Progman"), None).ok()?;
            if progman.is_invalid() {
                return None;
            }
            let ex = GetWindowLongPtrW(progman, GWL_EXSTYLE) as u32;
            let raised = ex & WS_EX_NOREDIRECTIONBITMAP.0 != 0;

            // Ask Progman to create a WorkerW behind the icons (does nothing if one already exists).
            SendMessageTimeoutW(progman, 0x052C, WPARAM(0xD), LPARAM(0x1), SMTO_NORMAL, 1000, None);

            let mut found = Found { workerw: HWND::default(), defview: HWND::default() };
            let _ = EnumWindows(Some(find_defview), LPARAM((&raw mut found) as isize));

            if raised {
                found.workerw = FindWindowExW(Some(progman), None, w!("WorkerW"), None).unwrap_or_default();
                if found.defview.is_invalid() {
                    found.defview =
                        FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None).unwrap_or_default();
                }
            }

            if found.workerw.is_invalid() && !raised {
                tracing::warn!(target: "sg::desktop", "WorkerW not found (Progman={:?})", progman.0);
                return None;
            }
            let d = DesktopLayer { progman, workerw: found.workerw, defview: found.defview, raised };
            tracing::debug!(
                target: "sg::desktop",
                "desktop layer: progman={:?} workerw={:?} defview={:?} raised={}",
                d.progman.0,
                d.workerw.0,
                d.defview.0,
                d.raised
            );
            Some(d)
        }
    }

    /// The parent window used to attach content and as the coordinate origin.
    pub fn parent(&self) -> HWND {
        if self.raised { self.progman } else { self.workerw }
    }

    pub fn is_valid(&self) -> bool {
        // SAFETY: IsWindow accepts any HWND value.
        unsafe { IsWindow(Some(self.parent())).as_bool() }
    }

    /// Converts a rectangle in screen coordinates to the parent window's client coordinates.
    pub fn screen_to_parent(&self, r: Rect) -> Rect {
        let mut tl = POINT { x: r.left, y: r.top };
        // SAFETY: tl is a valid local variable.
        unsafe {
            let _ = ScreenToClient(self.parent(), &mut tl);
        }
        Rect::new(tl.x, tl.y, tl.x + r.width(), tl.y + r.height())
    }

    /// After creating the child window: set the correct Z order (raised desktop: below DefView, above WorkerW).
    pub fn fix_zorder(&self, child: HWND) {
        if !self.raised {
            return;
        }
        // SAFETY: the HWNDs belong to Explorer/to us; if SetWindowPos fails, ignore it.
        unsafe {
            if !self.defview.is_invalid() {
                let _ = SetWindowPos(child, Some(self.defview), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            self.ensure_workerw_bottom();
        }
    }

    unsafe fn ensure_workerw_bottom(&self) {
        if self.workerw.is_invalid() {
            return;
        }
        // SAFETY: the calls only read the window tree; error results are turned into a null HWND.
        unsafe {
            // GW_HWNDLAST applied to a window returns its last sibling: take Progman's first child
            // then go to the last sibling to find which child is at the bottom.
            let first_child = GetWindow(self.progman, GW_CHILD).unwrap_or_default();
            let last_child =
                if first_child.is_invalid() { HWND::default() } else { GetWindow(first_child, GW_HWNDLAST).unwrap_or_default() };
            if last_child != self.workerw {
                tracing::info!(target: "sg::desktop", "WorkerW is not at the bottom, reordering");
                let _ = SetWindowPos(self.workerw, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    /// A window belonging to the desktop layer (used to tell whether the user is on the desktop).
    pub fn is_desktop_window(&self, hwnd: HWND) -> bool {
        hwnd == self.progman || hwnd == self.workerw || hwnd == self.defview || is_shell_desktop_class(hwnd)
    }

    /// The `SHELLDLL_DefView` window (contains the desktop icons).
    pub fn icon_view(&self) -> Option<HWND> {
        (!self.defview.is_invalid() && is_window(self.defview)).then_some(self.defview)
    }

    /// The `SysListView32` child of `SHELLDLL_DefView` (the real icon list).
    pub fn icon_list(&self) -> Option<HWND> {
        let view = self.icon_view()?;
        // SAFETY: view is a live window; on error → None.
        unsafe { FindWindowExW(Some(view), None, w!("SysListView32"), None).ok().filter(|h| !h.is_invalid()) }
    }
}

fn is_window(h: HWND) -> bool {
    // SAFETY: IsWindow accepts any HWND.
    unsafe { IsWindow(Some(h)).as_bool() }
}

fn is_shell_desktop_class(hwnd: HWND) -> bool {
    let c = class_name(hwnd);
    c == "WorkerW" || c == "Progman"
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = vec![0u16; 256];
    // SAFETY: buf is a valid buffer.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    buf.truncate(n.max(0) as usize);
    String::from_utf16_lossy(&buf)
}
