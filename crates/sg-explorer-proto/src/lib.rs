//! Pipe protocol between the app (`sg-explorer`) and the DLL inside Explorer (`sg-explorer-hook`).
//!
//! - Pipe: `\\.\pipe\ShunguangDecor-explorer-<session id>`, the app is the server, the DLL is the client.
//! - Frame: 4-byte length (little-endian) + JSON, with at most [`MAX_FRAME`] bytes of JSON.
//! - Both sides send a `Hello` carrying [`PROTO_VERSION`] as soon as they connect; on a version mismatch the DLL rests and the app reports
//!   "Explorer needs to be restarted".
//!
//! This crate has no Windows dependency so its tests run anywhere.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Protocol version. Bump this whenever the structure of any message changes.
pub const PROTO_VERSION: u32 = 2;
/// Maximum size of the JSON part of a frame.
pub const MAX_FRAME: usize = 1 << 20;
/// Maximum number of entries in a color table / layout (a real desktop rarely has more than a few hundred icons).
pub const MAX_ITEMS: usize = 4096;
/// Pipe name prefix; the Windows session id is appended after it.
pub const PIPE_PREFIX: &str = r"\\.\pipe\ShunguangDecor-explorer-";
/// Name of the private message (RegisterWindowMessageW) used to push work onto Explorer's desktop thread.
pub const WINDOW_MESSAGE: &str = "ShunguangDecor.Explorer";
/// Name of the mutex object the DLL creates once it is inside Explorer (the app uses it to know the DLL is present and avoid loading it twice).
pub const LOADED_MARKER: &str = r"Local\ShunguangDecor.ExplorerDll";
/// `wParam` of the private message: first load (the app sends it along with the hook) / paint state sync.
pub const CMD_INIT: usize = 0x5347_0001;
pub const CMD_SYNC: usize = 0x5347_0002;

/// Pipe name for one Windows session.
pub fn pipe_name(session_id: u32) -> String {
    format!("{PIPE_PREFIX}{session_id}")
}

/// App → DLL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ToDll {
    /// Sent as soon as the DLL connects. `log_path`: the DLL's log file (`paths.local/logs/explorer-hook.log`).
    Hello { version: u32, log_path: Option<String> },
    /// Paint the desktop labels.
    Labels(LabelsPaint),
    /// Restore the desktop labels to the Windows default.
    LabelsOff,
    /// Ask the DLL to resend the current label layout (`FromDll::LabelLayout`).
    GetLayout,
    Ping,
}

/// DLL → app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FromDll {
    /// `build`: the DLL build id (version + hash) for logging, not used for comparison.
    Hello { version: u32, pid: u32, build: String },
    /// The label rectangle of each item, in screen coordinates (real pixels).
    LabelLayout { items: Vec<LabelRect> },
    Status { labels_active: bool, error: Option<String> },
    Pong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaintMode {
    /// One style per item (`items`); an item missing from the table uses `uniform` (if any) or the default.
    PerItem,
    /// Every item uses `uniform`.
    #[default]
    Uniform,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LabelsPaint {
    pub mode: PaintMode,
    pub uniform: Option<TextStyle>,
    /// By item index in the desktop listview.
    #[serde(default)]
    pub items: Vec<ItemStyle>,
}

impl LabelsPaint {
    /// Style for item `index`. `None` = let Windows draw that item itself.
    pub fn style_for(&self, index: u32) -> Option<&TextStyle> {
        match self.mode {
            PaintMode::Uniform => self.uniform.as_ref(),
            PaintMode::PerItem => {
                // `items` is normally already sorted by index; binary search, falling back to a linear search if the order is off.
                match self.items.binary_search_by_key(&index, |i| i.index) {
                    Ok(pos) => Some(&self.items[pos].style),
                    Err(_) => self
                        .items
                        .iter()
                        .find(|i| i.index == index)
                        .map(|i| &i.style)
                        .or(self.uniform.as_ref()),
                }
            }
        }
    }

    /// Drops the excess beyond the limit, clamps values into the valid range, and sorts `items` by index. The DLL calls this before use.
    pub fn sanitized(mut self) -> LabelsPaint {
        self.items.truncate(MAX_ITEMS);
        self.items.sort_by_key(|i| i.index);
        self.items.dedup_by_key(|i| i.index);
        for i in &mut self.items {
            i.style = i.style.clamped();
        }
        self.uniform = self.uniform.map(|s| s.clamped());
        self
    }
}

/// Colors are `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TextStyle {
    pub color: u32,
    pub outline: Option<Outline>,
    pub shadow: Option<Shadow>,
    pub backdrop: Option<Backdrop>,
}

