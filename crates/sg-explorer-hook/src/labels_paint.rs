//! Subclass of `SHELLDLL_DefView`: colors desktop text through the `NM_CUSTOMDRAW` of the child `SysListView32`.
//!
//! - `CDDS_PREPAINT` → ask for `CDRF_NOTIFYITEMDRAW` as well.
//! - `CDDS_ITEMPREPAINT` → set `clrText` according to the item's style (+ `CDRF_NEWFONT`); if there is an outline/shadow/translucent
//!   background, ask for `CDRF_NOTIFYPOSTPAINT`.
//! - `CDDS_ITEMPOSTPAINT` → redraw the label in place on top: translucent background → shadow → outline → text, with a single
//!   premultiplied image via `AlphaBlend` (see `compose.rs`). Selected / being-renamed items are not painted over.
//!
//! Always calls `DefSubclassProc` first so DefView does its part, then only adds flags / changes colors.
//! Runs only on the desktop thread. Holds no locks; the paint buffers are reused between calls (no allocation per paint).

use crate::compose;
use crate::desktop::{self, def_subclass, fault_here, sg_msg};
use sg_explorer_proto::{LabelsPaint, TextStyle};
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, AlphaBlend, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CENTER, DT_EDITCONTROL,
    DT_END_ELLIPSIS, DT_NOPREFIX, DT_TOP, DT_WORD_ELLIPSIS, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW, GdiFlush, GetObjectW, HBITMAP, HDC, HFONT, HGDIOBJ,
    LOGFONTW, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPOSTPAINT, CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDRF_NEWFONT, CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYPOSTPAINT,
    CDRF_SKIPDEFAULT, LIST_VIEW_ITEM_FLAGS, LVIR_LABEL, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_GETEDITCONTROL,
    LVM_GETITEMRECT, LVM_GETITEMSTATE, LVM_GETITEMTEXTW, LVM_GETVIEW, LVN_DELETEALLITEMS, LVN_DELETEITEM,
    LVN_ENDLABELEDITW, LVN_INSERTITEM, LVN_ITEMCHANGED, LV_VIEW_ICON, NM_CUSTOMDRAW, NMHDR, NMLVCUSTOMDRAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXEDGE, SendMessageW, WM_DPICHANGED, WM_GETFONT, WM_NCDESTROY, WM_NOTIFY, WM_SETTINGCHANGE, WM_SIZE, WM_THEMECHANGED,
    WM_WINDOWPOSCHANGED,
};
use windows::core::PWSTR;

/// Fixed text buffer (the longest file name Windows allows is 260 characters; doubled to be safe).
const TEXT_BUF: usize = 520;
/// Labels larger than this (abnormal) get no effects drawn.
const MAX_W: i32 = 1024;
const MAX_H: i32 = 512;
/// Flags for drawing multi-line labels like a listview in icon mode (verified with `paint_harness`: each word that does not fit
/// is cut with "…", like "recov… / code …"; the same flag set Wine uses for listview).
const LABEL_DT: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(
    DT_TOP.0 | DT_NOPREFIX.0 | DT_EDITCONTROL.0 | DT_CENTER.0 | DT_WORDBREAK.0 | DT_WORD_ELLIPSIS.0 | DT_END_ELLIPSIS.0,
);

thread_local! {
    /// Coloring table currently applied on the desktop thread (only the desktop thread reads/writes it).
    static CURRENT: RefCell<Option<Arc<LabelsPaint>>> = const { RefCell::new(None) };
    static CACHE: RefCell<PaintCache> = RefCell::new(PaintCache::default());
    /// Currently inside an effect paint (blocks nested calls).
    static IN_EFFECTS: Cell<bool> = const { Cell::new(false) };
}

/// Changes the coloring table in effect. Returns `true` if it differs from the old table (a repaint is needed).
pub(crate) fn set_current(p: Option<Arc<LabelsPaint>>) -> bool {
    let free = p.is_none();
    let changed = CURRENT.with(|c| match c.try_borrow_mut() {
        Ok(mut cur) => {
            let same = match (&*cur, &p) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            *cur = p;
            !same
        }
        Err(_) => false,
    });
    if free {
        // Fully off: release the GDI buffers.
        CACHE.with(|c| {
            if let Ok(mut c) = c.try_borrow_mut() {
                c.release();
            }
        });
    }
    changed
}

