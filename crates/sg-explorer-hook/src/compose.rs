//! Builds the outline / shadow / blurred backdrop / text layers for one label (pure logic, testable anywhere).
//!
//! Input: `mask` = text coverage (0..255) produced by GDI drawing white text on black (gray anti-aliasing).
//! Output: **premultiplied** BGRA pixels so `AlphaBlend(AC_SRC_ALPHA)` can overlay them on the listview DC. This
//! is correct even when the desktop surface has an alpha channel (Windows 11 24H2+ "raised" desktop), where plain GDI text
//! would become transparent.
//!
//! Order from bottom to top: backdrop → shadow → outline → text.

use sg_explorer_proto::TextStyle;

/// The backdrop extends this many pixels beyond the text area on each side.
pub const BACKDROP_PAD: i32 = 2;
const BACKDROP_RADIUS: i32 = 3;

/// Offsets used to draw the outline: 8 directions × thickness (thickness 2 → 16 points, no duplicates, no (0,0)).
pub fn outline_offsets(width: u8) -> Vec<(i32, i32)> {
    let mut v = Vec::with_capacity(width as usize * 8);
    for r in 1..=width as i32 {
        for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            v.push((dx * r, dy * r));
        }
    }
    v
}

/// Margin around the text area needed for every effect of `style`.
pub fn margin(style: &TextStyle) -> i32 {
    let o = style.outline.map(|o| o.width as i32).unwrap_or(0);
    let s = style.shadow.map(|s| (s.dx as i32).abs().max((s.dy as i32).abs())).unwrap_or(0);
    let b = if style.backdrop.is_some() { BACKDROP_PAD } else { 0 };
    o.max(s).max(b) + 1
}

/// Blends color `rgb` with coverage `a` (0..255) over the premultiplied pixel `px` (B, G, R, A).
#[inline]
fn over(px: &mut [u8], rgb: u32, a: u32) {
    if a == 0 {
        return;
    }
    let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
    let inv = 255 - a;
    let mix = |src: u32, dst: u8| -> u8 { ((src * a + dst as u32 * inv + 127) / 255).min(255) as u8 };
    px[0] = mix(b, px[0]);
    px[1] = mix(g, px[1]);
    px[2] = mix(r, px[2]);
    px[3] = ((a * 255 + px[3] as u32 * inv + 127) / 255).min(255) as u8;
}

/// Backdrop coverage of the cell in the `label` region (x0, y0, x1, y1 in mask coordinates), with slightly rounded corners.
fn backdrop_cover(x: i32, y: i32, label: (i32, i32, i32, i32)) -> u32 {
    let (x0, y0, x1, y1) = (label.0 - BACKDROP_PAD, label.1 - BACKDROP_PAD, label.2 + BACKDROP_PAD, label.3 + BACKDROP_PAD);
    if x < x0 || y < y0 || x >= x1 || y >= y1 {
        return 0;
    }
    // Distance to the nearest corner (only computed inside the corner cell of radius R).
    let cx = if x < x0 + BACKDROP_RADIUS { x0 + BACKDROP_RADIUS - x } else if x >= x1 - BACKDROP_RADIUS { x - (x1 - BACKDROP_RADIUS - 1) } else { 0 };
    let cy = if y < y0 + BACKDROP_RADIUS { y0 + BACKDROP_RADIUS - y } else if y >= y1 - BACKDROP_RADIUS { y - (y1 - BACKDROP_RADIUS - 1) } else { 0 };
    if cx > 0 && cy > 0 && cx * cx + cy * cy > BACKDROP_RADIUS * BACKDROP_RADIUS {
        return 0;
    }
    255
}

/// Text coverage: `w * h` bytes, rows stored contiguously.
#[derive(Clone, Copy)]
pub struct Mask<'a> {
    pub data: &'a [u8],
    pub w: usize,
    pub h: usize,
}