impl TextStyle {
    pub fn plain(color: u32) -> TextStyle {
        TextStyle { color: color & 0xFF_FFFF, ..TextStyle::default() }
    }

    /// Whether anything needs drawing beyond changing the text color.
    pub fn has_effects(&self) -> bool {
        self.outline.is_some_and(|o| o.width > 0 && o.alpha > 0)
            || self.shadow.is_some_and(|s| s.alpha > 0)
            || self.backdrop.is_some_and(|b| b.alpha > 0)
    }

    pub fn clamped(self) -> TextStyle {
        TextStyle {
            color: self.color & 0xFF_FFFF,
            outline: self.outline.map(|o| Outline { color: o.color & 0xFF_FFFF, width: o.width.min(MAX_OUTLINE), alpha: o.alpha }),
            shadow: self.shadow.map(|s| Shadow {
                color: s.color & 0xFF_FFFF,
                alpha: s.alpha,
                dx: s.dx.clamp(-MAX_SHADOW_OFFSET, MAX_SHADOW_OFFSET),
                dy: s.dy.clamp(-MAX_SHADOW_OFFSET, MAX_SHADOW_OFFSET),
            }),
            backdrop: self.backdrop.map(|b| Backdrop { color: b.color & 0xFF_FFFF, alpha: b.alpha }),
        }
    }
}

pub const MAX_OUTLINE: u8 = 4;
pub const MAX_SHADOW_OFFSET: i8 = 6;

/// Text outline: `width` 1..=4 pixels, `alpha` 0..=255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outline {
    pub color: u32,
    pub width: u8,
    pub alpha: u8,
}

/// Text shadow: offset `dx`/`dy` pixels (-6..=6), `alpha` 0..=255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shadow {
    pub color: u32,
    pub alpha: u8,
    pub dx: i8,
    pub dy: i8,
}

/// Translucent backdrop behind the text (a slightly rounded rectangle covering the text area).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backdrop {
    pub color: u32,
    pub alpha: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStyle {
    pub index: u32,
    pub style: TextStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelRect {
    pub index: u32,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug)]
pub enum ProtoError {
    /// Not enough bytes yet for a complete frame.
    NeedMore,
    /// The frame declares a length above [`MAX_FRAME`].
    TooLarge(usize),
    Json(serde_json::Error),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtoError::NeedMore => write!(f, "incomplete frame"),
            ProtoError::TooLarge(n) => write!(f, "frame too large ({n} bytes)"),
            ProtoError::Json(e) => write!(f, "bad frame: {e}"),
        }
    }
}

impl std::error::Error for ProtoError {}

/// Frames one message: 4-byte LE length + JSON.
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    // The types in this crate can always be converted to JSON (no odd map keys); if it ever fails, send an empty `{}` frame
    // so the other side reports a JSON error instead of hanging.
    let body = serde_json::to_vec(msg).unwrap_or_else(|_| b"{}".to_vec());
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Reads the JSON length from the first 4 bytes of a frame.
pub fn frame_len(header: [u8; 4]) -> Result<usize, ProtoError> {
    let n = u32::from_le_bytes(header) as usize;
    if n > MAX_FRAME { Err(ProtoError::TooLarge(n)) } else { Ok(n) }
}

