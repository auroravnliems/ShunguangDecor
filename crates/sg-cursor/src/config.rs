//! Configuration of the Cursor part (`config/cursor.toml`). Fault-tolerant parsing: wrong type → default.

use crate::ScaleMode;

pub const MIN_SIZE: u32 = 32;
pub const MAX_SIZE: u32 = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorConfig {
    /// Id of the scheme currently selected in the library.
    pub scheme: Option<String>,
    pub size: u32,
    pub scaling: ScaleMode,
}

impl Default for CursorConfig {
    fn default() -> Self {
        CursorConfig { scheme: None, size: MIN_SIZE, scaling: ScaleMode::Sharp }
    }
}

impl CursorConfig {
    /// Missing/wrong type → default; `size` is rounded to a multiple of 8, then clamped to 32..=256.
    pub fn from_table(t: &toml::Table) -> CursorConfig {
        let d = CursorConfig::default();
        let scheme = t.get("scheme").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let size = match t.get("size") {
            Some(toml::Value::Integer(n)) => (*n).clamp(0, 100_000) as u32,
            Some(toml::Value::Float(f)) if f.is_finite() => f.clamp(0.0, 100_000.0).round() as u32,
            _ => d.size,
        };
        let size = ((size + 4) / 8 * 8).clamp(MIN_SIZE, MAX_SIZE);
        let scaling = match t.get("scaling").and_then(|v| v.as_str()) {
            Some("smooth") => ScaleMode::Smooth,
            _ => ScaleMode::Sharp,
        };
        CursorConfig { scheme, size, scaling }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(s: &str) -> toml::Table {
        s.parse().unwrap()
    }

    #[test]
    fn config_defaults_and_clamp() {
        assert_eq!(CursorConfig::from_table(&table("")), CursorConfig { scheme: None, size: 32, scaling: ScaleMode::Sharp });
        assert_eq!(CursorConfig::from_table(&table("size = 999")).size, 256);
        assert_eq!(CursorConfig::from_table(&table("size = 37")).size, 40);
        assert_eq!(CursorConfig::from_table(&table("size = -5")).size, 32);
        assert_eq!(CursorConfig::from_table(&table("size = \"x\"")).size, 32);
        assert_eq!(CursorConfig::from_table(&table("size = 64.4")).size, 64);
        let c = CursorConfig::from_table(&table("scheme = \" abc \"\nscaling = \"smooth\""));
        assert_eq!((c.scheme.as_deref(), c.scaling), (Some("abc"), ScaleMode::Smooth));
        assert_eq!(CursorConfig::from_table(&table("scheme = 5\nscaling = 7")).scaling, ScaleMode::Sharp);
        assert_eq!(CursorConfig::from_table(&table("scheme = \"\"")).scheme, None);
    }
}
