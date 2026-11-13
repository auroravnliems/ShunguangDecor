//! Exercises label painting on a real comctl32 v6 `SysListView32` **inside the test process itself** (never touches Explorer).
//!
//! Run by hand: `cargo test -p sg-explorer-hook paint_harness -- --ignored --test-threads=1`
//! (run separately: the test shares the DLL's global state with `subclass_install_is_idempotent`).
//! Set `SG_HARNESS_OUT=<directory>` to write a `.ppm` image of each style for inspection.

use crate::desktop::test_support::{set_desired_only, setup_for_window};
use crate::desktop::{subclass_present, sync};
use sg_explorer_proto::{Backdrop, LabelsPaint, Outline, PaintMode, Shadow, TextStyle};
use std::sync::Arc;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, CreateSolidBrush,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, FillRect, GetDC, HBITMAP, HDC, HGDIOBJ, LOGFONTW, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::ApplicationInstallationAndServicing::{ACTCTXW, ActivateActCtx, CreateActCtxW};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    LIST_VIEW_ITEM_FLAGS, LVIF_TEXT, LVITEMW, LVM_INSERTITEMW, LVM_SETBKCOLOR, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETTEXTBKCOLOR,
    LVM_SETTEXTCOLOR, LVS_EX_DOUBLEBUFFER, LVS_EX_TRANSPARENTBKGND, LVS_EX_TRANSPARENTSHADOWTEXT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, HMENU, MSG, PM_REMOVE, PeekMessageW,
    RegisterClassW, SPI_GETICONTITLELOGFONT, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SystemParametersInfoW,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ERASEBKGND, WM_PRINTCLIENT, WM_SETFONT, WNDCLASSW, WS_CHILD,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::{PCWSTR, PWSTR, w};

const BG: u32 = 0x00_6080A0; // COLORREF (BGR): fake "wallpaper" background
const W: i32 = 420;
const H: i32 = 300;

unsafe extern "system" fn parent_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: the test window's wndproc; only paints the background.
    unsafe {
        if msg == WM_ERASEBKGND || msg == WM_PRINTCLIENT {
            let hdc = HDC(wp.0 as *mut core::ffi::c_void);
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let br = CreateSolidBrush(COLORREF(BG));
            FillRect(hdc, &rc, br);
            let _ = DeleteObject(HGDIOBJ(br.0));
            return LRESULT(1);
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}

fn activate_v6() {
    let dir = std::env::temp_dir().join("sg-harness");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("v6.manifest");
    std::fs::write(
        &path,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
</assembly>"#,
    )
    .unwrap();
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let ctx = ACTCTXW { cbSize: size_of::<ACTCTXW>() as u32, lpSource: PCWSTR(wide.as_ptr()), ..Default::default() };
    // SAFETY: creates and activates an activation context for the test thread (never deactivated: the test runs separately).
    unsafe {
        let h = CreateActCtxW(&ctx).expect("CreateActCtxW");
        let mut cookie = 0usize;
        ActivateActCtx(Some(h), &mut cookie).expect("ActivateActCtx");
        let _ = windows::Win32::System::LibraryLoader::LoadLibraryW(w!("comctl32.dll"));
    }
}

fn pump() {
    // SAFETY: a short message loop on the test thread.
    unsafe {
        let mut msg = MSG::default();
        for _ in 0..50 {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

/// Prints the window into a BGRA array.
fn shot(h: HWND) -> Vec<u8> {
    // SAFETY: the GDI objects created are released before returning.
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: W,
                biHeight: -H,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let dib: HBITMAP = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        let old = SelectObject(mem, HGDIOBJ(dib.0));
        let _ = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(0));
        let out = std::slice::from_raw_parts(bits as *const u8, (W * H * 4) as usize).to_vec();
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    }
}

fn save(name: &str, bgra: &[u8]) {
    let Ok(dir) = std::env::var("SG_HARNESS_OUT") else { return };
    let mut ppm = format!("P6\n{W} {H}\n255\n").into_bytes();
    for p in bgra.chunks(4) {
        ppm.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    std::fs::write(std::path::Path::new(&dir).join(format!("{name}.ppm")), ppm).unwrap();
}

fn uniform(style: TextStyle) -> Arc<LabelsPaint> {
    Arc::new(LabelsPaint { mode: PaintMode::Uniform, uniform: Some(style), items: vec![] })
}

fn render(parent: HWND, lv: HWND, p: Option<Arc<LabelsPaint>>) -> Vec<u8> {
    set_desired_only(p);
    sync();
    // SAFETY: repaints the test's listview.
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(lv), None, true);
    }
    pump();
    shot(parent)
}

/// Fraction of pixels whose color is close to `rgb` (per-channel deviation ≤ tol).
fn share_near(img: &[u8], rgb: u32, tol: i32) -> f64 {
    let (r, g, b) = (((rgb >> 16) & 0xFF) as i32, ((rgb >> 8) & 0xFF) as i32, (rgb & 0xFF) as i32);
    let n = img.chunks(4).filter(|p| (p[2] as i32 - r).abs() <= tol && (p[1] as i32 - g).abs() <= tol && (p[0] as i32 - b).abs() <= tol).count();
    n as f64 / (img.len() / 4) as f64
}

/// New ink outside the original glyph footprint: pixels that are background in `plain` but have ink in `over` (as a fraction of `over`'s total ink).
/// An overdrawn text that coincides exactly gives close to 0 (only slightly bolder where text already exists).
fn stray_ink(plain: &[u8], over: &[u8]) -> f64 {
    let bg = [(BG >> 16) as i32 & 0xFF, (BG >> 8) as i32 & 0xFF, BG as i32 & 0xFF]; // B, G, R trong DIB
    let is_bg = |p: &[u8]| (0..3).all(|i| (p[i] as i32 - bg[i]).abs() <= 6);
    let (mut stray, mut ink) = (0usize, 0usize);
    for (a, b) in plain.chunks(4).zip(over.chunks(4)) {
        if !is_bg(b) {
            ink += 1;
            if is_bg(a) {
                stray += 1;
            }
        }
    }
    stray as f64 / ink.max(1) as f64
}

fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
    let s: u64 = a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64).sum();
    s as f64 / a.len() as f64
}

