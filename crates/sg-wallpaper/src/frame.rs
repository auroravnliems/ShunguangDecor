//! The "real" frame of a static-image wallpaper: the image placed with fill / fit / stretch to match the screen's aspect ratio.
//! Used for the preview frame: Windows redraws the wallpaper in Progman slowly, so capturing Progman may still show the old image.

use crate::config::Scaling;
use image::imageops::{FilterType, crop_imm, overlay, resize};
use image::{Rgba, RgbaImage};

/// Places `img` into the `target` frame (width, height) according to `scaling`; `max_width` caps the result's width (0 = no limit).
pub fn fit_picture(img: &RgbaImage, target: (u32, u32), scaling: Scaling, max_width: u32) -> RgbaImage {
    let (tw, th) = (target.0.max(1), target.1.max(1));
    let k = if max_width > 0 && tw > max_width { max_width as f32 / tw as f32 } else { 1.0 };
    let (ow, oh) = (((tw as f32 * k).round() as u32).max(1), ((th as f32 * k).round() as u32).max(1));
    let (iw, ih) = img.dimensions();
    if iw == 0 || ih == 0 {
        return RgbaImage::from_pixel(ow, oh, Rgba([0, 0, 0, 255]));
    }
    match scaling {
        Scaling::Stretch => resize(img, ow, oh, FilterType::Triangle),
        Scaling::Fill => {
            // Scale enough to cover completely, then crop the center.
            let s = (ow as f32 / iw as f32).max(oh as f32 / ih as f32);
            let (cw, ch) = (((ow as f32 / s).round() as u32).clamp(1, iw), ((oh as f32 / s).round() as u32).clamp(1, ih));
            let (cx, cy) = ((iw - cw) / 2, (ih - ch) / 2);
            resize(&crop_imm(img, cx, cy, cw, ch).to_image(), ow, oh, FilterType::Triangle)
        }
        Scaling::Fit => {
            let s = (ow as f32 / iw as f32).min(oh as f32 / ih as f32);
            let (w, h) = (((iw as f32 * s).round() as u32).clamp(1, ow), ((ih as f32 * s).round() as u32).clamp(1, oh));
            let mut canvas = RgbaImage::from_pixel(ow, oh, Rgba([0, 0, 0, 255]));
            overlay(&mut canvas, &resize(img, w, h, FilterType::Triangle), ((ow - w) / 2) as i64, ((oh - h) / 2) as i64);
            canvas
        }
    }
}

/// Reads an image from disk (format detected from content); an unreadable format (HEIC…) → `None`.
pub fn read_picture(path: &std::path::Path) -> Option<RgbaImage> {
    Some(image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide() -> RgbaImage {
        // 40x10: left half red, right half blue.
        RgbaImage::from_fn(40, 10, |x, _| if x < 20 { Rgba([255, 0, 0, 255]) } else { Rgba([0, 0, 255, 255]) })
    }

    #[test]
    fn fill_crops_to_the_target_aspect() {
        let o = fit_picture(&wide(), (20, 20), Scaling::Fill, 0);
        assert_eq!(o.dimensions(), (20, 20));
        // Center 10x10 crop of the 40x10 image: left half red, right half blue.
        assert_eq!(o.get_pixel(2, 10)[0], 255);
        assert_eq!(o.get_pixel(17, 10)[2], 255);
    }

    #[test]
    fn fit_letterboxes_in_black() {
        let o = fit_picture(&wide(), (40, 40), Scaling::Fit, 0);
        assert_eq!(o.dimensions(), (40, 40));
        assert_eq!(o.get_pixel(5, 2), &Rgba([0, 0, 0, 255]), "viền đen phía trên");
        assert_eq!(o.get_pixel(5, 20)[0], 255, "ảnh nằm giữa");
    }

    #[test]
    fn stretch_and_max_width() {
        let o = fit_picture(&wide(), (100, 50), Scaling::Stretch, 0);
        assert_eq!(o.dimensions(), (100, 50));
        let small = fit_picture(&wide(), (1920, 1080), Scaling::Fill, 960);
        assert_eq!(small.dimensions(), (960, 540));
        assert_eq!(fit_picture(&RgbaImage::new(0, 0), (4, 4), Scaling::Fill, 0).dimensions(), (4, 4));
    }
}