fn current() -> Option<Arc<LabelsPaint>> {
    CURRENT.with(|c| c.try_borrow().ok().and_then(|c| c.clone()))
}

fn interesting(msg: u32) -> bool {
    matches!(msg, WM_NOTIFY | WM_SIZE | WM_WINDOWPOSCHANGED | WM_DPICHANGED | WM_SETTINGCHANGE | WM_THEMECHANGED | WM_NCDESTROY)
        || msg == sg_msg()
}

/// Subclass proc on DefView.
///
/// # Safety
/// Only called by comctl32 (via `SetWindowSubclass`), on the thread that owns `hwnd`.
pub(crate) unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // Fast path: unrelated messages go straight down to DefView without running any other Rust code.
    if !interesting(msg) {
        return def_subclass(hwnd, msg, wparam, lparam);
    }
    let def_result: Cell<Option<LRESULT>> = Cell::new(None);
    match catch_unwind(AssertUnwindSafe(|| handle(hwnd, msg, wparam, lparam, &def_result))) {
        Ok(r) => r,
        Err(e) => {
            let m = crate::guard::panic_message(e.as_ref());
            crate::log::push(format!("panic in subclass proc (msg {msg:#x}): {m}"));
            fault_here(m);
            // DefView has already handled the message, so do not call it a second time.
            match def_result.get() {
                Some(r) => r,
                None if msg == sg_msg() => LRESULT(0),
                None => def_subclass(hwnd, msg, wparam, lparam),
            }
        }
    }
}

fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, def_result: &Cell<Option<LRESULT>>) -> LRESULT {
    let def = || {
        let r = def_subclass(hwnd, msg, wparam, lparam);
        def_result.set(Some(r));
        r
    };
    if msg == sg_msg() {
        desktop::sync();
        return LRESULT(0);
    }
    match msg {
        WM_NOTIFY if lparam.0 != 0 => {
            // SAFETY: WM_NOTIFY always carries a valid NMHDR pointer for the duration of the call.
            let hdr = unsafe { &*(lparam.0 as *const NMHDR) };
            if !is_our_listview(hwnd, hdr.hwndFrom) {
                return def();
            }
            match hdr.code {
                NM_CUSTOMDRAW => custom_draw(lparam, def),
                LVN_ITEMCHANGED | LVN_INSERTITEM | LVN_DELETEITEM | LVN_DELETEALLITEMS | LVN_ENDLABELEDITW => {
                    desktop::mark_layout_dirty();
                    def()
                }
                _ => def(),
            }
        }
        WM_NCDESTROY => {
            // SetWindowSubclass contract: when the window is destroyed, the subclass is removed automatically.
            desktop::remove_subclass(hwnd);
            set_current(None);
            desktop::DEFVIEW.store(0, Ordering::SeqCst);
            desktop::LISTVIEW.store(0, Ordering::SeqCst);
            crate::log::push("desktop view destroyed; subclass removed");
            def()
        }
        _ => {
            desktop::mark_layout_dirty();
            def()
        }
    }
}

fn is_our_listview(defview: HWND, from: HWND) -> bool {
    let cached = desktop::LISTVIEW.load(Ordering::SeqCst);
    if cached != 0 && from.0 as isize == cached {
        return true;
    }
    // New listview (DefView rebuilt its listview): accept it if it has the right class and is a child of this DefView.
    if desktop::is_window(from) && desktop::class_is(from, "SysListView32") && desktop::listview_of(defview) == Some(from) {
        return true;
    }
    false
}

fn colorref(rgb: u32) -> COLORREF {
    // 0xRRGGBB → COLORREF 0x00BBGGRR
    COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF))
}

