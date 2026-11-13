//! Captures the real desktop for the preview pane.
//!
//! Constraints: captured images live in memory only (never written to disk, content never logged, never sent anywhere);
//! capture only runs while a page is being viewed; when the last viewer leaves, the capture thread exits and the frame is dropped.

mod platform;
pub mod service;

pub use image::RgbaImage;
pub use service::{CaptureFrame, CaptureService, FAST_PERIOD, JPEG_QUALITY, MAX_WIDTH, PERIOD, ViewMode, period_for};

use sg_desktop::Monitor;
use std::sync::RwLock;

type FrameProvider = Box<dyn Fn(&Monitor) -> Option<RgbaImage> + Send + Sync>;

static PROVIDER: RwLock<Option<FrameProvider>> = RwLock::new(None);
static WINDOW_PROVIDER: RwLock<Option<FrameProvider>> = RwLock::new(None);

/// The "real" frame of a static-image wallpaper, built by `sg-wallpaper` from the image file itself (preferred over
/// printing Progman, because Windows repaints the wallpaper in Progman slowly). Returns `None` if that wallpaper is not a static image.
pub fn set_wallpaper_frame_provider(f: impl Fn(&Monitor) -> Option<RgbaImage> + Send + Sync + 'static) {
    *PROVIDER.write().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
}

/// Method (b): a frame captured from the live wallpaper's window; used only when printing Progman yields no image (a different kind of desktop).
pub fn set_window_frame_provider(f: impl Fn(&Monitor) -> Option<RgbaImage> + Send + Sync + 'static) {
    *WINDOW_PROVIDER.write().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
}

pub fn clear_wallpaper_frame_provider() {
    *PROVIDER.write().unwrap_or_else(|e| e.into_inner()) = None;
    *WINDOW_PROVIDER.write().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn wallpaper_frame(m: &Monitor) -> Option<RgbaImage> {
    PROVIDER.read().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|f| f(m))
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn window_frame(m: &Monitor) -> Option<RgbaImage> {
    WINDOW_PROVIDER.read().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|f| f(m))
}

/// Whether the image has "content": not empty and not pure black. Samples sparsely for speed.
pub fn has_content(img: &RgbaImage) -> bool {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return false;
    }
    let px = img.as_raw();
    let step = ((w as usize * h as usize) / 4096).max(1);
    px.as_chunks::<4>().0.iter().step_by(step).any(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
}

/// Shrinks to at most `max_width` wide (keeping the aspect ratio); smaller images are left unchanged.
pub fn shrink(img: RgbaImage, max_width: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if max_width == 0 || w <= max_width {
        return img;
    }
    let nh = ((h as u64 * max_width as u64) / w as u64).max(1) as u32;
    image::imageops::resize(&img, max_width, nh, image::imageops::FilterType::Triangle)
}

/// Captures the desktop layer (icons + wallpaper) of one monitor, at most `max_width` wide.
///
/// Methods are tried in order, stopping at the first that yields an image with content: (a) `PrintWindow` of the desktop layer;
/// (b) the live-wallpaper frame supplied by `sg-wallpaper`, with icons composited from (a);
/// (c) the static Windows wallpaper image.
pub fn capture_desktop(monitor: &Monitor, max_width: u32) -> Option<RgbaImage> {
    platform::capture(monitor, max_width)
}

/// Only the wallpaper layer of one monitor (same capture order as [`capture_desktop`], without compositing icons), used to measure
/// the brightness right behind the file names (desktop labels, Auto mode). The image stays in memory only.
pub fn capture_background(monitor: &Monitor, max_width: u32) -> Option<RgbaImage> {
    platform::capture_background(monitor, max_width)
}

/// Only the desktop icon layer of one monitor, PNG-ready RGBA: the icon layer's black background becomes transparent (see
/// [`icons_to_alpha`]). Used by the preview pane when the wallpaper is played live in the layer below.
pub fn capture_icons(monitor: &Monitor, max_width: u32) -> Option<RgbaImage> {
    platform::capture_icons(monitor, max_width)
}

/// Converts an icon image captured on a black background (`PrintWindow(SHELLDLL_DefView)`) into an image with an alpha channel.
///
/// Accepted limitation: `PrintWindow(SHELLDLL_DefView)` only yields icons drawn on black, with no real alpha channel
/// (same as `overlay_icons` on the Windows path). So only pure black (0,0,0) pixels become transparent; pure-black text/shadows are lost.
///
/// - pure black -> alpha 0;
/// - very dark but not black pixels (< `DARK_MAX`, e.g. dark text, drop shadows) -> color kept, opaque (alpha 255);
/// - brighter pixels: treated as the anti-aliased edge of bright content on black, each pixel = color * coverage, so
///   coverage (alpha) = the brightest channel, and the original color is recovered by dividing it back out; edges stay smooth instead of being hard-cut.
pub fn icons_to_alpha(img: &RgbaImage) -> RgbaImage {
    const DARK_MAX: u8 = 24;
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let a = p[0].max(p[1]).max(p[2]);
        if a == 0 {
            *p = image::Rgba([0, 0, 0, 0]);
            continue;
        }
        if a < DARK_MAX {
            *p = image::Rgba([p[0], p[1], p[2], 255]);
            continue;
        }
        let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        *p = image::Rgba([un(p[0]), un(p[1]), un(p[2]), a]);
    }
    out
}

/// The capture method used most recently (for recording in diagnostic logs; contains no image content).
pub fn last_method() -> &'static str {
    platform::last_method()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_to_alpha_keeps_edges() {
        let mut img = RgbaImage::from_pixel(4, 1, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([255, 255, 255, 255])); // solid
        img.put_pixel(2, 0, image::Rgba([100, 50, 25, 255])); // anti-aliased edge of an orange icon
        img.put_pixel(3, 0, image::Rgba([12, 12, 14, 255])); // dark text/shadow but not pure black
        let out = icons_to_alpha(&img);
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 0], "đen kịt -> trong suốt");
        assert_eq!(out.get_pixel(1, 0).0, [255, 255, 255, 255], "trắng đặc giữ nguyên");
        // Edge: alpha = brightness, color is divided back out so it keeps the orange hue; no hard cut.
        let e = out.get_pixel(2, 0).0;
        assert_eq!(e[3], 100);
        assert_eq!(e[0], 255);
        assert!((e[1] as i32 - 127).abs() <= 1 && (e[2] as i32 - 64).abs() <= 1, "{e:?}");
        assert_eq!(out.get_pixel(3, 0).0, [12, 12, 14, 255], "tối nhưng không đen kịt thì giữ đặc");
        // Over a background: the edge pixel blends with the background color by alpha (gray 100 background -> 100*155/255 + 255*100/255).
        let a = e[3] as f32 / 255.0;
        let mixed = 100.0 * (1.0 - a) + e[0] as f32 * a;
        assert!((mixed - (100.0 * 155.0 / 255.0 + 100.0)).abs() < 1.5);
    }
}