/// Builds the image for one label.
///
/// - `mask`: text coverage.
/// - `label`: text region (x0, y0, x1, y1) in mask coordinates (for the backdrop).
/// - `out`: premultiplied BGRA, `stride` bytes per row (>= `w * 4`), at least `h` rows. The `w × h` region is overwritten.
/// - `scratch`: buffer reused between draws (dilated outline) — no new allocation when it is large enough.
pub fn compose(mask: Mask, label: (i32, i32, i32, i32), style: &TextStyle, scratch: &mut Vec<u8>, out: &mut [u8], stride: usize) {
    let Mask { data: mask, w, h } = mask;
    if w == 0 || h == 0 || mask.len() < w * h || stride < w * 4 || out.len() < stride * (h - 1) + w * 4 {
        return;
    }
    for y in 0..h {
        out[y * stride..y * stride + w * 4].fill(0);
    }
    let at = |x: i32, y: i32| -> u32 {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 { 0 } else { mask[y as usize * w + x as usize] as u32 }
    };

    if let Some(b) = style.backdrop.filter(|b| b.alpha > 0) {
        for y in 0..h {
            for x in 0..w {
                let c = backdrop_cover(x as i32, y as i32, label);
                let i = y * stride + x * 4;
                over(&mut out[i..i + 4], b.color, c * b.alpha as u32 / 255);
            }
        }
    }

    if let Some(s) = style.shadow.filter(|s| s.alpha > 0) {
        for y in 0..h {
            for x in 0..w {
                let c = at(x as i32 - s.dx as i32, y as i32 - s.dy as i32);
                let i = y * stride + x * 4;
                over(&mut out[i..i + 4], s.color, c * s.alpha as u32 / 255);
            }
        }
    }

    if let Some(o) = style.outline.filter(|o| o.width > 0 && o.alpha > 0) {
        let offs = outline_offsets(o.width);
        scratch.clear();
        scratch.resize(w * h, 0);
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let mut m = 0u32;
                for &(dx, dy) in &offs {
                    m = m.max(at(x - dx, y - dy));
                    if m == 255 {
                        break;
                    }
                }
                scratch[y as usize * w + x as usize] = m as u8;
            }
        }
        for y in 0..h {
            for x in 0..w {
                let i = y * stride + x * 4;
                over(&mut out[i..i + 4], o.color, scratch[y * w + x] as u32 * o.alpha as u32 / 255);
            }
        }
    }

    for y in 0..h {
        for x in 0..w {
            let i = y * stride + x * 4;
            over(&mut out[i..i + 4], style.color, mask[y * w + x] as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sg_explorer_proto::{Backdrop, Outline, Shadow};
    use std::collections::HashSet;

    #[test]
    fn outline_offsets_for_thickness() {
        let o = outline_offsets(2);
        assert_eq!(o.len(), 16);
        let set: HashSet<_> = o.iter().copied().collect();
        assert_eq!(set.len(), 16, "không trùng");
        assert!(!set.contains(&(0, 0)));
        assert!(set.contains(&(2, 2)) && set.contains(&(-2, 0)) && set.contains(&(1, -1)));
        assert!(outline_offsets(0).is_empty());
        assert_eq!(outline_offsets(4).len(), 32);
    }

    fn px(out: &[u8], stride: usize, x: usize, y: usize) -> [u8; 4] {
        out[y * stride + x * 4..y * stride + x * 4 + 4].try_into().unwrap()
    }

    /// A single fully covered text pixel in the middle of a 9×9 image.
    fn dot() -> (Vec<u8>, usize, usize) {
        let mut m = vec![0u8; 81];
        m[4 * 9 + 4] = 255;
        (m, 9, 9)
    }

    #[test]
    fn compose_text_over_outline() {
        let (m, w, h) = dot();
        let style = TextStyle { color: 0xFF0000, outline: Some(Outline { color: 0x0000FF, width: 1, alpha: 255 }), shadow: None, backdrop: None };
        let stride = w * 4 + 8;
        let mut out = vec![7u8; stride * h];
        compose(Mask { data: &m, w, h }, (3, 3, 6, 6), &style, &mut Vec::new(), &mut out, stride);
        assert_eq!(px(&out, stride, 4, 4), [0, 0, 255, 255], "chữ đỏ nằm trên cùng");
        assert_eq!(px(&out, stride, 5, 5), [255, 0, 0, 255], "viền xanh quanh chữ");
        assert_eq!(px(&out, stride, 3, 4), [255, 0, 0, 255]);
        assert_eq!(px(&out, stride, 6, 4), [0, 0, 0, 0], "ngoài độ dày viền thì trong suốt");
        assert_eq!(out[w * 4], 7, "không ghi ra ngoài vùng w×h của hàng");
    }

    #[test]
    fn compose_shadow_offset_and_alpha() {
        let (m, w, h) = dot();
        let style = TextStyle { color: 0xFFFFFF, outline: None, shadow: Some(Shadow { color: 0x000000, alpha: 128, dx: 2, dy: 1 }), backdrop: None };
        let mut out = vec![0u8; w * h * 4];
        compose(Mask { data: &m, w, h }, (0, 0, 9, 9), &style, &mut Vec::new(), &mut out, w * 4);
        assert_eq!(px(&out, w * 4, 6, 5), [0, 0, 0, 128], "bóng đen nửa đậm lệch (2, 1)");
        assert_eq!(px(&out, w * 4, 4, 4), [255, 255, 255, 255]);
        assert_eq!(px(&out, w * 4, 2, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn compose_backdrop_and_premultiplied() {
        let m = vec![0u8; 20 * 10];
        let style = TextStyle { color: 0xFFFFFF, outline: None, shadow: None, backdrop: Some(Backdrop { color: 0x204080, alpha: 102 }) };
        let mut out = vec![0u8; 20 * 10 * 4];
        compose(Mask { data: &m, w: 20, h: 10 }, (4, 3, 16, 7), &style, &mut Vec::new(), &mut out, 80);
        let c = px(&out, 80, 10, 5);
        assert_eq!(c[3], 102);
        assert_eq!([c[0], c[1], c[2]], [51, 26, 13], "màu đã nhân alpha (premultiplied)");
        assert_eq!(px(&out, 80, 1, 5)[3], 0, "ngoài vùng chữ + lề thì trống");
        assert_eq!(px(&out, 80, 2, 1)[3], 0, "góc được bo");
        for p in out.chunks(4) {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "premultiplied: kênh màu ≤ alpha");
        }
    }

    #[test]
    fn compose_rejects_bad_sizes() {
        let mut out = vec![1u8; 16];
        compose(Mask { data: &[0; 4], w: 2, h: 2 }, (0, 0, 2, 2), &TextStyle::plain(0), &mut Vec::new(), &mut out, 4);
        assert!(out.iter().all(|&b| b == 1), "stride nhỏ hơn w*4 → không làm gì");
        compose(Mask { data: &[0; 2], w: 2, h: 2 }, (0, 0, 2, 2), &TextStyle::plain(0), &mut Vec::new(), &mut out, 8);
        assert!(out.iter().all(|&b| b == 1), "mask thiếu → không làm gì");
    }

    #[test]
    fn margin_covers_effects() {
        let s = TextStyle { color: 0, outline: Some(Outline { color: 0, width: 3, alpha: 9 }), shadow: Some(Shadow { color: 0, alpha: 1, dx: -5, dy: 2 }), backdrop: None };
        assert_eq!(margin(&s), 6);
        assert_eq!(margin(&TextStyle::plain(0)), 1);
    }
}
