//! The bar's user-friendly configuration (`config/taskbar.toml`). Fault-tolerant parsing: wrong type/unknown value → default with a warning,
//! out-of-range numbers → clamped, unknown widgets dropped, repeated widgets of the same kind keep only the first.
//!
//! ```toml
//! edge = "bottom"             # top | bottom
//! height = 36                 # 24..=64
//! style = "docked"            # docked | floating
//! margin = 8                  # 0..=32, only used when floating
//! background = "#1c1c1c"
//! opacity = 85                # 0..=100
//! blur = "blur"               # none | blur | acrylic
//! windows_button = "left"     # left | center
//! native = "reveal"           # keep | hide | reveal
//! [widgets]
//! left = ["windows_button", "search", "apps"]
//! right = ["tray", { kind = "network" }]
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Top,
    #[default]
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarStyle {
    #[default]
    Docked,
    Floating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blur {
    None,
    #[default]
    Blur,
    Acrylic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WinButtonPos {
    #[default]
    Left,
    Center,
}

/// How to handle the native Windows taskbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeMode {
    Keep,
    Hide,
    /// Hidden; shows immediately when the mouse touches the edge.
    #[default]
    Reveal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    WindowsButton,
    Search,
    Apps,
    Tray,
    Clock,
    Volume,
    Network,
    Battery,
    Notifications,
    Cpu,
    Memory,
    ActiveWindow,
    PowerMenu,
}

impl WidgetKind {
    pub const ALL: [WidgetKind; 13] = [
        WidgetKind::WindowsButton,
        WidgetKind::Search,
        WidgetKind::Apps,
        WidgetKind::Tray,
        WidgetKind::Clock,
        WidgetKind::Volume,
        WidgetKind::Network,
        WidgetKind::Battery,
        WidgetKind::Notifications,
        WidgetKind::Cpu,
        WidgetKind::Memory,
        WidgetKind::ActiveWindow,
        WidgetKind::PowerMenu,
    ];

    /// snake_case key (also the widget name in YAML).
    pub fn key(self) -> &'static str {
        match self {
            WidgetKind::WindowsButton => "windows_button",
            WidgetKind::Search => "search",
            WidgetKind::Apps => "apps",
            WidgetKind::Tray => "tray",
            WidgetKind::Clock => "clock",
            WidgetKind::Volume => "volume",
            WidgetKind::Network => "network",
            WidgetKind::Battery => "battery",
            WidgetKind::Notifications => "notifications",
            WidgetKind::Cpu => "cpu",
            WidgetKind::Memory => "memory",
            WidgetKind::ActiveWindow => "active_window",
            WidgetKind::PowerMenu => "power_menu",
        }
    }

    pub fn parse(s: &str) -> Option<WidgetKind> {
        WidgetKind::ALL.into_iter().find(|k| k.key() == s.trim())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WidgetEntry {
    pub kind: WidgetKind,
    /// Per-widget customization (overrides the default options); may be empty.
    pub options: toml::Table,
}

impl WidgetEntry {
    pub fn new(kind: WidgetKind) -> WidgetEntry {
        WidgetEntry { kind, options: toml::Table::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Zones {
    pub left: Vec<WidgetEntry>,
    pub center: Vec<WidgetEntry>,
    pub right: Vec<WidgetEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskbarConfig {
    pub edge: Edge,
    pub height: u32,
    pub style: BarStyle,
    pub margin: u32,
    pub background: String,
    pub opacity: u8,
    pub blur: Blur,
    pub accent: String,
    pub font_family: String,
    pub font_size: u32,
    pub text_color: String,
    pub windows_button: WinButtonPos,
    pub native: NativeMode,
    pub widgets: Zones,
    pub hide_on_fullscreen: bool,
    pub hot_corner: Option<Corner>,
    pub all_monitors: bool,
    pub auto_hide: bool,
}

impl Default for TaskbarConfig {
    fn default() -> Self {
        use WidgetKind as K;
        let zone = |ks: &[K]| ks.iter().map(|k| WidgetEntry::new(*k)).collect();
        TaskbarConfig {
            edge: Edge::Bottom,
            height: 36,
            style: BarStyle::Docked,
            margin: 8,
            background: "#1c1c1c".into(),
            opacity: 85,
            blur: Blur::Blur,
            accent: "#4cc2ff".into(),
            font_family: "Segoe UI Variable".into(),
            font_size: 12,
            text_color: "#ffffff".into(),
            windows_button: WinButtonPos::Left,
            native: NativeMode::Reveal,
            widgets: Zones {
                left: zone(&[K::WindowsButton, K::Search, K::Apps]),
                center: zone(&[K::Clock]),
                right: zone(&[K::Tray, K::Network, K::Volume, K::Battery, K::Notifications]),
            },
            hide_on_fullscreen: true,
            hot_corner: None,
            all_monitors: true,
            auto_hide: false,
        }
    }
}

/// `#rrggbb` or `#rgb` (case-insensitive, the `#` may be missing) → lowercase `#rrggbb`.
pub fn normalize_hex(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        6 => Some(format!("#{}", h.to_ascii_lowercase())),
        3 => Some(format!("#{}", h.chars().flat_map(|c| [c, c]).collect::<String>().to_ascii_lowercase())),
        _ => None,
    }
}

/// Decode a color → (r, g, b); `None` if invalid.
pub fn rgb_of(hex: &str) -> Option<(u8, u8, u8)> {
    let h = normalize_hex(hex)?;
    let v = u32::from_str_radix(&h[1..], 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Valid ranges for the numbers (shared by `from_table` and `sanitized`).
const HEIGHT: (i64, i64) = (24, 64);
const MARGIN: (i64, i64) = (0, 32);
const FONT_SIZE: (i64, i64) = (9, 20);
const OPACITY: (i64, i64) = (0, 100);

fn clamp_u32(n: u32, (lo, hi): (i64, i64)) -> u32 {
    i64::from(n).clamp(lo, hi) as u32
}

fn parse_enum<T: for<'de> Deserialize<'de>>(v: &toml::Value) -> Option<T> {
    v.clone().try_into().ok()
}

/// Read an integer from `t[key]`, clamped to `lo..=hi`; missing key → `None`.
fn read_int(t: &toml::Table, key: &str, lo: i64, hi: i64, warns: &mut Vec<String>) -> Option<u32> {
    let v = t.get(key)?;
    let Some(n) = v.as_integer().or_else(|| v.as_float().filter(|f| f.is_finite()).map(|f| f.round() as i64)) else {
        warns.push(format!("{key}: cần một số, dùng mặc định"));
        return None;
    };
    if n < lo || n > hi {
        warns.push(format!("{key}: {n} nằm ngoài {lo}..={hi}, đã kẹp lại"));
    }
    Some(n.clamp(lo, hi) as u32)
}

fn read_color(t: &toml::Table, key: &str, slot: &mut String, warns: &mut Vec<String>) {
    let Some(v) = t.get(key) else { return };
    match v.as_str().and_then(normalize_hex) {
        Some(h) => *slot = h,
        None => warns.push(format!("{key}: màu không hợp lệ, dùng mặc định")),
    }
}

fn read_bool(t: &toml::Table, key: &str, slot: &mut bool, warns: &mut Vec<String>) {
    let Some(v) = t.get(key) else { return };
    match v.as_bool() {
        Some(b) => *slot = b,
        None => warns.push(format!("{key}: cần true/false, dùng mặc định")),
    }
}

fn read_enum<T: for<'de> Deserialize<'de>>(t: &toml::Table, key: &str, slot: &mut T, warns: &mut Vec<String>) {
    let Some(v) = t.get(key) else { return };
    match parse_enum(v) {
        Some(x) => *slot = x,
        None => warns.push(format!("{key}: giá trị không hợp lệ, dùng mặc định")),
    }
}

impl TaskbarConfig {
    pub fn from_table(t: &toml::Table) -> (TaskbarConfig, Vec<String>) {
        let mut c = TaskbarConfig::default();
        let mut warns = Vec::new();
        let w = &mut warns;

        read_enum(t, "edge", &mut c.edge, w);
        read_enum(t, "style", &mut c.style, w);
        read_enum(t, "blur", &mut c.blur, w);
        read_enum(t, "windows_button", &mut c.windows_button, w);
        read_enum(t, "native", &mut c.native, w);
        if let Some(n) = read_int(t, "height", HEIGHT.0, HEIGHT.1, w) {
            c.height = n;
        }
        if let Some(n) = read_int(t, "margin", MARGIN.0, MARGIN.1, w) {
            c.margin = n;
        }
        if let Some(n) = read_int(t, "font_size", FONT_SIZE.0, FONT_SIZE.1, w) {
            c.font_size = n;
        }
        if let Some(n) = read_int(t, "opacity", OPACITY.0, OPACITY.1, w) {
            c.opacity = n as u8;
        }
        read_color(t, "background", &mut c.background, w);
        read_color(t, "accent", &mut c.accent, w);
        read_color(t, "text_color", &mut c.text_color, w);
        if let Some(v) = t.get("font_family") {
            match v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                Some(s) => c.font_family = s.to_string(),
                None => w.push("font_family: cần một chuỗi, dùng mặc định".into()),
            }
        }
        read_bool(t, "hide_on_fullscreen", &mut c.hide_on_fullscreen, w);
        read_bool(t, "all_monitors", &mut c.all_monitors, w);
        read_bool(t, "auto_hide", &mut c.auto_hide, w);
        if let Some(v) = t.get("hot_corner") {
            match parse_enum::<Corner>(v) {
                Some(x) => c.hot_corner = Some(x),
                None if v.as_str().is_some_and(|s| s.is_empty() || s == "none") => c.hot_corner = None,
                None => w.push("hot_corner: giá trị không hợp lệ, tắt góc nóng".into()),
            }
        }

        if let Some(wt) = t.get("widgets") {
            let Some(wt) = wt.as_table() else {
                w.push("widgets: cần một bảng left/center/right, dùng mặc định".into());
                return (c, warns);
            };
            let mut seen = std::collections::HashSet::new();
            for (name, slot) in [("left", &mut c.widgets.left), ("center", &mut c.widgets.center), ("right", &mut c.widgets.right)] {
                // Zones that aren't declared keep their defaults.
                let Some(v) = wt.get(name) else { continue };
                let Some(arr) = v.as_array() else {
                    warns.push(format!("widgets.{name}: cần một danh sách, dùng mặc định"));
                    continue;
                };
                let mut out = Vec::new();
                for item in arr {
                    let Some(entry) = read_entry(item, name, &mut warns) else { continue };
                    if !seen.insert(entry.kind) {
                        warns.push(format!("widgets.{name}: {} đã có ở nơi khác, bỏ bản lặp", entry.kind.key()));
                        continue;
                    }
                    out.push(entry);
                }
                *slot = out;
            }
        }
        (c, warns)
    }

    /// A copy with numbers clamped, invalid colors replaced by defaults, and repeated widgets removed (first one kept): the input to `generate`,
    /// so a hand-built config (all fields are `pub`) also yields a valid bar.
    pub fn sanitized(&self) -> TaskbarConfig {
        let def = TaskbarConfig::default();
        let mut c = self.clone();
        c.height = clamp_u32(c.height, HEIGHT);
        c.margin = clamp_u32(c.margin, MARGIN);
        c.font_size = clamp_u32(c.font_size, FONT_SIZE);
        c.opacity = c.opacity.min(OPACITY.1 as u8);
        c.background = normalize_hex(&c.background).unwrap_or(def.background);
        c.accent = normalize_hex(&c.accent).unwrap_or(def.accent);
        c.text_color = normalize_hex(&c.text_color).unwrap_or(def.text_color);
        if c.font_family.trim().is_empty() {
            c.font_family = def.font_family;
        }
        let mut seen = std::collections::HashSet::new();
        for z in [&mut c.widgets.left, &mut c.widgets.center, &mut c.widgets.right] {
            z.retain(|e| seen.insert(e.kind));
        }
        c
    }

    /// The inverse of `from_table` (used to write `config/taskbar.toml`).
    pub fn to_table(&self) -> toml::Table {
        fn put<T: Serialize>(t: &mut toml::Table, k: &str, v: &T) {
            if let Ok(v) = toml::Value::try_from(v) {
                t.insert(k.into(), v);
            }
        }
        let mut t = toml::Table::new();
        put(&mut t, "edge", &self.edge);
        put(&mut t, "height", &self.height);
        put(&mut t, "style", &self.style);
        put(&mut t, "margin", &self.margin);
        put(&mut t, "background", &self.background);
        put(&mut t, "opacity", &self.opacity);
        put(&mut t, "blur", &self.blur);
        put(&mut t, "accent", &self.accent);
        put(&mut t, "font_family", &self.font_family);
        put(&mut t, "font_size", &self.font_size);
        put(&mut t, "text_color", &self.text_color);
        put(&mut t, "windows_button", &self.windows_button);
        put(&mut t, "native", &self.native);
        put(&mut t, "hide_on_fullscreen", &self.hide_on_fullscreen);
        put(&mut t, "all_monitors", &self.all_monitors);
        put(&mut t, "auto_hide", &self.auto_hide);
        if let Some(c) = &self.hot_corner {
            put(&mut t, "hot_corner", c);
        }
        let zone = |z: &[WidgetEntry]| -> toml::Value {
            z.iter()
                .map(|e| {
                    if e.options.is_empty() {
                        toml::Value::String(e.kind.key().into())
                    } else {
                        let mut m = e.options.clone();
                        m.insert("kind".into(), e.kind.key().into());
                        toml::Value::Table(m)
                    }
                })
                .collect::<Vec<_>>()
                .into()
        };
        let mut w = toml::Table::new();
        w.insert("left".into(), zone(&self.widgets.left));
        w.insert("center".into(), zone(&self.widgets.center));
        w.insert("right".into(), zone(&self.widgets.right));
        t.insert("widgets".into(), w.into());
        t
    }
}

/// One element of a widget list: the string `"clock"` or a table `{ kind = "clock", ...options }`.
fn read_entry(v: &toml::Value, zone: &str, warns: &mut Vec<String>) -> Option<WidgetEntry> {
    let (name, options) = match v {
        toml::Value::String(s) => (s.clone(), toml::Table::new()),
        toml::Value::Table(t) => {
            let mut opts = t.clone();
            let name = opts.remove("kind").and_then(|k| k.as_str().map(str::to_string)).unwrap_or_default();
            (name, opts)
        }
        _ => {
            warns.push(format!("widgets.{zone}: bỏ một mục không phải tên widget"));
            return None;
        }
    };
    match WidgetKind::parse(&name) {
        Some(kind) => Some(WidgetEntry { kind, options }),
        None => {
            warns.push(format!("widgets.{zone}: widget \"{name}\" không tồn tại, đã bỏ"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let c = TaskbarConfig::default();
        assert_eq!(c.edge, Edge::Bottom);
        assert_eq!(c.windows_button, WinButtonPos::Left);
        assert_eq!(c.native, NativeMode::Reveal);
        assert_eq!(c.hot_corner, None);
        assert_eq!(c.height, 36);
        assert!(c.hide_on_fullscreen && c.all_monitors && !c.auto_hide);
        let kinds = |z: &[WidgetEntry]| z.iter().map(|e| e.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&c.widgets.left), [WidgetKind::WindowsButton, WidgetKind::Search, WidgetKind::Apps]);
        assert_eq!(kinds(&c.widgets.center), [WidgetKind::Clock]);
        assert_eq!(
            kinds(&c.widgets.right),
            [WidgetKind::Tray, WidgetKind::Network, WidgetKind::Volume, WidgetKind::Battery, WidgetKind::Notifications]
        );
    }

    #[test]
    fn config_clamps_and_drops_unknown_widgets() {
        let t: toml::Table = toml::from_str(
            r##"
            height = 5000
            margin = 99
            font_size = 1
            opacity = 250
            background = "#zzz"
            accent = "#ABC"
            edge = "left"
            [widgets]
            left = ["clock", "does_not_exist", 7]
            right = [{ kind = "cpu", label = "x" }]
            "##,
        )
        .unwrap();
        let (c, warns) = TaskbarConfig::from_table(&t);
        assert_eq!(c.height, 64);
        assert_eq!(c.margin, 32);
        assert_eq!(c.font_size, 9);
        assert_eq!(c.opacity, 100);
        assert_eq!(c.background, TaskbarConfig::default().background);
        assert_eq!(c.accent, "#aabbcc");
        assert_eq!(c.edge, Edge::Bottom);
        assert_eq!(c.widgets.left.len(), 1);
        assert_eq!(c.widgets.left[0].kind, WidgetKind::Clock);
        assert_eq!(c.widgets.right[0].kind, WidgetKind::Cpu);
        assert_eq!(c.widgets.right[0].options.get("label").and_then(|v| v.as_str()), Some("x"));
        // Zones that aren't declared keep their defaults.
        assert_eq!(c.widgets.center.len(), 1);
        assert!(warns.len() >= 6, "{warns:?}");
        let (_, w2) = TaskbarConfig::from_table(&toml::Table::new());
        assert!(w2.is_empty());
    }

    #[test]
    fn duplicate_widget_kept_once() {
        let t: toml::Table = toml::from_str(
            r#"
            [widgets]
            left = ["clock", "apps"]
            center = ["clock"]
            right = ["apps", "tray"]
            "#,
        )
        .unwrap();
        let (c, warns) = TaskbarConfig::from_table(&t);
        assert_eq!(c.widgets.left.len(), 2);
        assert_eq!(c.widgets.center.len(), 0);
        assert_eq!(c.widgets.right.len(), 1);
        assert_eq!(c.widgets.right[0].kind, WidgetKind::Tray);
        assert_eq!(warns.len(), 2);
    }

    #[test]
    fn to_table_roundtrip() {
        let c = TaskbarConfig {
            hot_corner: Some(Corner::TopRight),
            style: BarStyle::Floating,
            blur: Blur::Acrylic,
            ..TaskbarConfig::default()
        };
        let (back, w) = TaskbarConfig::from_table(&c.to_table());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(back, c);
    }
}