#[test]
#[ignore]
fn paint_harness_real_listview_v6() {
    activate_v6();
    // SAFETY: creates the test window (off-screen, no focus) + a child v6 listview.
    let (parent, lv) = unsafe {
        let inst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW { lpfnWndProc: Some(parent_proc), hInstance: inst.into(), lpszClassName: w!("SgHarnessParent"), ..Default::default() };
        RegisterClassW(&wc);
        let parent = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("SgHarnessParent"),
            w!("sg-harness"),
            WS_POPUP | WS_VISIBLE,
            -4000,
            -4000,
            W,
            H,
            None,
            None,
            Some(inst.into()),
            None,
        )
        .unwrap();
        let lv = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SysListView32"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0x0000_0100 /* LVS_AUTOARRANGE */),
            0,
            0,
            W,
            H,
            Some(parent),
            Some(HMENU(1 as _)),
            Some(inst.into()),
            None,
        )
        .unwrap();
        let mut lf = LOGFONTW::default();
        SystemParametersInfoW(SPI_GETICONTITLELOGFONT, size_of::<LOGFONTW>() as u32, Some(&mut lf as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).unwrap();
        let font = CreateFontIndirectW(&lf);
        SendMessageW(lv, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let ex = LVS_EX_DOUBLEBUFFER | LVS_EX_TRANSPARENTBKGND | LVS_EX_TRANSPARENTSHADOWTEXT;
        SendMessageW(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, Some(WPARAM(ex as usize)), Some(LPARAM(ex as isize)));
        SendMessageW(lv, LVM_SETBKCOLOR, None, Some(LPARAM(-1))); // CLR_NONE like the desktop
        SendMessageW(lv, LVM_SETTEXTBKCOLOR, None, Some(LPARAM(-1)));
        SendMessageW(lv, LVM_SETTEXTCOLOR, None, Some(LPARAM(0xFFFFFF)));
        for (i, t) in ["Visual Studio Code", "recovery code minecraft backup", "Zalo", "task.txt"].iter().enumerate() {
            let mut buf: Vec<u16> = t.encode_utf16().chain(Some(0)).collect();
            let item = LVITEMW { mask: LIST_VIEW_ITEM_FLAGS(LVIF_TEXT.0), iItem: i as i32, pszText: PWSTR(buf.as_mut_ptr()), ..Default::default() };
            SendMessageW(lv, LVM_INSERTITEMW, None, Some(LPARAM(&item as *const LVITEMW as isize)));
        }
        (parent, lv)
    };
    setup_for_window(parent);
    pump();

    let base = render(parent, lv, None);
    save("0-default", &base);
    assert!(!subclass_present(parent));

    let pink = 0xFF69B4;
    let plain = render(parent, lv, Some(uniform(TextStyle::plain(pink))));
    save("1-pink", &plain);
    assert!(subclass_present(parent));
    let share_plain = share_near(&plain, pink, 40);
    println!("pink share {share_plain:.5} (default {:.5})", share_near(&base, pink, 40));
    assert!(share_plain > 0.001, "chữ phải đổi sang hồng qua clrText");

    // A nearly invisible effect → forces the text to be overdrawn. If our text coincides exactly with the listview's text, the image barely changes.
    let ghost = TextStyle { shadow: Some(Shadow { color: 0, alpha: 1, dx: 0, dy: 0 }), ..TextStyle::plain(pink) };
    let over = render(parent, lv, Some(uniform(ghost)));
    save("2-pink-overlay", &over);
    let d = stray_ink(&plain, &over);
    println!("overlay stray ink {d:.4} (mean diff {:.4})", mean_diff(&plain, &over));

    let outlined = TextStyle { outline: Some(Outline { color: 0x000000, width: 2, alpha: 255 }), ..TextStyle::plain(pink) };
    let o = render(parent, lv, Some(uniform(outlined)));
    save("3-pink-outline2", &o);
    println!("black share outline {:.5} vs plain {:.5}", share_near(&o, 0, 20), share_near(&plain, 0, 20));
    assert!(share_near(&o, 0, 20) > share_near(&plain, 0, 20) + 0.001, "viền đen phải hiện");

    let dark = TextStyle {
        outline: Some(Outline { color: 0xFFFFFF, width: 1, alpha: 170 }),
        ..TextStyle::plain(0x101010)
    };
    save("4-dark", &render(parent, lv, Some(uniform(dark))));
    let fancy = TextStyle {
        shadow: Some(Shadow { color: 0x000000, alpha: 160, dx: 2, dy: 2 }),
        backdrop: Some(Backdrop { color: 0x000000, alpha: 90 }),
        ..TextStyle::plain(0xFFFFFF)
    };
    save("5-shadow-backdrop", &render(parent, lv, Some(uniform(fancy))));

    let back = render(parent, lv, None);
    save("6-off", &back);
    assert!(!subclass_present(parent));
    assert!(mean_diff(&base, &back) < 0.01, "tắt là về đúng như cũ");
    assert!(d < 0.03, "chữ vẽ đè phải trùng chữ của listview (mực lệch {d})");
    // SAFETY: the test's window.
    unsafe {
        let _ = DestroyWindow(parent);
    }
}