fn custom_draw(lparam: LPARAM, def: impl FnOnce() -> LRESULT) -> LRESULT {
    let res = def();
    let Some(paint) = current() else { return res };
    let cd = lparam.0 as *mut NMLVCUSTOMDRAW;
    // SAFETY: the listview's NM_CUSTOMDRAW carries a valid NMLVCUSTOMDRAW for the duration of the call; only standard fields are read/written.
    let (stage, index) = unsafe { ((*cd).nmcd.dwDrawStage, (*cd).nmcd.dwItemSpec as u32) };
    let flags = res.0 as u32;
    if stage == CDDS_PREPAINT {
        return LRESULT((flags | CDRF_NOTIFYITEMDRAW) as isize);
    }
    if flags & CDRF_SKIPDEFAULT != 0 {
        return res;
    }
    let Some(style) = paint.style_for(index).copied() else { return res };
    if stage == CDDS_ITEMPREPAINT {
        // SAFETY: as above; clrText is the field the listview reads back when we return CDRF_NEWFONT.
        unsafe {
            (*cd).clrText = colorref(style.color);
        }
        let mut r = flags | CDRF_NEWFONT;
        if style.has_effects() {
            r |= CDRF_NOTIFYPOSTPAINT;
        }
        return LRESULT(r as isize);
    }
    if stage == CDDS_ITEMPOSTPAINT && style.has_effects() {
        // SAFETY: as above.
        let (hdc, lv) = unsafe { ((*cd).nmcd.hdc, (*cd).nmcd.hdr.hwndFrom) };
        draw_effects(hdc, lv, index, &style);
    }
    res
}

/// Redraws the label of item `index` along with its effects. Silently skips when anything is abnormal.
fn draw_effects(hdc: HDC, lv: HWND, index: u32, style: &TextStyle) {
    if hdc.is_invalid() || IN_EFFECTS.with(Cell::get) {
        return;
    }
    IN_EFFECTS.with(|c| c.set(true));
    draw_effects_inner(hdc, lv, index, style);
    IN_EFFECTS.with(|c| c.set(false));
}

fn draw_effects_inner(hdc: HDC, lv: HWND, index: u32, style: &TextStyle) {
    // Collect everything needed from the listview first (it may call back into DefView, e.g. LVN_GETDISPINFO),
    // and only then borrow the paint buffers.
    // SAFETY: every call is a same-thread SendMessageW to the listview, with parameters pointing to live local variables.
    let (rc, text, n, font) = unsafe {
        if SendMessageW(lv, LVM_GETVIEW, None, None).0 as u32 != LV_VIEW_ICON {
            return;
        }
        let st = SendMessageW(lv, LVM_GETITEMSTATE, Some(WPARAM(index as usize)), Some(LPARAM((LVIS_SELECTED.0 | LVIS_FOCUSED.0) as isize))).0 as u32;
        if st & LVIS_SELECTED.0 != 0 {
            return;
        }
        if st & LVIS_FOCUSED.0 != 0 && SendMessageW(lv, LVM_GETEDITCONTROL, None, None).0 != 0 {
            return;
        }
        let mut rc = RECT { left: LVIR_LABEL as i32, ..Default::default() };
        if SendMessageW(lv, LVM_GETITEMRECT, Some(WPARAM(index as usize)), Some(LPARAM(&mut rc as *mut RECT as isize))).0 == 0 {
            return;
        }
        let mut text = [0u16; TEXT_BUF];
        let mut item = LVITEMW {
            mask: LIST_VIEW_ITEM_FLAGS(0),
            iSubItem: 0,
            pszText: PWSTR(text.as_mut_ptr()),
            cchTextMax: TEXT_BUF as i32,
            ..Default::default()
        };
        let n = SendMessageW(lv, LVM_GETITEMTEXTW, Some(WPARAM(index as usize)), Some(LPARAM(&mut item as *mut LVITEMW as isize))).0;
        let n = (n.max(0) as usize).min(TEXT_BUF - 1);
        // The listview may return text in its own buffer (pszText changed): copy it into our buffer.
        if !item.pszText.is_null() && item.pszText.0 != text.as_mut_ptr() {
            let src = std::slice::from_raw_parts(item.pszText.0, n);
            text[..n].copy_from_slice(src);
        }
        let font = HFONT(SendMessageW(lv, WM_GETFONT, None, None).0 as *mut core::ffi::c_void);
        (rc, text, n, font)
    };
    if n == 0 {
        return;
    }
    let (lw, lh) = (rc.right - rc.left, rc.bottom - rc.top);
    if lw <= 0 || lh <= 0 || lw > MAX_W || lh > MAX_H {
        return;
    }
    let m = compose::margin(style);
    let (w, h) = ((lw + 2 * m) as usize, (lh + 2 * m) as usize);
    // The listview draws text inside the label area inset by one border (SM_CXEDGE) on each side — measured with `paint_harness`.
    // SAFETY: no pointer parameters.
    let inset = unsafe { GetSystemMetrics(SM_CXEDGE) }.clamp(0, lw / 4);
    CACHE.with(|c| {
        let Ok(mut cache) = c.try_borrow_mut() else { return };
        let mut text = text;
        let label = (m, m, m + lw, m + lh);
        cache.draw(hdc, &mut text[..n], font, (rc.left - m, rc.top - m), (w, h), label, (label.0 + inset, label.2 - inset), style);
    });
}

