//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! Loads images via WIC into alpha-premultiplied BGRA Pixmaps: image files, HICON, exe file icons.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use windows::core::{HSTRING, GUID};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

use crate::engine::ui::tree::Pixmap;

type IconCache = HashMap<(String, u32), Option<Rc<Pixmap>>>;
type FitCache = HashMap<(String, u32, u32, Fit), Option<Rc<Pixmap>>>;

thread_local! {
    static WIC: RefCell<Option<IWICImagingFactory>> = const { RefCell::new(None) };
    static EXE_ICONS: RefCell<IconCache> = RefCell::new(HashMap::new());
}

fn wic() -> Option<IWICImagingFactory> {
    WIC.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            *w = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };
        }
        w.clone()
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fit {
    Stretch,
    /// fill completely then crop; crop position in thousandths (0..=1000)
    Cover(u16, u16),
    /// fit inside, keeping the aspect ratio (the returned image may be smaller than w×h)
    Contain,
}

/// Convert a WIC source to a `w`×`h` Pixmap (center-cropped if `cover`).
fn to_pixmap(f: &IWICImagingFactory, src: &IWICBitmapSource, w: u32, h: u32, cover: bool) -> Option<Rc<Pixmap>> {
    to_pixmap_fit(f, src, w, h, if cover { Fit::Cover(500, 500) } else { Fit::Stretch })
}

fn to_pixmap_fit(f: &IWICImagingFactory, src: &IWICBitmapSource, w: u32, h: u32, fit: Fit) -> Option<Rc<Pixmap>> {
    unsafe {
        let (mut sw, mut sh) = (0u32, 0u32);
        src.GetSize(&mut sw, &mut sh).ok()?;
        if sw == 0 || sh == 0 || w == 0 || h == 0 {
            return None;
        }
        let (w, h) = if fit == Fit::Contain {
            let k = (w as f32 / sw as f32).min(h as f32 / sh as f32);
            (((sw as f32 * k).round() as u32).max(1), ((sh as f32 * k).round() as u32).max(1))
        } else {
            (w, h)
        };
        let cover = matches!(fit, Fit::Cover(..));
        let (px, py) = match fit {
            Fit::Cover(x, y) => (x as f32 / 1000.0, y as f32 / 1000.0),
            _ => (0.5, 0.5),
        };
        let (tw, th) = if cover {
            let k = (w as f32 / sw as f32).max(h as f32 / sh as f32);
            (((sw as f32 * k).round() as u32).max(w), ((sh as f32 * k).round() as u32).max(h))
        } else {
            (w, h)
        };
        let scaled: IWICBitmapSource = if (tw, th) != (sw, sh) {
            let sc = f.CreateBitmapScaler().ok()?;
            sc.Initialize(src, tw, th, WICBitmapInterpolationModeHighQualityCubic).ok()?;
            sc.into()
        } else {
            src.clone()
        };
        let conv = f.CreateFormatConverter().ok()?;
        conv.Initialize(&scaled, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let rect = WICRect { X: ((tw - w) as f32 * px) as i32, Y: ((th - h) as f32 * py) as i32, Width: w as i32, Height: h as i32 };
        let mut data = vec![0u8; (w * h * 4) as usize];
        conv.CopyPixels(&rect, w * 4, &mut data).ok()?;
        Some(Pixmap::new(w, h, data))
    }
}

pub fn load_file(path: &str, w: u32, h: u32, cover: bool) -> Option<Rc<Pixmap>> {
    let f = wic()?;
    unsafe {
        let dec = f
            .CreateDecoderFromFilename(&HSTRING::from(path), Some(&GUID::zeroed()), GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = dec.GetFrame(0).ok()?;
        to_pixmap(&f, &frame.into(), w, h, cover)
    }
}

/// Image from a file using a frame-fitting mode, cached (used for the stylesheet's `image:`/`background-image`).
pub fn load_file_fit(path: &str, w: u32, h: u32, fit: Fit) -> Option<Rc<Pixmap>> {
    thread_local! {
        static CACHE: RefCell<FitCache> = RefCell::new(HashMap::new());
    }
    let key = (path.to_string(), w, h, fit);
    if let Some(v) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return v;
    }
    let p = (|| {
        let f = wic()?;
        unsafe {
            let dec = f
                .CreateDecoderFromFilename(&HSTRING::from(path), Some(&GUID::zeroed()), GENERIC_READ, WICDecodeMetadataCacheOnDemand)
                .ok()?;
            let frame = dec.GetFrame(0).ok()?;
            to_pixmap_fit(&f, &frame.into(), w, h, fit)
        }
    })();
    if p.is_none() {
        crate::engine::log::warn(&format!("Không nạp được ảnh '{path}'"));
    }
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 400 {
            c.clear();
        }
        c.insert(key, p.clone());
    });
    p
}

pub fn from_hicon(icon: HICON, size: u32) -> Option<Rc<Pixmap>> {
    let f = wic()?;
    unsafe {
        let b = f.CreateBitmapFromHICON(icon).ok()?;
        to_pixmap(&f, &b.into(), size, size, false)
    }
}

/// 32-bit HBITMAP with alpha (from IShellItemImageFactory) → Pixmap; frees the HBITMAP.
pub fn from_hbitmap(hbm: windows::Win32::Graphics::Gdi::HBITMAP, size: u32) -> Option<Rc<Pixmap>> {
    let f = wic()?;
    let r = unsafe {
        f.CreateBitmapFromHBITMAP(hbm, windows::Win32::Graphics::Gdi::HPALETTE::default(), WICBitmapUsePremultipliedAlpha)
            .ok()
            .and_then(|b| to_pixmap(&f, &b.into(), size, size, false))
    };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(hbm.into());
    }
    r
}

/// Icon of an exe file (cached).
pub fn exe_icon(path: &str, size: u32) -> Option<Rc<Pixmap>> {
    if path.is_empty() {
        return None;
    }
    let key = (path.to_ascii_lowercase(), size);
    if let Some(v) = EXE_ICONS.with(|m| m.borrow().get(&key).cloned()) {
        return v;
    }
    let pix = unsafe {
        let mut info = SHFILEINFOW::default();
        let flags = SHGFI_ICON | if size > 16 { SHGFI_LARGEICON } else { SHGFI_SMALLICON };
        let r = SHGetFileInfoW(&HSTRING::from(path), Default::default(), Some(&mut info), std::mem::size_of::<SHFILEINFOW>() as u32, flags);
        if r != 0 && !info.hIcon.is_invalid() {
            let p = from_hicon(info.hIcon, size);
            let _ = DestroyIcon(info.hIcon);
            p
        } else {
            None
        }
    };
    EXE_ICONS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 300 {
            m.clear();
        }
        m.insert(key, pix.clone());
    });
    pix
}
