//! Captures the desktop layer with PrintWindow (method a), composes the live-wallpaper frame (b), falls back to the static wallpaper (c).

use crate::{has_content, icons_to_alpha, wallpaper_frame, window_frame};
use image::RgbaImage;
use sg_desktop::{DesktopLayer, Monitor, Rect};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDC, HBITMAP, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

static METHOD: AtomicU8 = AtomicU8::new(0);

pub fn last_method() -> &'static str {
    match METHOD.load(Ordering::Relaxed) {
        1 => "printwindow-progman+icons",
        2 => "wallpaper-frame+icons",
        3 => "static-wallpaper",
        _ => "none",
    }
}

/// PrintWindow the whole `hwnd` window into an RGBA image (size = the window rectangle).
/// Also returns the window's top-left corner in screen coordinates.
fn print_window(hwnd: HWND) -> Option<(RgbaImage, (i32, i32))> {
    let mut r = RECT::default();
    // SAFETY: hwnd was supplied by the system; r is a local variable.
    unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if !(1..=16384).contains(&w) || !(1..=16384).contains(&h) {
        return None;
    }
    // SAFETY: every GDI object created is released before leaving the block; `bits` is only read while the DIB is alive.
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let dib: Option<HBITMAP> = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        let result = dib.and_then(|dib| {
            let old = SelectObject(mem, HGDIOBJ(dib.0));
            // PW_RENDERFULLCONTENT = 2: also capture DirectComposition content.
            let ok = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(2)).as_bool();
            let img = if ok && !bits.is_null() {
                let src = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                let mut rgba = Vec::with_capacity(src.len());
                for p in src.as_chunks::<4>().0 {
                    rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
                RgbaImage::from_raw(w as u32, h as u32, rgba)
            } else {
                None
            };
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(dib.0));
            img
        });
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result.map(|i| (i, (r.left, r.top)))
    }
}

/// Crops the `rect` region (screen coordinates) out of an image whose top-left corner is at `origin`.
fn crop(img: &RgbaImage, origin: (i32, i32), rect: Rect) -> Option<RgbaImage> {
    let x = rect.left - origin.0;
    let y = rect.top - origin.1;
    if x < 0 || y < 0 || rect.width() <= 0 || rect.height() <= 0 {
        return None;
    }
    let (x, y, w, h) = (x as u32, y as u32, rect.width() as u32, rect.height() as u32);
    if x + w > img.width() || y + h > img.height() {
        return None;
    }
    Some(image::imageops::crop_imm(img, x, y, w, h).to_image())
}

fn static_wallpaper(monitor: &Monitor) -> Option<RgbaImage> {
    let w = sg_core::desktop::current_wallpaper()?;
    let img = image::ImageReader::open(&w.path).ok()?.with_guessed_format().ok()?.decode().ok()?.to_rgba8();
    // Windows defaults to "Fill": crop to match the screen aspect ratio.
    let (tw, th) = (monitor.rect.width().max(1) as f32, monitor.rect.height().max(1) as f32);
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let scale = (tw / iw).max(th / ih);
    let (cw, ch) = (((tw / scale).round() as u32).min(img.width()).max(1), ((th / scale).round() as u32).min(img.height()).max(1));
    let (cx, cy) = ((img.width() - cw) / 2, (img.height() - ch) / 2);
    Some(image::imageops::crop_imm(&img, cx, cy, cw, ch).to_image())
}

/// Overlays `icons` (black background = transparent) onto `base`: non-black pixels of icons win.
fn overlay_icons(base: &mut RgbaImage, icons: &RgbaImage) {
    if base.dimensions() != icons.dimensions() {
        return;
    }
    for (b, i) in base.pixels_mut().zip(icons.pixels()) {
        if i[0] > 12 || i[1] > 12 || i[2] > 12 {
            *b = *i;
        }
    }
}

/// Target size when downscaling to `max_width` while keeping the aspect ratio.
fn target_size(w: u32, h: u32, max_width: u32) -> (u32, u32) {
    if max_width == 0 || w <= max_width { (w, h) } else { (max_width, ((h as u64 * max_width as u64) / w as u64).max(1) as u32) }
}

fn fit_to(img: RgbaImage, size: (u32, u32)) -> RgbaImage {
    if img.dimensions() == size { img } else { image::imageops::resize(&img, size.0, size.1, image::imageops::FilterType::Triangle) }
}

/// The icon layer changes very slowly but printing it is expensive: keep it for a few seconds per (monitor, size).
const ICON_TTL: std::time::Duration = std::time::Duration::from_secs(4);
type IconCache = Option<(std::time::Instant, String, (u32, u32), Option<RgbaImage>)>;
static ICONS: Mutex<IconCache> = Mutex::new(None);

fn icon_layer(layer: Option<DesktopLayer>, monitor: &Monitor, size: (u32, u32)) -> Option<RgbaImage> {
    let mut g = ICONS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, dev, sz, img)) = g.as_ref()
        && at.elapsed() < ICON_TTL
        && dev == &monitor.device
        && *sz == size
    {
        return img.clone();
    }
    let img = layer
        .and_then(|l| l.icon_view())
        .and_then(print_window)
        .and_then(|(img, o)| crop(&img, o, monitor.rect))
        .map(|i| fit_to(i, size));
    *g = Some((std::time::Instant::now(), monitor.device.clone(), size, img.clone()));
    img
}

