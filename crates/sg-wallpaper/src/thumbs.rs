// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Wallpaper thumbnails (`wallpapers-thumbs/<id>.jpg`, at most 480 wide) and capturing a window into memory.
//! Video/exe get their frame/icon from Explorer's thumbnail provider; images and the thumbnails bundled with Lively are read with `image`.

use crate::config::WallpaperPaths;
use crate::library::{Wallpaper, WallpaperType};
use image::RgbaImage;
use image::codecs::jpeg::JpegEncoder;
use std::path::{Path, PathBuf};

pub const THUMB_WIDTH: u32 = 480;

pub fn thumb_path(paths: &WallpaperPaths, id: &str) -> PathBuf {
    paths.thumbs.join(format!("{id}.jpg"))
}

/// Encodes a JPEG (quality 82) from an RGBA image, scaled down to at most `max_width` wide.
pub fn encode_jpeg(img: &RgbaImage, max_width: u32) -> Option<Vec<u8>> {
    let (w, h) = img.dimensions();
    let small;
    let src = if max_width > 0 && w > max_width {
        let nh = ((h as u64 * max_width as u64) / w as u64).max(1) as u32;
        small = image::imageops::resize(img, max_width, nh, image::imageops::FilterType::Triangle);
        &small
    } else {
        img
    };
    let (w, h) = src.dimensions();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for p in src.pixels() {
        rgb.extend_from_slice(&p.0[..3]);
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 82).encode(&rgb, w, h, image::ExtendedColorType::Rgb8).ok()?;
    Some(out)
}

fn read_image(path: &Path) -> Option<RgbaImage> {
    Some(image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?.to_rgba8())
}

/// Ensures a wallpaper has a thumbnail; returns the path if there is one. Otherwise `None` (URL, exe without an icon...).
pub fn ensure_thumb(paths: &WallpaperPaths, w: &Wallpaper) -> Option<PathBuf> {
    let dst = thumb_path(paths, &w.id);
    if dst.is_file() {
        return Some(dst);
    }
    let img = candidate_image(w)?;
    let jpeg = encode_jpeg(&img, THUMB_WIDTH)?;
    std::fs::create_dir_all(&paths.thumbs).ok()?;
    std::fs::write(&dst, jpeg).ok()?;
    Some(dst)
}

fn candidate_image(w: &Wallpaper) -> Option<RgbaImage> {
    // 1) The thumbnail bundled with the Lively wallpaper.
    if let Some(p) = w.thumbnail_path()
        && let Some(i) = read_image(&p)
    {
        return Some(i);
    }
    if matches!(w.kind(), WallpaperType::Url | WallpaperType::VideoStream) {
        return None;
    }
    let source = PathBuf::from(w.source());
    if !source.is_file() {
        return None;
    }
    // 2) Static image / GIF: read directly.
    if matches!(w.kind(), WallpaperType::Picture | WallpaperType::Gif)
        && let Some(i) = read_image(&source)
    {
        return Some(i);
    }
    // 3) Video, exe, images in unusual formats: ask Explorer.
    shell_thumbnail(&source, 480).ok()
}

pub use win::{shell_thumbnail, snapshot_window};

mod win {
    use super::RgbaImage;
    use crate::util::wide;
    use std::path::Path;
    use windows::Win32::Foundation::{E_FAIL, HWND, RECT, SIZE};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        GetDIBits, GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC, SelectObject,
    };
    use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK};
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    use windows::core::Result;

    fn fail() -> windows::core::Error {
        windows::core::Error::from_hresult(E_FAIL)
    }

    /// A thumbnail produced by Explorer (video gets a frame, exe gets an icon).
    pub fn shell_thumbnail(src: &Path, size: i32) -> Result<RgbaImage> {
        // SAFETY: every GDI/COM object created is released; the pixel buffer is large enough for w*h*4.
        unsafe {
            let path = wide(src.as_os_str());
            let factory: IShellItemImageFactory = SHCreateItemFromParsingName(path.pcwstr(), None)?;
            let hbmp = factory.GetImage(SIZE { cx: size, cy: size }, SIIGBF_BIGGERSIZEOK)?;
            let mut bm = BITMAP::default();
            GetObjectW(HGDIOBJ(hbmp.0), size_of::<BITMAP>() as i32, Some((&raw mut bm).cast()));
            let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
            if w <= 0 || h <= 0 {
                let _ = DeleteObject(HGDIOBJ(hbmp.0));
                return Err(fail());
            }
            let mut bmi = BITMAPINFO {
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
            let mut px = vec![0u8; (w * h * 4) as usize];
            let hdc = GetDC(None);
            GetDIBits(hdc, hbmp, 0, h as u32, Some(px.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS);
            ReleaseDC(None, hdc);
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            // A video thumbnail often has alpha = 0 at every pixel → treat it as opaque.
            let all_clear = px.as_chunks::<4>().0.iter().all(|p| p[3] == 0);
            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
                if all_clear {
                    p[3] = 255;
                }
            }
            RgbaImage::from_raw(w as u32, h as u32, px).ok_or_else(fail)
        }
    }

    /// Captures a window's content (including DirectX/WebView2) into memory, without writing to disk.
    pub fn snapshot_window(hwnd: HWND) -> Result<RgbaImage> {
        // SAFETY: as in `shell_thumbnail`; the DIB is only read while it is alive.
        unsafe {
            let mut r = RECT::default();
            GetClientRect(hwnd, &mut r)?;
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if w <= 0 || h <= 0 {
                return Err(fail());
            }
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
            let dib: Result<HBITMAP> = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
            let out = dib.and_then(|dib| {
                let old = SelectObject(mem, HGDIOBJ(dib.0));
                // PW_RENDERFULLCONTENT = 2: also capture DirectComposition content.
                let _ = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(2));
                let src = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                let mut rgba = Vec::with_capacity(src.len());
                for p in src.as_chunks::<4>().0 {
                    rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
                SelectObject(mem, old);
                let _ = DeleteObject(HGDIOBJ(dib.0));
                RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or_else(fail)
            });
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_is_shrunk() {
        let img = RgbaImage::from_pixel(1000, 500, image::Rgba([10, 120, 200, 255]));
        let j = encode_jpeg(&img, 480).unwrap();
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
        let back = image::load_from_memory(&j).unwrap();
        assert_eq!((back.width(), back.height()), (480, 240));
    }
}
