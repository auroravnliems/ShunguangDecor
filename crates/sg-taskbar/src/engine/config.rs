//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! Reads configuration following YASB's `config.yaml` schema exactly (received as a string, no file is read).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_yaml::Value;

/// The user's home directory (for `~` in widget paths).
pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\"))
}

/// Base directory for widgets' relative paths (images of the Aurora widget). The engine no longer
/// reads `~/.config/yasb`; if left empty, relative paths resolve against the working directory.
pub fn active_dir() -> PathBuf {
    PathBuf::new()
}

// ------------------------------------------------------------------ schema

fn t() -> bool {
    true
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct RootConfig {
    pub watch_config: bool,
    pub watch_stylesheet: bool,
    pub debug: bool,
    pub show_systray: bool,
    pub tooltip: TooltipOptions,
    pub bars: BTreeMap<String, BarConfig>,
    #[serde(skip)]
    pub bar_order: Vec<String>,
    pub widgets: BTreeMap<String, WidgetDef>,
}

impl Default for RootConfig {
    fn default() -> Self {
        let mut bars = BTreeMap::new();
        bars.insert("yasb-bar".to_string(), BarConfig::default());
        RootConfig {
            watch_config: true,
            watch_stylesheet: true,
            debug: false,
            show_systray: true,
            tooltip: TooltipOptions::default(),
            bars,
            bar_order: vec!["yasb-bar".into()],
            widgets: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BlurOptions {
    pub enabled: bool,
    pub dark_mode: bool,
    pub round_corners: bool,
    pub round_corners_type: String,
    pub border_color: String,
    /// real acrylic frosted glass (`ACCENT_ENABLE_ACRYLICBLURBEHIND`) instead of regular blur
    pub acrylic: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct TooltipOptions {
    pub offset: i32,
    pub blur_effect: BlurOptions,
}

impl Default for TooltipOptions {
    fn default() -> Self {
        TooltipOptions {
            offset: 5,
            blur_effect: BlurOptions { border_color: "None".into(), round_corners_type: "normal".into(), ..Default::default() },
        }
    }
}

/// In YASB a widget can be a string (another widget's name) or a {type, options} table.
#[derive(Clone, Debug, Default)]
pub struct WidgetDef {
    pub type_name: String,
    pub options: Value,
}

impl<'de> Deserialize<'de> for WidgetDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match v {
            Value::String(s) => Ok(WidgetDef { type_name: s, options: Value::Mapping(Default::default()) }),
            Value::Mapping(m) => {
                let type_name = m.get("type").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let options = m.get("options").cloned().unwrap_or(Value::Mapping(Default::default()));
                Ok(WidgetDef { type_name, options })
            }
            _ => Err(serde::de::Error::custom("widget phải là bảng {type, options}")),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct BarAlignment {
    pub position: String,
    pub align: String,
}

impl Default for BarAlignment {
    fn default() -> Self {
        BarAlignment { position: "top".into(), align: "center".into() }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct BarAnimation {
    pub enabled: bool,
    pub duration: u32,
    #[serde(rename = "type")]
    pub kind: String,
}

impl Default for BarAnimation {
    fn default() -> Self {
        BarAnimation { enabled: true, duration: 500, kind: "slide".into() }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct WindowFlags {
    pub always_on_top: bool,
    pub windows_app_bar: bool,
    pub hide_on_fullscreen: bool,
    pub hide_on_maximized: bool,
    pub auto_hide: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Dimensions {
    pub width: Value,
    pub height: i32,
}

impl Default for Dimensions {
    fn default() -> Self {
        Dimensions { width: Value::String("100%".into()), height: 30 }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Padding {
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BarWidgets {
    pub left: Vec<String>,
    pub center: Vec<String>,
    pub right: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct BarLayout {
    pub alignment: String,
    #[serde(default = "t")]
    pub stretch: bool,
}

impl Default for BarLayout {
    fn default() -> Self {
        BarLayout { alignment: "left".into(), stretch: true }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct BarLayouts {
    pub left: BarLayout,
    pub center: BarLayout,
    pub right: BarLayout,
}

impl Default for BarLayouts {
    fn default() -> Self {
        BarLayouts {
            left: BarLayout { alignment: "left".into(), stretch: true },
            center: BarLayout { alignment: "center".into(), stretch: true },
            right: BarLayout { alignment: "right".into(), stretch: true },
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct BarConfig {
    pub enabled: bool,
    pub screens: Vec<String>,
    pub class_name: String,
    pub context_menu: bool,
    pub alignment: BarAlignment,
    pub blur_effect: BlurOptions,
    pub animation: BarAnimation,
    pub window_flags: WindowFlags,
    pub dimensions: Dimensions,
    pub padding: Padding,
    pub widgets: BarWidgets,
    pub layouts: BarLayouts,
}

impl Default for BarConfig {
    fn default() -> Self {
        BarConfig {
            enabled: true,
            screens: vec!["*".into()],
            class_name: "yasb-bar".into(),
            context_menu: true,
            alignment: BarAlignment::default(),
            blur_effect: BlurOptions { round_corners_type: "normal".into(), border_color: "System".into(), ..Default::default() },
            animation: BarAnimation::default(),
            window_flags: WindowFlags::default(),
            dimensions: Dimensions::default(),
            padding: Padding::default(),
            widgets: BarWidgets::default(),
            layouts: BarLayouts::default(),
        }
    }
}

impl BarConfig {
    /// Width: a px number, "N%" or "auto" (None).
    pub fn width_px(&self, screen_w: i32) -> Option<i32> {
        match &self.dimensions.width {
            Value::Number(n) => Some(n.as_i64().unwrap_or(screen_w as i64) as i32),
            Value::String(s) if s.trim() == "auto" => None,
            Value::String(s) => {
                let s = s.trim();
                if let Some(p) = s.strip_suffix('%') {
                    Some((screen_w as f64 * p.trim().parse::<f64>().unwrap_or(100.0) / 100.0) as i32)
                } else {
                    Some(s.parse().unwrap_or(screen_w))
                }
            }
            _ => Some(screen_w),
        }
    }
}

// ------------------------------------------------------------------ loading

/// YASB (PyYAML) accepts the surrogate pair `\uDB80\uDF41`; serde_yaml does not → merge it into `\U000F0341`.
fn fix_surrogates(src: &str) -> String {
    let re = regex::Regex::new(r"\\u([dD][89abAB][0-9a-fA-F]{2})\\u([dD][c-fC-F][0-9a-fA-F]{2})").unwrap();
    re.replace_all(src, |c: &regex::Captures| {
        let hi = u32::from_str_radix(&c[1], 16).unwrap();
        let lo = u32::from_str_radix(&c[2], 16).unwrap();
        let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
        format!("\\U{cp:08X}")
    })
    .into_owned()
}

/// `$env:VAR_NAME` in any string is replaced by the environment variable (like YASB's `parse_env`).
fn expand_env(v: &mut Value) {
    let re = regex::Regex::new(r"(?i)\$env:([\w_]+)").unwrap();
    fn walk(v: &mut Value, re: &regex::Regex) {
        match v {
            Value::String(s) if s.to_ascii_lowercase().contains("$env:") => {
                *s = re
                    .replace_all(s, |c: &regex::Captures| std::env::var(&c[1]).unwrap_or_default())
                    .into_owned();
            }
            Value::Sequence(seq) => seq.iter_mut().for_each(|x| walk(x, re)),
            Value::Mapping(m) => m.iter_mut().for_each(|(_, x)| walk(x, re)),
            _ => {}
        }
    }
    walk(v, &re);
}

pub fn parse_yaml(src: &str) -> Result<RootConfig, String> {
    let fixed = fix_surrogates(src);
    let mut raw: Value = serde_yaml::from_str(&fixed).map_err(|e| e.to_string())?;
    if raw.is_null() {
        raw = Value::Mapping(Default::default());
    }
    expand_env(&mut raw);
    // keep the bar order as in the file
    let order: Vec<String> = raw
        .get("bars")
        .and_then(|b| b.as_mapping())
        .map(|m| m.keys().filter_map(|k| k.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let mut cfg: RootConfig = serde_yaml::from_value(raw).map_err(|e| e.to_string())?;
    cfg.bar_order = if order.is_empty() { cfg.bars.keys().cloned().collect() } else { order };
    Ok(cfg)
}

/// Reads the widget options into a struct with default values; on error, logs and uses the defaults.
pub fn options<T: serde::de::DeserializeOwned + Default>(name: &str, v: &Value) -> T {
    if v.is_null() {
        return T::default();
    }
    match serde_yaml::from_value::<T>(v.clone()) {
        Ok(o) => o,
        Err(e) => {
            crate::engine::log::error(&format!("Tuỳ chọn widget '{name}' sai: {e}. Dùng mặc định."));
            T::default()
        }
    }
}

/// The mouse callbacks shared by all widgets.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Callbacks {
    pub on_left: Option<String>,
    pub on_middle: Option<String>,
    pub on_right: Option<String>,
}

impl Callbacks {
    pub fn resolve(&self, left: &str, middle: &str, right: &str) -> [String; 3] {
        [
            self.on_left.clone().unwrap_or_else(|| left.into()),
            self.on_middle.clone().unwrap_or_else(|| middle.into()),
            self.on_right.clone().unwrap_or_else(|| right.into()),
        ]
    }
}

/// Shared popup options (blur, rounded corners, position).
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct PopupOptions {
    pub blur: bool,
    pub round_corners: bool,
    pub round_corners_type: String,
    pub border_color: String,
    pub alignment: String,
    pub direction: String,
    pub offset_top: i32,
    pub offset_left: i32,
}

impl Default for PopupOptions {
    fn default() -> Self {
        PopupOptions {
            blur: true,
            round_corners: true,
            round_corners_type: "normal".into(),
            border_color: "System".into(),
            alignment: "right".into(),
            direction: "down".into(),
            offset_top: 6,
            offset_left: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let c = parse_yaml(include_str!("testdata/config.yaml")).unwrap();
        assert_eq!(c.bar_order, vec!["primary-bar"]);
        let b = &c.bars["primary-bar"];
        assert_eq!(b.dimensions.height, 34);
        assert!(b.window_flags.windows_app_bar);
        assert_eq!(b.widgets.right, vec!["volume", "notifications", "power_menu"]);
        let pm = &c.widgets["power_menu"];
        let lock = pm.options["buttons"]["lock"][0].as_str().unwrap();
        assert_eq!(lock.chars().next().unwrap() as u32, 0xF0341);
    }

    #[test]
    fn blur_acrylic_option_parses() {
        let y = "bars:
  b:
    blur_effect:
      enabled: true
      acrylic: true
widgets: {}
";
        let c = parse_yaml(y).unwrap();
        assert!(c.bars["b"].blur_effect.enabled);
        assert!(c.bars["b"].blur_effect.acrylic);
        let d = parse_yaml("bars:
  b:
    blur_effect:
      enabled: true
widgets: {}
").unwrap();
        assert!(!d.bars["b"].blur_effect.acrylic);
    }
}
