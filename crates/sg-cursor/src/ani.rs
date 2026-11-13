//! Reads `.ani` (RIFF/ACON). Wrong chunk lengths, too many frames, etc. all return an error, never panic.

use crate::image::{CursorError, CursorImage, largest, parse_ico_cur, u32_at};
use crate::{MAX_FILE_BYTES, MAX_FRAMES};

/// Maximum total RGBA bytes across all frames after decoding (guards against extremely compressed files).
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    /// In playback order (with `seq ` applied); each frame is its largest image.
    pub frames: Vec<CursorImage>,
    /// How many jiffies (1/60 s) each frame is held. Same length as `frames`.
    pub delays_jiffies: Vec<u32>,
}

/// `.ani` or `.cur`/`.ico` (a single frame, delay `[0]`).
pub fn parse_cursor_file(bytes: &[u8]) -> Result<Animation, CursorError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(CursorError::TooLarge);
    }
    if !bytes.starts_with(b"RIFF") {
        let imgs = parse_ico_cur(bytes)?;
        let img = largest(&imgs).cloned().ok_or(CursorError::Malformed("no images"))?;
        return Ok(Animation { frames: vec![img], delays_jiffies: vec![0] });
    }
    if bytes.get(8..12) != Some(b"ACON") {
        return Err(CursorError::Malformed("RIFF is not ACON"));
    }
    let mut jif = 6u32;
    let mut declared_frames = None;
    let mut rate: Option<Vec<u32>> = None;
    let mut seq: Option<Vec<u32>> = None;
    let mut icons: Vec<CursorImage> = Vec::new();
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_at(bytes, pos + 4).ok_or(CursorError::Malformed("truncated chunk"))? as usize;
        let body = pos
            .checked_add(8)
            .and_then(|s| s.checked_add(size).and_then(|e| bytes.get(s..e)))
            .ok_or(CursorError::Malformed("chunk runs past end of file"))?;
        match id {
            b"anih" => {
                declared_frames = u32_at(body, 4);
                jif = match u32_at(body, 28) {
                    Some(0) | None => 6,
                    Some(j) => j,
                };
            }
            b"rate" => rate = Some(words(body)),
            b"seq " => seq = Some(words(body)),
            b"LIST" if body.get(0..4) == Some(b"fram") => read_frames(body, &mut icons)?,
            _ => {}
        }
        pos += 8 + size + (size & 1);
    }
    if declared_frames.is_some_and(|n| n as usize > MAX_FRAMES) {
        return Err(CursorError::TooLarge);
    }
    if icons.is_empty() {
        return Err(CursorError::Malformed("no frames"));
    }
    let order: Vec<usize> = match &seq {
        Some(s) if !s.is_empty() => s.iter().map(|&i| i as usize).collect(),
        _ => (0..icons.len()).collect(),
    };
    if order.len() > MAX_FRAMES {
        return Err(CursorError::TooLarge);
    }
    let mut frames = Vec::with_capacity(order.len());
    let mut budget = 0usize;
    for i in order {
        let img = icons.get(i).ok_or(CursorError::Malformed("seq refers to a missing frame"))?;
        budget += img.rgba.len();
        if budget > MAX_DECODED_BYTES {
            return Err(CursorError::TooLarge);
        }
        frames.push(img.clone());
    }
    let delays_jiffies = (0..frames.len()).map(|i| rate.as_ref().and_then(|r| r.get(i).copied()).unwrap_or(jif)).collect();
    Ok(Animation { frames, delays_jiffies })
}

fn words(body: &[u8]) -> Vec<u32> {
    body.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
}

fn read_frames(list: &[u8], icons: &mut Vec<CursorImage>) -> Result<(), CursorError> {
    let mut p = 4usize;
    while p + 8 <= list.len() {
        let id = &list[p..p + 4];
        let size = u32_at(list, p + 4).ok_or(CursorError::Malformed("truncated chunk"))? as usize;
        let body = p
            .checked_add(8)
            .and_then(|s| s.checked_add(size).and_then(|e| list.get(s..e)))
            .ok_or(CursorError::Malformed("frame runs past end of list"))?;
        if id == b"icon" {
            if icons.len() >= MAX_FRAMES {
                return Err(CursorError::TooLarge);
            }
            let imgs = parse_ico_cur(body)?;
            icons.push(largest(&imgs).cloned().ok_or(CursorError::Malformed("empty frame"))?);
        }
        p += 8 + size + (size & 1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;

    fn three() -> Vec<Vec<u8>> {
        vec![flat_cur(8, 8, [1, 0, 0, 255]), flat_cur(8, 8, [2, 0, 0, 255]), flat_cur(8, 8, [3, 0, 0, 255])]
    }

    #[test]
    fn ani_rate_seq() {
        let ani = make_ani(&three(), Some(&[2, 0, 1]), Some(&[6, 12, 18]), 6);
        let a = parse_cursor_file(&ani).unwrap();
        assert_eq!(a.frames.len(), 3);
        assert_eq!(a.frames[0].rgba[0], 3);
        assert_eq!(a.frames[1].rgba[0], 1);
        assert_eq!(a.delays_jiffies, vec![6, 12, 18]);
    }

    #[test]
    fn ani_no_rate_uses_jif() {
        let a = parse_cursor_file(&make_ani(&three(), None, None, 10)).unwrap();
        assert_eq!(a.delays_jiffies, vec![10, 10, 10]);
        let zero = parse_cursor_file(&make_ani(&three(), None, None, 0)).unwrap();
        assert_eq!(zero.delays_jiffies, vec![6, 6, 6]);
    }

    #[test]
    fn plain_cur_is_one_frame() {
        let a = parse_cursor_file(&flat_cur(16, 16, [5, 5, 5, 255])).unwrap();
        assert_eq!((a.frames.len(), a.delays_jiffies.clone()), (1, vec![0]));
    }

    #[test]
    fn ani_truncated_chunk_errors() {
        let mut ani = make_ani(&three(), None, None, 6);
        // an `anih` chunk declaring a length of 0x7FFFFFFF.
        ani[16..20].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        assert!(parse_cursor_file(&ani).is_err());
        let cut = make_ani(&three(), None, None, 6);
        assert!(parse_cursor_file(&cut[..cut.len() - 5]).is_err());
        assert!(parse_cursor_file(b"RIFF").is_err());
    }

    #[test]
    fn ani_frame_limit() {
        let frames: Vec<Vec<u8>> = (0..513).map(|_| flat_cur(1, 1, [0, 0, 0, 255])).collect();
        let ani = make_ani(&frames, None, None, 6);
        assert_eq!(parse_cursor_file(&ani), Err(CursorError::TooLarge));
    }

    #[test]
    fn ani_seq_out_of_range_errors() {
        let ani = make_ani(&three(), Some(&[0, 9]), None, 6);
        assert!(parse_cursor_file(&ani).is_err());
    }
}