/// GDI + memory buffers reused between paints (one instance on the desktop thread).
#[derive(Default)]
struct PaintCache {
    dc: Option<HDC>,
    dib: Option<HBITMAP>,
    old_bmp: Option<HGDIOBJ>,
    bits: usize,
    dib_w: usize,
    dib_h: usize,
    src_font: isize,
    gray_font: Option<HFONT>,
    mask: Vec<u8>,
    scratch: Vec<u8>,
}

impl PaintCache {
    fn release(&mut self) {
        // SAFETY: only deletes GDI objects created by the cache itself; the bitmap is deselected before deletion.
        unsafe {
            if let (Some(dc), Some(old)) = (self.dc, self.old_bmp.take()) {
                SelectObject(dc, old);
            }
            if let Some(b) = self.dib.take() {
                let _ = DeleteObject(HGDIOBJ(b.0));
            }
            if let Some(dc) = self.dc.take() {
                let _ = DeleteDC(dc);
            }
            if let Some(f) = self.gray_font.take() {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
        }
        self.bits = 0;
        self.dib_w = 0;
        self.dib_h = 0;
        self.src_font = 0;
        self.mask = Vec::new();
        self.scratch = Vec::new();
    }

    /// A 32-bit (top-down) DIB of at least `w × h`; only recreated when a larger one is needed.
    fn ensure_dib(&mut self, w: usize, h: usize) -> bool {
        if self.dib.is_some() && self.dib_w >= w && self.dib_h >= h && self.bits != 0 {
            return true;
        }
        let (nw, nh) = (w.max(self.dib_w).next_multiple_of(64), h.max(self.dib_h).next_multiple_of(32));
        // SAFETY: creates our own DC/DIB; the old bitmap is deselected and deleted before being replaced.
        unsafe {
            if self.dc.is_none() {
                let dc = CreateCompatibleDC(None);
                if dc.is_invalid() {
                    return false;
                }
                self.dc = Some(dc);
            }
            let dc = self.dc.unwrap_or_default();
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: nw as i32,
                    biHeight: -(nh as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(dib) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else { return false };
            if bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(dib.0));
                return false;
            }
            let prev = SelectObject(dc, HGDIOBJ(dib.0));
            match self.old_bmp {
                // An old DIB was already selected: `prev` is that one → delete it.
                Some(_) => {
                    let _ = DeleteObject(prev);
                }
                None => self.old_bmp = Some(prev),
            }
            self.dib = Some(dib);
            self.bits = bits as usize;
            self.dib_w = nw;
            self.dib_h = nh;
        }
        true
    }

