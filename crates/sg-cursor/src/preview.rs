//! Preview images for the UI: a single frame or all frames laid out horizontally, original size, PNG.

use crate::{Animation, CursorImage};
use std::io::Cursor;

fn png(width: u32, height: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let img = ::image::RgbaImage::from_raw(width, height, rgba)?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ::image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

pub fn frame_png(img: &CursorImage) -> Option<Vec<u8>> {
    png(img.width, img.height, img.rgba.clone())
}

/// All frames laid out horizontally on one strip; each cell is as wide as the largest frame, smaller images sit in the top-left corner.
pub fn sheet_png(anim: &Animation) -> Option<Vec<u8>> {
    let cell_w = anim.frames.iter().map(|f| f.width).max()?;
    let cell_h = anim.frames.iter().map(|f| f.height).max()?;
    let n = anim.frames.len() as u32;
    let mut canvas = vec![0u8; (cell_w * cell_h * n * 4) as usize];
    let row = (cell_w * n * 4) as usize;
    for (i, f) in anim.frames.iter().enumerate() {
        for y in 0..f.height as usize {
            let src = y * f.width as usize * 4;
            let dst = y * row + i * cell_w as usize * 4;
            canvas[dst..dst + f.width as usize * 4].copy_from_slice(&f.rgba[src..src + f.width as usize * 4]);
        }
    }
    png(cell_w * n, cell_h, canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, v: u8) -> CursorImage {
        CursorImage { width: w, height: h, hotspot: (0, 0), rgba: vec![v; (w * h * 4) as usize] }
    }

    #[test]
    fn sheet_places_frames_side_by_side() {
        let a = Animation { frames: vec![img(4, 4, 10), img(2, 2, 200)], delays_jiffies: vec![6, 6] };
        let bytes = sheet_png(&a).unwrap();
        let sheet = ::image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(sheet.dimensions(), (8, 4));
        assert_eq!(sheet.get_pixel(0, 0).0, [10; 4]);
        assert_eq!(sheet.get_pixel(4, 0).0, [200; 4]);
        assert_eq!(sheet.get_pixel(7, 3).0, [0; 4]);
        assert!(frame_png(&a.frames[1]).is_some());
        assert!(sheet_png(&Animation { frames: vec![], delays_jiffies: vec![] }).is_none());
    }
}
