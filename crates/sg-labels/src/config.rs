//! Configuration of the desktop label part (`config/labels.toml`). Fault-tolerant reading: missing/wrong type → default, out-of-range number → clamped.
//!
//! ```toml
//! mode = "auto"          # auto | custom
//! outline = true         # Auto: add a thin outline/shadow so it stays readable on a speckled background
//! [custom]
//! color = "#ffffff"
//! outline_color = "#000000"
//! outline_width = 1      # 0..=4 (0 = no outline)
//! shadow_color = "#000000"
//! shadow_alpha = 0       # 0..=100 (%)
//! shadow_dx = 1          # -6..=6
//! shadow_dy = 1
//! backdrop_color = "#000000"
//! backdrop_alpha = 0     # 0..=100 (%)
//! ```

use sg_explorer_proto::{Backdrop, MAX_OUTLINE, MAX_SHADOW_OFFSET, Outline, Shadow, TextStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Each file name turns light/dark automatically based on the background right behind it.
    #[default]
    Auto,
    Custom,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Custom => "custom",
        }
    }
}

/// A text style chosen by the user. Colors are `0xRRGGBB`, opacity in %.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyleCfg {
    pub color: u32,
    pub outline_color: u32,
    pub outline_width: u8,
    pub shadow_color: u32,
    pub shadow_alpha: u8,
    pub shadow_dx: i8,
    pub shadow_dy: i8,
    pub backdrop_color: u32,
    pub backdrop_alpha: u8,
}

impl Default for TextStyleCfg {
    fn default() -> Self {
        TextStyleCfg {
            color: 0xFF_FFFF,
            outline_color: 0x00_0000,
            outline_width: 1,
            shadow_color: 0x00_0000,
            shadow_alpha: 0,
            shadow_dx: 1,
            shadow_dy: 1,
            backdrop_color: 0x00_0000,
            backdrop_alpha: 0,
        }
    }
}

fn pct_to_alpha(p: u8) -> u8 {
    ((p.min(100) as u32 * 255 + 50) / 100) as u8
}