    /// A copy of the listview's font with grayscale antialiasing (coverage can be read from a single color channel).
    fn gray_font(&mut self, src: HFONT) -> Option<HFONT> {
        if src.is_invalid() {
            return None;
        }
        if self.src_font == src.0 as isize && self.gray_font.is_some() {
            return self.gray_font;
        }
        // SAFETY: reads the LOGFONT of the listview's font; creates our own new font and deletes our old one.
        unsafe {
            let mut lf = LOGFONTW::default();
            if GetObjectW(HGDIOBJ(src.0), size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut LOGFONTW as *mut _)) == 0 {
                return None;
            }
            lf.lfQuality = ANTIALIASED_QUALITY;
            let f = CreateFontIndirectW(&lf);
            if f.is_invalid() {
                return None;
            }
            if let Some(old) = self.gray_font.replace(f) {
                let _ = DeleteObject(HGDIOBJ(old.0));
            }
            self.src_font = src.0 as isize;
            Some(f)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        target: HDC,
        text: &mut [u16],
        font: HFONT,
        at: (i32, i32),
        size: (usize, usize),
        label: (i32, i32, i32, i32),
        text_x: (i32, i32),
        style: &TextStyle,
    ) {
        let (w, h) = size;
        let Some(font) = self.gray_font(font) else { return };
        if !self.ensure_dib(w, h) {
            return;
        }
        let Some(dc) = self.dc else { return };
        let stride = self.dib_w * 4;
        // SAFETY: `bits` is the memory of the live DIB, sized dib_w*dib_h*4; every index below is < w*4 + (h-1)*stride.
        let px = unsafe { std::slice::from_raw_parts_mut(self.bits as *mut u8, stride * self.dib_h) };
        for y in 0..h {
            px[y * stride..y * stride + w * 4].fill(0);
        }
        // SAFETY: draws white text onto our DIB.
        unsafe {
            let old_font = SelectObject(dc, HGDIOBJ(font.0));
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            SetBkMode(dc, TRANSPARENT);
            let mut r = RECT { left: text_x.0, top: label.1, right: text_x.1, bottom: label.3 };
            DrawTextW(dc, text, &mut r, LABEL_DT);
            SelectObject(dc, old_font);
            let _ = GdiFlush();
        }
        self.mask.clear();
        self.mask.reserve(w * h);
        for y in 0..h {
            for x in 0..w {
                // The green channel = coverage (the font uses grayscale antialiasing, so R = G = B).
                self.mask.push(px[y * stride + x * 4 + 1]);
            }
        }
        compose::compose(compose::Mask { data: &self.mask, w, h }, label, style, &mut self.scratch, px, stride);
        // SAFETY: the source is our premultiplied DIB; the destination is the DC the listview hands to custom draw.
        unsafe {
            let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = AlphaBlend(target, at.0, at.1, w as i32, h as i32, dc, 0, 0, w as i32, h as i32, bf);
        }
    }
}

impl Drop for PaintCache {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::test_support::{set_desired_only, setup_for_window};
    use crate::desktop::{INSTALLED, install_subclass, remove_subclass, subclass_present, sync};
    use sg_explorer_proto::PaintMode;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPED};
    use windows::core::w;

    #[test]
    fn colorref_swaps_channels() {
        assert_eq!(colorref(0x112233).0, 0x332211);
        assert_eq!(colorref(0xFF0000).0, 0x0000FF);
    }

    /// Toggle on/off 20 times: DefView has exactly one subclass of ours, and turning off removes it entirely (counted via `GetWindowSubclass`).
    #[test]
    fn subclass_install_is_idempotent() {
        // SAFETY: creates a hidden window owned by the test process itself, on the test thread (playing the desktop thread).
        let h = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("sg-test"), WS_OVERLAPPED, 0, 0, 10, 10, None, None, None, None)
        }
        .unwrap();
        setup_for_window(h);
        for _ in 0..20 {
            assert!(install_subclass(h));
            assert!(install_subclass(h), "gắn lần hai không tạo subclass thứ hai");
            assert!(subclass_present(h));
            remove_subclass(h);
            assert!(!subclass_present(h), "gỡ một lần là hết");
        }
        // Through the real path (sync to the desired coloring table).
        let p = Arc::new(LabelsPaint { mode: PaintMode::Uniform, uniform: Some(TextStyle::plain(0xFF00FF)), items: vec![] });
        for i in 0..20 {
            set_desired_only(Some(if i % 2 == 0 { p.clone() } else { Arc::new((*p).clone()) }));
            sync();
            assert!(INSTALLED.load(Ordering::SeqCst) && subclass_present(h));
            set_desired_only(None);
            sync();
            assert!(!INSTALLED.load(Ordering::SeqCst) && !subclass_present(h));
        }
        assert!(current().is_none());
        // SAFETY: the test's window.
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}
