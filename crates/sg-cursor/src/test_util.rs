//! Builds cursor files by hand for tests (no purchased cursors are used).

pub enum Entry<'a> {
    Bmp32(&'a dyn Fn(u32, u32) -> [u8; 4]),
    Png(&'a dyn Fn(u32, u32) -> [u8; 4]),
}

fn bmp32(w: u32, h: u32, f: &dyn Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&w.to_le_bytes());
    b.extend_from_slice(&(h * 2).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, bl, a] = f(x, y);
            b.extend_from_slice(&[bl, g, r, a]);
        }
    }
    let mask_row = (w.div_ceil(32) * 4) as usize;
    b.extend(std::iter::repeat_n(0u8, mask_row * h as usize));
    b
}

fn png(w: u32, h: u32, f: &dyn Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// CUR with several images `(w, h, kind)`, same hotspot.
pub fn make_cur(entries: &[(u32, u32, Entry)], hot: (u16, u16)) -> Vec<u8> {
    let blobs: Vec<Vec<u8>> = entries
        .iter()
        .map(|(w, h, e)| match e {
            Entry::Bmp32(f) => bmp32(*w, *h, *f),
            Entry::Png(f) => png(*w, *h, *f),
        })
        .collect();
    let mut out = vec![0, 0, 2, 0];
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut off = 6 + 16 * entries.len();
    for ((w, h, _), blob) in entries.iter().zip(&blobs) {
        out.extend_from_slice(&[(*w % 256) as u8, (*h % 256) as u8, 0, 0]);
        out.extend_from_slice(&hot.0.to_le_bytes());
        out.extend_from_slice(&hot.1.to_le_bytes());
        out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        out.extend_from_slice(&(off as u32).to_le_bytes());
        off += blob.len();
    }
    for b in blobs {
        out.extend_from_slice(&b);
    }
    out
}

/// CUR with one 32bpp BMP image, flat color, hotspot (0,0).
pub fn flat_cur(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
    make_cur(&[(w, h, Entry::Bmp32(&move |_, _| color))], (0, 0))
}

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v.extend_from_slice(body);
    if body.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// ANI built from already-built `.cur` files, with optional `seq`/`rate`.
pub fn make_ani(frames: &[Vec<u8>], seq: Option<&[u32]>, rate: Option<&[u32]>, jif: u32) -> Vec<u8> {
    let steps = seq.map(|s| s.len()).unwrap_or(frames.len()) as u32;
    let mut anih = Vec::new();
    for v in [36u32, frames.len() as u32, steps, 0, 0, 0, 1, jif, 1] {
        anih.extend_from_slice(&v.to_le_bytes());
    }
    let mut body = b"ACON".to_vec();
    body.extend(chunk(b"anih", &anih));
    let words = |w: &[u32]| w.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
    if let Some(r) = rate {
        body.extend(chunk(b"rate", &words(r)));
    }
    if let Some(s) = seq {
        body.extend(chunk(b"seq ", &words(s)));
    }
    let mut list = b"fram".to_vec();
    for f in frames {
        list.extend(chunk(b"icon", f));
    }
    body.extend(chunk(b"LIST", &list));
    chunk(b"RIFF", &body)
}
