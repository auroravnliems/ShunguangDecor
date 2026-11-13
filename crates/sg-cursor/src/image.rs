//! Reads `.cur`/`.ico`: every index goes through a bounds-checked helper; a broken file yields an error, never a panic.

use crate::{MAX_FILE_BYTES, MAX_IMAGE_SIDE, MAX_IMAGES_PER_ICON};
use sg_core::text::I18nText;
use std::fmt;
use std::io::Cursor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub hotspot: (u32, u32),
    /// RGBA 8-bit, `width * height * 4` bytes, top row first.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorError {
    Malformed(&'static str),
    TooLarge,
    Io(String),
}

impl CursorError {
    pub fn to_text(&self) -> I18nText {
        I18nText::new("cursor.err.bad_file").arg("detail", self)
    }
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CursorError::Malformed(why) => write!(f, "malformed cursor file: {why}"),
            CursorError::TooLarge => write!(f, "cursor file exceeds the size limits"),
            CursorError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CursorError {}

pub(crate) fn u16_at(b: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(pos..pos.checked_add(2)?)?.try_into().ok()?))
}

pub(crate) fn u32_at(b: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(pos..pos.checked_add(4)?)?.try_into().ok()?))
}

fn be32_at(b: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(pos..pos.checked_add(4)?)?.try_into().ok()?))
}

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Every image in a CUR/ICO file (the hotspot comes from the directory for CUR; for ICO it is (0,0)).
pub fn parse_ico_cur(bytes: &[u8]) -> Result<Vec<CursorImage>, CursorError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(CursorError::TooLarge);
    }
    let kind = match (u16_at(bytes, 0), u16_at(bytes, 2), u16_at(bytes, 4)) {
        (Some(0), Some(k @ (1 | 2)), Some(_)) => k,
        _ => return Err(CursorError::Malformed("not an ICO/CUR header")),
    };
    let count = u16_at(bytes, 4).unwrap_or(0) as usize;
    if count == 0 {
        return Err(CursorError::Malformed("no images"));
    }
    if count > MAX_IMAGES_PER_ICON {
        return Err(CursorError::TooLarge);
    }
    let mut images = Vec::new();
    let mut first_err = None;
    for i in 0..count {
        let pos = 6 + 16 * i;
        let (Some(planes), Some(bitcount), Some(size), Some(off)) =
            (u16_at(bytes, pos + 4), u16_at(bytes, pos + 6), u32_at(bytes, pos + 8), u32_at(bytes, pos + 12))
        else {
            first_err.get_or_insert(CursorError::Malformed("truncated directory"));
            continue;
        };
        let blob = (off as usize).checked_add(size as usize).and_then(|end| bytes.get(off as usize..end));
        let Some(blob) = blob else {
            first_err.get_or_insert(CursorError::Malformed("image data out of range"));
            continue;
        };
        let hotspot = if kind == 2 { (planes as u32, bitcount as u32) } else { (0, 0) };
        match decode_entry(blob, hotspot) {
            Ok(img) => images.push(img),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    if images.is_empty() {
        return Err(first_err.unwrap_or(CursorError::Malformed("no usable image")));
    }
    Ok(images)
}

fn decode_entry(blob: &[u8], hotspot: (u32, u32)) -> Result<CursorImage, CursorError> {
    let rgba_img = if blob.starts_with(PNG_MAGIC) {
        // IHDR: width/height sit at bytes 16..24 — check them before decoding to avoid a huge allocation.
        let (Some(w), Some(h)) = (be32_at(blob, 16), be32_at(blob, 20)) else {
            return Err(CursorError::Malformed("truncated PNG"));
        };
        check_side(w)?;
        check_side(h)?;
        ::image::load_from_memory_with_format(blob, ::image::ImageFormat::Png)
            .map_err(|_| CursorError::Malformed("bad PNG image"))?
            .to_rgba8()
    } else {
        let (Some(header), Some(w), Some(h), Some(bits)) =
            (u32_at(blob, 0), u32_at(blob, 4), u32_at(blob, 8), u16_at(blob, 14))
        else {
            return Err(CursorError::Malformed("truncated bitmap header"));
        };
        if header < 40 {
            return Err(CursorError::Malformed("unsupported bitmap header"));
        }
        // The height in an ICO/CUR DIB is doubled (image + AND mask).
        let h = (h as i32).unsigned_abs() / 2;
        check_side(w)?;
        check_side(h)?;
        // Wrap it as a single-entry ICO so the `image` crate's ICO decoder can be used.
        let mut wrapped = Vec::with_capacity(22 + blob.len());
        wrapped.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
        wrapped.extend_from_slice(&[(w % 256) as u8, (h % 256) as u8, 0, 0]);
        wrapped.extend_from_slice(&1u16.to_le_bytes());
        wrapped.extend_from_slice(&bits.to_le_bytes());
        wrapped.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        wrapped.extend_from_slice(&22u32.to_le_bytes());
        wrapped.extend_from_slice(blob);
        let decoder = ::image::codecs::ico::IcoDecoder::new(Cursor::new(wrapped))
            .map_err(|_| CursorError::Malformed("bad bitmap image"))?;
        ::image::DynamicImage::from_decoder(decoder)
            .map_err(|_| CursorError::Malformed("bad bitmap image"))?
            .to_rgba8()
    };
    let (width, height) = rgba_img.dimensions();
    check_side(width)?;
    check_side(height)?;
    Ok(CursorImage {
        width,
        height,
        hotspot: (hotspot.0.min(width - 1), hotspot.1.min(height - 1)),
        rgba: rgba_img.into_raw(),
    })
}

fn check_side(v: u32) -> Result<(), CursorError> {
    if v == 0 {
        Err(CursorError::Malformed("image has zero size"))
    } else if v > MAX_IMAGE_SIDE {
        Err(CursorError::TooLarge)
    } else {
        Ok(())
    }
}

/// The image with the largest area.
pub fn largest(images: &[CursorImage]) -> Option<&CursorImage> {
    images.iter().max_by_key(|i| i.width as u64 * i.height as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;

    fn flat(c: [u8; 4]) -> impl Fn(u32, u32) -> [u8; 4] {
        move |_, _| c
    }

    #[test]
    fn cur_bmp32_hotspot() {
        let f = |x: u32, y: u32| if (x, y) == (0, 0) { [10, 20, 30, 255] } else { [1, 2, 3, 128] };
        let cur = make_cur(&[(32, 32, Entry::Bmp32(&f))], (5, 7));
        let imgs = parse_ico_cur(&cur).unwrap();
        assert_eq!(imgs.len(), 1);
        let i = &imgs[0];
        assert_eq!((i.width, i.height, i.hotspot), (32, 32, (5, 7)));
        assert_eq!(&i.rgba[0..4], &[10, 20, 30, 255]);
        assert_eq!(&i.rgba[4..8], &[1, 2, 3, 128]);
    }

    #[test]
    fn cur_png_entry() {
        let f = flat([9, 8, 7, 255]);
        let cur = make_cur(&[(64, 64, Entry::Png(&f))], (1, 2));
        let imgs = parse_ico_cur(&cur).unwrap();
        assert_eq!((imgs[0].width, imgs[0].hotspot), (64, (1, 2)));
        assert_eq!(&imgs[0].rgba[0..4], &[9, 8, 7, 255]);
    }

    #[test]
    fn largest_picks_biggest() {
        let f = flat([1, 1, 1, 255]);
        let cur = make_cur(&[(16, 16, Entry::Bmp32(&f)), (48, 48, Entry::Bmp32(&f)), (32, 32, Entry::Bmp32(&f))], (0, 0));
        let imgs = parse_ico_cur(&cur).unwrap();
        assert_eq!(imgs.len(), 3);
        assert_eq!(largest(&imgs).unwrap().width, 48);
        assert!(largest(&[]).is_none());
    }

    #[test]
    fn ico_count_limit() {
        let mut b = vec![0, 0, 2, 0];
        b.extend_from_slice(&65535u16.to_le_bytes());
        assert_eq!(parse_ico_cur(&b), Err(CursorError::TooLarge));
        assert!(parse_ico_cur(&[0, 0, 2]).is_err());
        assert!(parse_ico_cur(b"").is_err());
    }

    #[test]
    fn zero_size_image_errors() {
        let mut cur = flat_cur(8, 8, [1, 1, 1, 255]);
        // biWidth (byte 4 of the DIB, which starts at 22) = 0.
        cur[22 + 4..22 + 8].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse_ico_cur(&cur).is_err());
    }

    #[test]
    fn huge_bmp_header_rejected_without_allocating() {
        let mut cur = flat_cur(8, 8, [1, 1, 1, 255]);
        cur[22 + 4..22 + 8].copy_from_slice(&60000u32.to_le_bytes());
        assert_eq!(parse_ico_cur(&cur), Err(CursorError::TooLarge));
    }

    #[test]
    fn entry_out_of_range_errors() {
        let mut cur = flat_cur(8, 8, [1, 1, 1, 255]);
        cur[6 + 12..6 + 16].copy_from_slice(&0xFFFF_FF00u32.to_le_bytes());
        assert!(parse_ico_cur(&cur).is_err());
    }
}
