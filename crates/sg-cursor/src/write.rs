//! Writes `.cur`/`.ani` files Windows can load: each image is a 32bpp BGRA BMP + an AND mask.

use crate::image::CursorImage;

fn bmp_blob(img: &CursorImage) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    let mask_row = w.div_ceil(32) * 4;
    let mut b = Vec::with_capacity(40 + w * h * 4 + mask_row * h);
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&img.width.to_le_bytes());
    // The DIB height in CUR/ICO is doubled: image + AND mask.
    b.extend_from_slice(&(img.height * 2).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    b.extend_from_slice(&((w * h * 4 + mask_row * h) as u32).to_le_bytes());
    b.extend_from_slice(&[0u8; 16]); // resolution, color count
    for y in (0..h).rev() {
        for x in 0..w {
            let p = (y * w + x) * 4;
            let px = img.rgba.get(p..p + 4).unwrap_or(&[0, 0, 0, 0]);
            b.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    for y in (0..h).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..w {
            let alpha = img.rgba.get((y * w + x) * 4 + 3).copied().unwrap_or(0);
            if alpha == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        b.extend_from_slice(&row);
    }
    b
}

/// A single `.cur` file containing every image (multiple sizes).
pub fn write_cur(images: &[CursorImage]) -> Vec<u8> {
    let blobs: Vec<Vec<u8>> = images.iter().map(bmp_blob).collect();
    let mut out = vec![0, 0, 2, 0];
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut off = 6 + 16 * images.len();
    for (img, blob) in images.iter().zip(&blobs) {
        // Size 256 is stored as 0 in the directory.
        out.push((img.width % 256) as u8);
        out.push((img.height % 256) as u8);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(img.hotspot.0 as u16).to_le_bytes());
        out.extend_from_slice(&(img.hotspot.1 as u16).to_le_bytes());
        out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        out.extend_from_slice(&(off as u32).to_le_bytes());
        off += blob.len();
    }
    for blob in blobs {
        out.extend_from_slice(&blob);
    }
    out
}

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + body.len() + 1);
    v.extend_from_slice(id);
    v.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v.extend_from_slice(body);
    if body.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// A single `.ani` file: each frame is a list of images in multiple sizes; `delays_jiffies` has the same length as `frames`.
pub fn write_ani(frames: &[Vec<CursorImage>], delays_jiffies: &[u32]) -> Vec<u8> {
    let n = frames.len() as u32;
    let first = delays_jiffies.first().copied().unwrap_or(6);
    let mut anih = Vec::with_capacity(36);
    // cbSize, nFrames, nSteps, cx, cy, cBitCount, nPlanes, jifRate, flags (AF_ICON = 1).
    for v in [36u32, n, n, 0, 0, 0, 1, first, 1] {
        anih.extend_from_slice(&v.to_le_bytes());
    }
    let rate: Vec<u8> = (0..frames.len())
        .flat_map(|i| delays_jiffies.get(i).copied().unwrap_or(first).to_le_bytes())
        .collect();
    let mut list = b"fram".to_vec();
    for f in frames {
        list.extend(chunk(b"icon", &write_cur(f)));
    }
    let mut body = b"ACON".to_vec();
    body.extend(chunk(b"anih", &anih));
    body.extend(chunk(b"rate", &rate));
    body.extend(chunk(b"LIST", &list));
    chunk(b"RIFF", &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_cursor_file, parse_ico_cur};

    pub(crate) fn pattern(size: u32, hot: (u32, u32), seed: u8) -> CursorImage {
        let mut rgba = Vec::new();
        for y in 0..size {
            for x in 0..size {
                if (x + y) % 5 == 0 {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    rgba.extend_from_slice(&[(x as u8).wrapping_add(seed), (y as u8).wrapping_mul(3), seed, 255 - ((x + y) % 7) as u8]);
                }
            }
        }
        CursorImage { width: size, height: size, hotspot: hot, rgba }
    }

    #[test]
    fn cur_roundtrip() {
        let imgs = vec![pattern(32, (3, 4), 1), pattern(48, (5, 6), 2)];
        assert_eq!(parse_ico_cur(&write_cur(&imgs)).unwrap(), imgs);
    }

    #[test]
    fn cur_256_dir_byte_zero() {
        let img = pattern(256, (10, 10), 3);
        let bytes = write_cur(std::slice::from_ref(&img));
        assert_eq!((bytes[6], bytes[7]), (0, 0));
        assert_eq!(parse_ico_cur(&bytes).unwrap(), vec![img]);
    }

    #[test]
    fn ani_roundtrip() {
        let frames: Vec<Vec<CursorImage>> = (0..3).map(|i| vec![pattern(32, (1, 1), i), pattern(48, (2, 2), i)]).collect();
        let bytes = write_ani(&frames, &[6, 6, 12]);
        let a = parse_cursor_file(&bytes).unwrap();
        assert_eq!(a.frames.len(), 3);
        assert_eq!(a.delays_jiffies, vec![6, 6, 12]);
        assert!(a.frames.iter().all(|f| f.width == 48));
        assert_eq!(a.frames[1], frames[1][1]);
    }
}
