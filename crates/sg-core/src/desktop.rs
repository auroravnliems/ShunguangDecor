//! The OS's current wallpaper (for the large image on the Home page) and the process's RAM usage.
//! The image part will later be replaced by a real desktop capture frame (`sg-capture`).

use crate::platform;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use std::path::{Path, PathBuf};

/// File name Windows uses for its wallpaper copy; meaningless to the user.
pub const WINDOWS_TRANSCODED: &str = "TranscodedWallpaper";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperRef {
    pub path: PathBuf,
    /// File name to display; empty when it is Windows's `TranscodedWallpaper` copy.
    pub name: String,
    pub modified_unix: u64,
}

pub fn current_wallpaper() -> Option<WallpaperRef> {
    let path = platform::current_wallpaper_path()?;
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let file = path.file_name()?.to_string_lossy().to_string();
    let name = if file == WINDOWS_TRANSCODED { String::new() } else { file };
    let modified_unix = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(WallpaperRef { path, name, modified_unix })
}

/// JPEG thumbnail (quality 85), at most `max_width` wide, aspect ratio kept; smaller images are left as-is.
/// The format is detected from the file content (Windows's file has no extension).
pub fn thumbnail_jpeg(path: &Path, max_width: u32) -> Result<Vec<u8>, String> {
    let img = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let img = if img.width() > max_width && max_width > 0 {
        let h = ((img.height() as u64 * max_width as u64) / img.width() as u64).max(1) as u32;
        img.resize_exact(max_width, h, FilterType::Triangle)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 85).encode_image(&rgb).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Actual RAM in use by this process (working set on Windows).
pub fn process_memory_bytes() -> Option<u64> {
    memory_stats::memory_stats().map(|m| m.physical_mem as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([59, 123, 233]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn dims(jpeg: &[u8]) -> (u32, u32) {
        let img = image::load_from_memory(jpeg).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn thumbnail_downscales() {
        let d = tempfile::tempdir().unwrap();
        // No file extension, like Windows's TranscodedWallpaper.
        let p = d.path().join("TranscodedWallpaper");
        std::fs::write(&p, png(3000, 1000)).unwrap();
        let j = thumbnail_jpeg(&p, 960).unwrap();
        assert_eq!(&j[..2], &[0xFF, 0xD8], "phải là JPEG");
        assert_eq!(dims(&j), (960, 320));

        let small = d.path().join("small.png");
        std::fs::write(&small, png(400, 300)).unwrap();
        assert_eq!(dims(&thumbnail_jpeg(&small, 960).unwrap()), (400, 300));
    }

    #[test]
    fn thumbnail_bad_file_errors() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("rac.jpg");
        std::fs::write(&p, b"khong phai anh").unwrap();
        assert!(thumbnail_jpeg(&p, 960).is_err());
        assert!(thumbnail_jpeg(&d.path().join("khong-co.jpg"), 960).is_err());
    }

    #[test]
    fn memory_is_reported() {
        assert!(process_memory_bytes().unwrap_or(1) > 0);
    }

    #[test]
    #[ignore]
    fn print_real_desktop() {
        println!("wallpaper = {:?}", current_wallpaper());
        println!("ram = {:?}", process_memory_bytes());
        println!("{}", crate::sysinfo::collect("dev", None, false).to_text());
    }
}