impl TextStyleCfg {
    /// The style sent down to the DLL.
    pub fn to_style(&self) -> TextStyle {
        TextStyle {
            color: self.color,
            outline: (self.outline_width > 0).then_some(Outline { color: self.outline_color, width: self.outline_width, alpha: 255 }),
            shadow: (self.shadow_alpha > 0).then_some(Shadow {
                color: self.shadow_color,
                alpha: pct_to_alpha(self.shadow_alpha),
                dx: self.shadow_dx,
                dy: self.shadow_dy,
            }),
            backdrop: (self.backdrop_alpha > 0).then_some(Backdrop { color: self.backdrop_color, alpha: pct_to_alpha(self.backdrop_alpha) }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelsConfig {
    pub mode: Mode,
    /// Auto mode: thin outline/shadow (on by default).
    pub outline: bool,
    pub custom: TextStyleCfg,
}

impl Default for LabelsConfig {
    fn default() -> Self {
        LabelsConfig { mode: Mode::Auto, outline: true, custom: TextStyleCfg::default() }
    }
}

/// `#rrggbb` or `#rgb` (case-insensitive, the `#` may be missing).
pub fn parse_hex_color(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    match h.len() {
        6 => u32::from_str_radix(h, 16).ok(),
        3 => {
            let v = u32::from_str_radix(h, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
            Some((r * 17) << 16 | (g * 17) << 8 | (b * 17))
        }
        _ => None,
    }
}

pub fn hex(c: u32) -> String {
    format!("#{:06x}", c & 0xFF_FFFF)
}

fn int(t: &toml::Table, k: &str) -> Option<i64> {
    match t.get(k) {
        Some(toml::Value::Integer(n)) => Some(*n),
        Some(toml::Value::Float(f)) if f.is_finite() => Some(f.round() as i64),
        _ => None,
    }
}

fn color(t: &toml::Table, k: &str, d: u32) -> u32 {
    t.get(k).and_then(toml::Value::as_str).and_then(parse_hex_color).unwrap_or(d)
}

impl LabelsConfig {
    pub fn from_table(t: &toml::Table) -> LabelsConfig {
        let d = LabelsConfig::default();
        let mode = match t.get("mode").and_then(toml::Value::as_str) {
            Some("custom") => Mode::Custom,
            Some("auto") => Mode::Auto,
            Some(other) => {
                tracing::warn!(target: "sg::labels", "labels.toml: unknown mode {other:?}, using auto");
                Mode::Auto
            }
            None => d.mode,
        };
        let outline = t.get("outline").and_then(toml::Value::as_bool).unwrap_or(d.outline);
        let empty = toml::Table::new();
        let c = t.get("custom").and_then(toml::Value::as_table).unwrap_or(&empty);
        let dc = d.custom;
        let off = MAX_SHADOW_OFFSET as i64;
        let custom = TextStyleCfg {
            color: color(c, "color", dc.color),
            outline_color: color(c, "outline_color", dc.outline_color),
            outline_width: int(c, "outline_width").map(|v| v.clamp(0, MAX_OUTLINE as i64) as u8).unwrap_or(dc.outline_width),
            shadow_color: color(c, "shadow_color", dc.shadow_color),
            shadow_alpha: int(c, "shadow_alpha").map(|v| v.clamp(0, 100) as u8).unwrap_or(dc.shadow_alpha),
            shadow_dx: int(c, "shadow_dx").map(|v| v.clamp(-off, off) as i8).unwrap_or(dc.shadow_dx),
            shadow_dy: int(c, "shadow_dy").map(|v| v.clamp(-off, off) as i8).unwrap_or(dc.shadow_dy),
            backdrop_color: color(c, "backdrop_color", dc.backdrop_color),
            backdrop_alpha: int(c, "backdrop_alpha").map(|v| v.clamp(0, 100) as u8).unwrap_or(dc.backdrop_alpha),
        };
        LabelsConfig { mode, outline, custom }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(s: &str) -> toml::Table {
        s.parse().unwrap()
    }

    #[test]
    fn config_defaults_and_clamps() {
        let d = LabelsConfig::from_table(&table(""));
        assert_eq!(d, LabelsConfig::default());
        assert_eq!((d.mode, d.outline), (Mode::Auto, true));
        assert_eq!(d.custom.color, 0xFFFFFF);
        assert_eq!(d.custom.outline_width, 1);
        let c = LabelsConfig::from_table(&table(
            r##"mode = "custom"
outline = false
[custom]
color = "#FF69B4"
outline_color = "#123"
outline_width = 9
shadow_alpha = 150
shadow_dx = -20
shadow_dy = 3.6
backdrop_color = "không phải màu"
backdrop_alpha = -4
"##,
        ));
        assert_eq!((c.mode, c.outline), (Mode::Custom, false));
        assert_eq!(c.custom.color, 0xFF69B4);
        assert_eq!(c.custom.outline_color, 0x112233);
        assert_eq!(c.custom.outline_width, 4, "độ dày kẹp về 4");
        assert_eq!(c.custom.shadow_alpha, 100);
        assert_eq!(c.custom.shadow_dx, -6);
        assert_eq!(c.custom.shadow_dy, 4);
        assert_eq!(c.custom.backdrop_color, 0, "màu hỏng → mặc định");
        assert_eq!(c.custom.backdrop_alpha, 0);
        // Completely wrong type → default, no error.
        let w = LabelsConfig::from_table(&table("mode = 5\noutline = \"có\"\ncustom = 3"));
        assert_eq!(w, LabelsConfig::default());
        assert_eq!(LabelsConfig::from_table(&table("mode = \"rainbow\"")).mode, Mode::Auto);
    }

    #[test]
    fn custom_to_style() {
        let mut c = TextStyleCfg::default();
        let s = c.to_style();
        assert_eq!(s.outline, Some(Outline { color: 0, width: 1, alpha: 255 }));
        assert!(s.shadow.is_none() && s.backdrop.is_none());
        c.outline_width = 0;
        c.shadow_alpha = 50;
        c.backdrop_alpha = 100;
        let s = c.to_style();
        assert!(s.outline.is_none());
        assert_eq!(s.shadow.unwrap().alpha, 128);
        assert_eq!(s.backdrop.unwrap().alpha, 255);
        assert_eq!(hex(0xff69b4), "#ff69b4");
        assert_eq!(parse_hex_color("abc"), Some(0xAABBCC));
        assert_eq!(parse_hex_color("#12345"), None);
    }
}