/// Measured on this machine (Windows 11 26200, raised desktop): `PrintWindow(Progman)` returns the running wallpaper
/// (image, web, even Media Foundation video) but no icons; `PrintWindow(SHELLDLL_DefView)` returns only icons on a black background.
/// So: background = a frame built from the image file if it is a static image we set, otherwise (a) Progman; if (a) is empty/black (a different desktop style) then (b) capture the wallpaper window, then (c) the static wallpaper;
/// icons come from DefView (cached for a few seconds) and are overlaid on the background.
pub fn capture(monitor: &Monitor, max_width: u32) -> Option<RgbaImage> {
    capture_inner(monitor, max_width, true)
}

/// Wallpaper layer only (no icons): the background right behind the text, for the Auto text-color mode.
pub fn capture_background(monitor: &Monitor, max_width: u32) -> Option<RgbaImage> {
    capture_inner(monitor, max_width, false)
}

/// Icon layer only (black background -> alpha 0), same size as `capture`. `None` if the icon layer cannot be printed.
pub fn capture_icons(monitor: &Monitor, max_width: u32) -> Option<RgbaImage> {
    let layer = DesktopLayer::locate();
    let size = target_size(monitor.rect.width().max(1) as u32, monitor.rect.height().max(1) as u32, max_width);
    icon_layer(layer, monitor, size).map(|i| icons_to_alpha(&i))
}

fn capture_inner(monitor: &Monitor, max_width: u32, with_icons: bool) -> Option<RgbaImage> {
    let t0 = std::time::Instant::now();
    let layer = DesktopLayer::locate();
    let size = target_size(monitor.rect.width().max(1) as u32, monitor.rect.height().max(1) as u32, max_width);
    let icons = if with_icons { icon_layer(layer, monitor, size) } else { None };

    // (b0) static image we set ourselves: build it from the image file (Progman may still print the old image).
    let mut method = 2u8;
    let mut base = wallpaper_frame(monitor).filter(has_content).map(|f| fit_to(f, size));
    // (a) print Progman: the running wallpaper (image, web, video).
    if base.is_none() {
        method = 1;
        base = layer
            .and_then(|l| print_window(l.progman))
            .and_then(|(img, o)| crop(&img, o, monitor.rect))
            .filter(has_content)
            .map(|i| fit_to(i, size));
    }
    // (b) capture the live wallpaper's window.
    if base.is_none() {
        method = 2;
        base = window_frame(monitor).filter(has_content).map(|f| fit_to(f, size));
    }
    // (c) Windows' static wallpaper.
    if base.is_none() {
        method = 3;
        base = static_wallpaper(monitor).map(|i| fit_to(i, size));
    }
    tracing::debug!(target: "sg::capture", "capture took {:?} (method {method}, icons {:?})", t0.elapsed(), icons.as_ref().map(|i| i.pixels().filter(|p| p[0] > 12 || p[1] > 12 || p[2] > 12).count()));
    let mut base = base?;
    if let Some(icons) = &icons {
        overlay_icons(&mut base, icons);
    }
    if with_icons {
        METHOD.store(method, Ordering::Relaxed);
    }
    Some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_inside_and_outside() {
        let img = RgbaImage::from_pixel(100, 50, image::Rgba([1, 2, 3, 255]));
        let c = crop(&img, (-10, 0), Rect::new(0, 0, 40, 20)).unwrap();
        assert_eq!(c.dimensions(), (40, 20));
        assert!(crop(&img, (0, 0), Rect::new(80, 0, 120, 20)).is_none());
        assert!(crop(&img, (0, 0), Rect::new(-5, 0, 10, 10)).is_none());
    }

    #[test]
    fn overlay_keeps_bright_pixels() {
        let mut base = RgbaImage::from_pixel(2, 1, image::Rgba([50, 50, 50, 255]));
        let mut icons = RgbaImage::from_pixel(2, 1, image::Rgba([0, 0, 0, 255]));
        icons.put_pixel(1, 0, image::Rgba([200, 10, 10, 255]));
        overlay_icons(&mut base, &icons);
        assert_eq!(base.get_pixel(0, 0)[0], 50);
        assert_eq!(base.get_pixel(1, 0)[0], 200);
    }

    #[test]
    #[ignore]
    fn real_print_each_layer() {
        let l = DesktopLayer::locate().unwrap();
        for (name, h) in [("progman", Some(l.progman)), ("workerw", Some(l.workerw)), ("defview", l.icon_view())] {
            let Some(h) = h else { continue };
            let t = std::time::Instant::now();
            let r = print_window(h);
            println!("{name}: took {:?}", t.elapsed());
            match r {
                Some((img, o)) => {
                    let sum: u64 = img.pixels().map(|p| (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3).sum();
                    println!("{name}: bright px = {}", img.pixels().filter(|p| p[0] > 12 || p[1] > 12 || p[2] > 12).count());
                    println!("{name}: {:?} origin={o:?} avg={}", img.dimensions(), sum / (img.width() as u64 * img.height() as u64).max(1));
                }
                None => println!("{name}: failed"),
            }
        }
    }

    /// Manual run: really captures the primary monitor, prints the method used + average brightness (no image is written to disk).
    #[test]
    #[ignore]
    fn real_capture_primary() {
        let ms = sg_desktop::monitors();
        let m = ms.iter().find(|m| m.primary).unwrap();
        let img = capture(m, 960).expect("capture");
        let sum: u64 = img.pixels().map(|p| (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3).sum();
        println!("method={} size={:?} avg={}", last_method(), img.dimensions(), sum / (img.width() as u64 * img.height() as u64));
    }
}