/// Decodes one complete frame (length + JSON). Missing bytes → `NeedMore`.
pub fn decode<T: DeserializeOwned>(buf: &[u8]) -> Result<T, ProtoError> {
    let header: [u8; 4] = buf.get(..4).and_then(|h| h.try_into().ok()).ok_or(ProtoError::NeedMore)?;
    let n = frame_len(header)?;
    let body = buf.get(4..4 + n).ok_or(ProtoError::NeedMore)?;
    serde_json::from_slice(body).map_err(ProtoError::Json)
}

/// Incremental read buffer: push bytes in, pull complete messages out.
#[derive(Debug, Default)]
pub struct FrameBuf {
    buf: Vec<u8>,
}

impl FrameBuf {
    pub fn new() -> FrameBuf {
        FrameBuf::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next message if enough bytes have arrived. On an error (too large, bad JSON) the connection should be closed.
    pub fn next_msg<T: DeserializeOwned>(&mut self) -> Result<Option<T>, ProtoError> {
        let Some(header) = self.buf.get(..4).and_then(|h| <[u8; 4]>::try_from(h).ok()) else { return Ok(None) };
        let n = frame_len(header)?;
        if self.buf.len() < 4 + n {
            return Ok(None);
        }
        let msg = serde_json::from_slice(&self.buf[4..4 + n]).map_err(ProtoError::Json);
        self.buf.drain(..4 + n);
        msg.map(Some)
    }

    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> TextStyle {
        TextStyle {
            color: 0xFF_80C0,
            outline: Some(Outline { color: 0, width: 2, alpha: 200 }),
            shadow: Some(Shadow { color: 0x10_1010, alpha: 120, dx: 1, dy: -2 }),
            backdrop: Some(Backdrop { color: 0x20_2020, alpha: 60 }),
        }
    }

    fn all_to_dll() -> Vec<ToDll> {
        vec![
            ToDll::Hello { version: PROTO_VERSION, log_path: Some(r"C:\x\logs\explorer-hook.log".into()) },
            ToDll::Hello { version: 7, log_path: None },
            ToDll::Labels(LabelsPaint {
                mode: PaintMode::PerItem,
                uniform: None,
                items: vec![ItemStyle { index: 3, style: style() }, ItemStyle { index: 9, style: TextStyle::plain(0) }],
            }),
            ToDll::Labels(LabelsPaint { mode: PaintMode::Uniform, uniform: Some(style()), items: vec![] }),
            ToDll::LabelsOff,
            ToDll::GetLayout,
            ToDll::Ping,
        ]
    }

    fn all_from_dll() -> Vec<FromDll> {
        vec![
            FromDll::Hello { version: PROTO_VERSION, pid: 1234, build: "0.1.0-abcd1234".into() },
            FromDll::LabelLayout {
                items: vec![LabelRect { index: 0, left: -1920, top: 10, right: -1800, bottom: 40 }],
            },
            FromDll::Status { labels_active: true, error: Some("x".into()) },
            FromDll::Status { labels_active: false, error: None },
            FromDll::Pong,
        ]
    }

    #[test]
    fn status_with_unknown_field_still_decodes() {
        // A different DLL version sends an extra field this version does not know: it still decodes, ignoring the unknown field.
        let json = br#"{"t":"status","labels_active":true,"extra_field":false,"error":null}"#;
        let mut f = (json.len() as u32).to_le_bytes().to_vec();
        f.extend_from_slice(json);
        assert_eq!(decode::<FromDll>(&f).unwrap(), FromDll::Status { labels_active: true, error: None });
    }

    #[test]
    fn frame_roundtrip_all_variants() {
        for m in all_to_dll() {
            let f = encode(&m);
            assert_eq!(frame_len(f[..4].try_into().unwrap()).unwrap(), f.len() - 4);
            assert_eq!(decode::<ToDll>(&f).unwrap(), m);
        }
        for m in all_from_dll() {
            assert_eq!(decode::<FromDll>(&encode(&m)).unwrap(), m);
        }
        // The `t` tag is snake_case, readable by eye in logs.
        let s = String::from_utf8(encode(&ToDll::LabelsOff)[4..].to_vec()).unwrap();
        assert_eq!(s, r#"{"t":"labels_off"}"#);
    }

    #[test]
    fn oversized_frame_rejected() {
        let mut f = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        f.extend_from_slice(b"{}");
        assert!(matches!(frame_len(f[..4].try_into().unwrap()), Err(ProtoError::TooLarge(_))));
        assert!(matches!(decode::<ToDll>(&f), Err(ProtoError::TooLarge(_))));
        let mut b = FrameBuf::new();
        b.push(&f);
        assert!(matches!(b.next_msg::<ToDll>(), Err(ProtoError::TooLarge(_))));
        // Exactly MAX_FRAME still has its header read (not enough bytes → wait for more).
        let ok = (MAX_FRAME as u32).to_le_bytes();
        assert_eq!(frame_len(ok).unwrap(), MAX_FRAME);
    }

    #[test]
    fn truncated_frame_needs_more() {
        let mut bytes = Vec::new();
        let msgs = all_to_dll();
        for m in &msgs {
            bytes.extend(encode(m));
        }
        assert!(matches!(decode::<ToDll>(&bytes[..3]), Err(ProtoError::NeedMore)));
        assert!(matches!(decode::<ToDll>(&encode(&ToDll::Ping)[..6]), Err(ProtoError::NeedMore)));
        // Push in 5-byte chunks: everything comes out, in the right order.
        let mut b = FrameBuf::new();
        let mut got = Vec::new();
        for chunk in bytes.chunks(5) {
            b.push(chunk);
            while let Some(m) = b.next_msg::<ToDll>().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, msgs);
        assert_eq!(b.pending(), 0);
    }

    #[test]
    fn unknown_tag_is_error() {
        let body = br#"{"t":"format_c_drive"}"#;
        let mut f = (body.len() as u32).to_le_bytes().to_vec();
        f.extend_from_slice(body);
        assert!(matches!(decode::<ToDll>(&f), Err(ProtoError::Json(_))));
        let mut b = FrameBuf::new();
        b.push(&f);
        assert!(matches!(b.next_msg::<FromDll>(), Err(ProtoError::Json(_))));
    }

    #[test]
    fn style_lookup_and_sanitize() {
        let p = LabelsPaint {
            mode: PaintMode::PerItem,
            uniform: Some(TextStyle::plain(0x11)),
            items: vec![
                ItemStyle { index: 5, style: TextStyle::plain(0x55) },
                ItemStyle { index: 2, style: TextStyle { outline: Some(Outline { color: 0x1FF_FFFF, width: 9, alpha: 1 }), ..TextStyle::plain(0x22) } },
                ItemStyle { index: 5, style: TextStyle::plain(0x99) },
            ],
        }
        .sanitized();
        assert_eq!(p.items.iter().map(|i| i.index).collect::<Vec<_>>(), vec![2, 5]);
        assert_eq!(p.style_for(2).unwrap().outline.unwrap().width, MAX_OUTLINE);
        assert_eq!(p.style_for(2).unwrap().outline.unwrap().color, 0xFF_FFFF);
        assert_eq!(p.style_for(5).unwrap().color, 0x55);
        assert_eq!(p.style_for(7).unwrap().color, 0x11, "mục không có trong bảng → uniform");
        let u = LabelsPaint { mode: PaintMode::Uniform, uniform: None, items: vec![] };
        assert!(u.style_for(0).is_none());
        assert!(!TextStyle::plain(1).has_effects());
        assert!(style().has_effects());
        assert_eq!(pipe_name(3), r"\\.\pipe\ShunguangDecor-explorer-3");
    }
}
