//! Auto mode: each file name picks light or dark text from the background right behind it, with hysteresis against flicker.
//!
//! "Brightness" here is the perceived lightness L* (CIE, 0..1) of the mean relative luminance (sRGB channels with gamma removed).
//! The 0.5 threshold of L* corresponds to a luminance of ≈ 0.18 — roughly the point where white and black text give equal contrast.

use image::RgbaImage;
use sg_explorer_proto::{Outline, Shadow, TextStyle};
use std::collections::HashMap;

pub const THRESHOLD: f32 = 0.5;
/// The threshold must be exceeded by this much (±8 %) to count as a "change".
pub const BAND: f32 = 0.08;
/// …and it must hold continuously for this long before the color really changes.
pub const HOLD_MS: u64 = 1500;

pub fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Relative luminance (0..1) → perceived lightness L*/100 (0..1).
pub fn lightness(y: f32) -> f32 {
    let y = y.clamp(0.0, 1.0);
    let f = if y > 216.0 / 24389.0 { y.cbrt() } else { (24389.0 / 27.0 * y + 16.0) / 116.0 };
    ((116.0 * f - 16.0) / 100.0).clamp(0.0, 1.0)
}

/// Mean brightness of the region `rect` = (x0, y0, x1, y1) in image coordinates (clamped to the image; empty region → `None`).
/// Large regions are sampled sparsely (at most ~4096 points).
pub fn luminance(img: &RgbaImage, rect: (i32, i32, i32, i32)) -> Option<f32> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (x0, y0) = (rect.0.clamp(0, w), rect.1.clamp(0, h));
    let (x1, y1) = (rect.2.clamp(0, w), rect.3.clamp(0, h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let area = ((x1 - x0) as i64 * (y1 - y0) as i64).max(1);
    let step = ((area as f64 / 4096.0).sqrt().ceil() as i32).max(1);
    let (mut sum, mut n) = (0f64, 0u32);
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let p = img.get_pixel(x as u32, y as u32);
            let lin = 0.2126 * srgb_to_linear(p[0]) + 0.7152 * srgb_to_linear(p[1]) + 0.0722 * srgb_to_linear(p[2]);
            sum += lin as f64;
            n += 1;
            x += step;
        }
        y += step;
    }
    Some(lightness((sum / n.max(1) as f64) as f32))
}

#[derive(Debug, Clone, Copy)]
struct ItemState {
    light: bool,
    pending_since: Option<u64>,
}

/// Per-item hysteresis: changes only when the brightness stays beyond the threshold ±[`BAND`] continuously for ≥ [`HOLD_MS`].
#[derive(Debug, Default)]
pub struct Hysteresis {
    items: HashMap<u32, ItemState>,
}

impl Hysteresis {
    /// `Some(light text?)` when this item's decision has just been made (first time) or has just changed; `None` when unchanged.
    pub fn update(&mut self, index: u32, lum: f32, now_ms: u64) -> Option<bool> {
        let Some(st) = self.items.get_mut(&index) else {
            // First time: decide immediately (dark background → light text).
            let light = lum < THRESHOLD;
            self.items.insert(index, ItemState { light, pending_since: None });
            return Some(light);
        };
        let wants_switch = if st.light { lum > THRESHOLD + BAND } else { lum < THRESHOLD - BAND };
        if !wants_switch {
            st.pending_since = None;
            return None;
        }
        match st.pending_since {
            None => {
                st.pending_since = Some(now_ms);
                None
            }
            Some(t) if now_ms.saturating_sub(t) >= HOLD_MS => {
                st.light = !st.light;
                st.pending_since = None;
                Some(st.light)
            }
            Some(_) => None,
        }
    }

    pub fn get(&self, index: u32) -> Option<bool> {
        self.items.get(&index).map(|s| s.light)
    }

