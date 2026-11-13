//! Cursor resizing: Sharp (nearest-neighbor) or Smooth (Lanczos).

use crate::image::CursorImage;
use ::image::imageops::{FilterType, resize};
use ::image::RgbaImage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScaleMode {
    #[default]
    Sharp,
    Smooth,
}

impl ScaleMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ScaleMode::Sharp => "sharp",
            ScaleMode::Smooth => "smooth",
        }
    }
}

/// Outputs exactly `size × size`; non-square images keep their aspect ratio, are placed at the top-left, on a transparent background.
/// The hotspot is multiplied by the scale factor, then clamped to the image.
pub fn scale(img: &CursorImage, size: u32, mode: ScaleMode) -> CursorImage {
    let size = size.max(1);
    if img.width == size && img.height == size {
        return img.clone();
    }
    let Some(src) = RgbaImage::from_raw(img.width, img.height, img.rgba.clone()) else {
        return CursorImage { width: size, height: size, hotspot: (0, 0), rgba: vec![0; (size * size * 4) as usize] };
    };
    let factor = size as f64 / img.width.max(img.height).max(1) as f64;
    let nw = ((img.width as f64 * factor).round() as u32).clamp(1, size);
    let nh = ((img.height as f64 * factor).round() as u32).clamp(1, size);
    let resized = match mode {
        ScaleMode::Sharp => resize(&src, nw, nh, FilterType::Nearest),
        ScaleMode::Smooth => unpremultiply(resize(&premultiply(src), nw, nh, FilterType::Lanczos3)),
    };
    let mut canvas = vec![0u8; (size * size * 4) as usize];
    for y in 0..nh {
        let from = (y * nw * 4) as usize;
        let to = ((y * size) * 4) as usize;
        canvas[to..to + (nw * 4) as usize].copy_from_slice(&resized.as_raw()[from..from + (nw * 4) as usize]);
    }
    let hot = |v: u32| ((v as f64 * factor).round() as u32).min(size - 1);
    CursorImage { width: size, height: size, hotspot: (hot(img.hotspot.0), hot(img.hotspot.1)), rgba: canvas }
}

/// Multiply color by alpha before sampling so transparent edges don't bleed black.
fn premultiply(mut img: RgbaImage) -> RgbaImage {
    for p in img.pixels_mut() {
        let a = p.0[3] as u32;
        for c in 0..3 {
            p.0[c] = ((p.0[c] as u32 * a + 127) / 255) as u8;
        }
    }
    img
}

fn unpremultiply(mut img: RgbaImage) -> RgbaImage {
    for p in img.pixels_mut() {
        let a = p.0[3] as u32;
        for c in 0..3 {
            if let Some(v) = (p.0[c] as u32 * 255 + a / 2).checked_div(a) {
                p.0[c] = v.min(255) as u8;
            }
        }
    }
    img
}

/// Side length (px) of the single image written in each frame: cursor size × system DPI, clamped to `1..=MAX_IMAGE_SIDE`.
///
/// Only ONE image per frame, rather than several sizes for several DPI levels as before: Windows (Win11 26200, tested 13/11)
/// refuses to load the whole `.ani` file when a frame contains multiple sizes in certain combinations (e.g. 64+80+96+128,
/// 128+64, 160+200, 96+120+144+192: `LoadCursorFromFile` returns NULL for every requested size), whereas a single-image
/// frame always loads from 16 up to 256. Windows rescales the image to each monitor's cursor size by itself, so one image
/// sized for the primary monitor is enough.
pub fn image_side(size: u32, dpi: u32) -> u32 {
    let dpi = if dpi == 0 { 96 } else { dpi };
    ((size as u64 * dpi as u64 + 48) / 96).clamp(1, crate::MAX_IMAGE_SIDE as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_sharp_exact_pixels() {
        let img = CursorImage {
            width: 2,
            height: 2,
            hotspot: (0, 0),
            rgba: vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
        };
        let s = scale(&img, 4, ScaleMode::Sharp);
        assert_eq!((s.width, s.height), (4, 4));
        let at = |x: usize, y: usize| s.rgba[(y * 4 + x) * 4];
        for (qx, qy, v) in [(0, 0, 1), (1, 0, 2), (0, 1, 3), (1, 1, 4)] {
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                assert_eq!(at(qx * 2 + dx, qy * 2 + dy), v);
            }
        }
    }

    #[test]
    fn scale_hotspot_scaled() {
        let img = CursorImage { width: 32, height: 32, hotspot: (5, 7), rgba: vec![255; 32 * 32 * 4] };
        assert_eq!(scale(&img, 64, ScaleMode::Sharp).hotspot, (10, 14));
        assert_eq!(scale(&img, 64, ScaleMode::Smooth).hotspot, (10, 14));
        assert_eq!(scale(&img, 32, ScaleMode::Sharp), img);
    }

    #[test]
    fn scale_non_square_keeps_ratio() {
        let img = CursorImage { width: 4, height: 2, hotspot: (3, 1), rgba: vec![255; 4 * 2 * 4] };
        let s = scale(&img, 8, ScaleMode::Sharp);
        assert_eq!((s.width, s.height), (8, 8));
        // 8×4 at the top-left, the rest transparent.
        assert_eq!(s.rgba[3], 255);
        assert_eq!(s.rgba[(5 * 8) * 4 + 3], 0);
        assert_eq!(s.hotspot, (6, 2));
    }

    #[test]
    fn scale_smooth_no_dark_fringe() {
        // Left half opaque white, right half transparent (color 0): after smoothing there must be no gray/black fringe.
        let mut rgba = Vec::new();
        for _y in 0..4 {
            for x in 0..4 {
                rgba.extend_from_slice(if x < 2 { &[255, 255, 255, 255] } else { &[0, 0, 0, 0] });
            }
        }
        let img = CursorImage { width: 4, height: 4, hotspot: (0, 0), rgba };
        let s = scale(&img, 16, ScaleMode::Smooth);
        for p in s.rgba.chunks(4).filter(|p| p[3] > 40) {
            assert!(p[0] > 240, "viền bị tối: {p:?}");
        }
    }

    #[test]
    fn image_side_cases() {
        assert_eq!(image_side(32, 96), 32);
        assert_eq!(image_side(40, 120), 50);
        assert_eq!(image_side(48, 144), 72);
        assert_eq!(image_side(200, 192), 256);
        assert_eq!(image_side(64, 0), 64);
    }
}