    /// The item moved (the background behind it changed entirely): decide again right at the next measurement.
    pub fn reset(&mut self, index: u32) {
        self.items.remove(&index);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn retain(&mut self, keep: impl Fn(u32) -> bool) {
        self.items.retain(|k, _| keep(*k));
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Near-black text (not pure black: easier on the eyes and does not collide with the "transparent" color of some surfaces).
pub const DARK_TEXT: u32 = 0x10_1010;
pub const LIGHT_TEXT: u32 = 0xFF_FFFF;

/// Automatic text style: dark background → white text + thin black outline/shadow; light background → black text + thin white outline.
pub fn auto_style(light_text: bool, outline: bool) -> TextStyle {
    if light_text {
        TextStyle {
            color: LIGHT_TEXT,
            outline: outline.then_some(Outline { color: 0x00_0000, width: 1, alpha: 140 }),
            shadow: outline.then_some(Shadow { color: 0x00_0000, alpha: 110, dx: 1, dy: 1 }),
            backdrop: None,
        }
    } else {
        TextStyle {
            color: DARK_TEXT,
            outline: outline.then_some(Outline { color: 0xFF_FFFF, width: 1, alpha: 170 }),
            shadow: None,
            backdrop: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn solid(v: u8) -> RgbaImage {
        RgbaImage::from_pixel(64, 32, Rgba([v, v, v, 255]))
    }

    #[test]
    fn luminance_black_white_gray() {
        assert!(luminance(&solid(0), (0, 0, 64, 32)).unwrap() < 0.001);
        assert!(luminance(&solid(255), (0, 0, 64, 32)).unwrap() > 0.999);
        // sRGB 119 ≈ L* 50.
        let g = luminance(&solid(119), (0, 0, 64, 32)).unwrap();
        assert!((g - 0.5).abs() < 0.01, "{g}");
        // Left half black, right half white: only the requested region is measured.
        let mut img = solid(0);
        for y in 0..32 {
            for x in 32..64 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        assert!(luminance(&img, (0, 0, 32, 32)).unwrap() < 0.01);
        assert!(luminance(&img, (40, 0, 60, 32)).unwrap() > 0.99);
        assert!(luminance(&img, (-50, -50, 10, 10)).unwrap() < 0.01, "vùng ra ngoài ảnh được kẹp");
        assert!(luminance(&img, (70, 0, 90, 10)).is_none(), "hoàn toàn ngoài ảnh");
    }

    #[test]
    fn hysteresis_no_flicker() {
        let mut h = Hysteresis::default();
        assert_eq!(h.update(1, 0.45, 0), Some(true), "lần đầu quyết ngay: nền hơi tối → chữ sáng");
        // Oscillating around the threshold (including crossings shorter than 1.5 s) for 20 s → never changes.
        let mut t = 0;
        for i in 0..200 {
            t += 100;
            let lum = match i % 10 {
                0..=4 => 0.40 + (i % 5) as f32 * 0.04, // 0.40 … 0.56
                5..=8 => 0.60,                         // crosses the band for 400 ms, then returns
                _ => 0.52,
            };
            assert_eq!(h.update(1, lum, t), None, "lần {i} (lum {lum})");
        }
        assert_eq!(h.get(1), Some(true));
    }

    #[test]
    fn hysteresis_switches_after_hold() {
        let mut h = Hysteresis::default();
        h.update(7, 0.2, 0);
        assert_eq!(h.update(7, 0.7, 1000), None, "bắt đầu đếm");
        assert_eq!(h.update(7, 0.7, 2000), None);
        assert_eq!(h.update(7, 0.7, 2500), Some(false), "giữ đủ 1,5 s → chữ tối");
        assert_eq!(h.get(7), Some(false));
        // Changing back must also hold long enough.
        assert_eq!(h.update(7, 0.3, 3000), None);
        assert_eq!(h.update(7, 0.3, 4499), None);
        assert_eq!(h.update(7, 0.3, 4500), Some(true));
        h.reset(7);
        assert_eq!(h.update(7, 0.9, 4600), Some(false), "sau reset quyết ngay");
        h.retain(|i| i != 7);
        assert!(h.is_empty());
    }

    #[test]
    fn auto_style_contrast() {
        let l = auto_style(true, true);
        assert_eq!(l.color, LIGHT_TEXT);
        assert_eq!(l.outline.unwrap().color, 0x000000, "chữ sáng ↔ viền tối");
        let d = auto_style(false, true);
        assert_eq!(d.color, DARK_TEXT);
        assert_eq!(d.outline.unwrap().color, 0xFFFFFF, "chữ tối ↔ viền sáng");
        assert!(!auto_style(true, false).has_effects() && !auto_style(false, false).has_effects());
        // Enough contrast: light text on a dark background and vice versa.
        assert!(lightness(srgb_to_linear(0x10)) < 0.1);
    }
}
