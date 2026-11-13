//! Generator of YASB-style `config.yaml` + `styles.css` from [`TaskbarConfig`]. Templates come from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Odd values never corrupt the output: a bad color → default, numbers are clamped, font names are filtered by character.

use crate::model::{BarStyle, Blur, Edge, TaskbarConfig, WidgetEntry, WidgetKind, WinButtonPos, normalize_hex, rgb_of};
use serde_yaml::{Mapping, Value};

/// Generation result: the contents of the two files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    pub yaml: String,
    pub css: String,
}

/// CSS template (copied from TachyonTaskbar), with `{{...}}` placeholders.
const BASE_CSS: &str = include_str!("../res/base.css");
const BAR_NAME: &str = "primary-bar";

pub fn generate(cfg: &TaskbarConfig, lang: &str) -> Generated {
    let cfg = &cfg.sanitized();
    Generated { yaml: gen_yaml(cfg, lang), css: gen_css(cfg) }
}

/// `chrono` locale code for the clock according to the app language; unknown language → empty (English by default).
fn locale_of(lang: &str) -> &'static str {
    match lang.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase().as_str() {
        "vi" => "vi_VN",
        "en" => "en_US",
        "fr" => "fr_FR",
        "de" => "de_DE",
        "es" => "es_ES",
        "pt" => "pt_BR",
        "ru" => "ru_RU",
        "ja" => "ja_JP",
        "ko" => "ko_KR",
        "zh" => "zh_CN",
        _ => "",
    }
}

/// Center/left/right zones after applying `windows_button`: if the Windows button is centered, it moves to the start of the center zone.
fn zones(cfg: &TaskbarConfig) -> [Vec<&WidgetEntry>; 3] {
    let mut left: Vec<&WidgetEntry> = cfg.widgets.left.iter().collect();
    let mut center: Vec<&WidgetEntry> = cfg.widgets.center.iter().collect();
    let right: Vec<&WidgetEntry> = cfg.widgets.right.iter().collect();
    if cfg.windows_button == WinButtonPos::Center {
        let all = || cfg.widgets.left.iter().chain(&cfg.widgets.center).chain(&cfg.widgets.right);
        if let Some(wb) = all().find(|e| e.kind == WidgetKind::WindowsButton) {
            left.retain(|e| e.kind != WidgetKind::WindowsButton);
            center.retain(|e| e.kind != WidgetKind::WindowsButton);
            center.insert(0, wb);
        }
    }
    [left, center, right]
}

/// Widget type name in YAML (`yasb.<module>.<Name>`; the engine only needs the last segment).
fn type_name(k: WidgetKind) -> &'static str {
    match k {
        WidgetKind::WindowsButton => "yasb.windows_button.WindowsButtonWidget",
        WidgetKind::Search => "yasb.search.SearchWidget",
        WidgetKind::Apps => "yasb.taskbar.TaskbarWidget",
        WidgetKind::Tray => "yasb.systray.SystrayWidget",
        WidgetKind::Clock => "yasb.clock.ClockWidget",
        WidgetKind::Volume => "yasb.volume.VolumeWidget",
        WidgetKind::Network => "yasb.wifi.WifiWidget",
        WidgetKind::Battery => "yasb.battery.BatteryWidget",
        WidgetKind::Notifications => "yasb.notifications.NotificationsWidget",
        WidgetKind::Cpu => "yasb.cpu.CpuWidget",
        WidgetKind::Memory => "yasb.memory.MemoryWidget",
        WidgetKind::ActiveWindow => "yasb.active_window.ActiveWindowWidget",
        WidgetKind::PowerMenu => "yasb.power_menu.PowerMenuWidget",
    }
}

/// Default options of each widget (YAML), taken from TachyonTaskbar's sample config where available.
/// The `{DIR}` placeholder is the popup opening direction (see `build_options`).
fn default_options(k: WidgetKind) -> &'static str {
    match k {
        // Windows button: the logo is drawn with 4 squares, no options; Search: `label` = show text next to the magnifier.
        WidgetKind::Search => "label: false\n",
        WidgetKind::WindowsButton | WidgetKind::Apps | WidgetKind::Tray | WidgetKind::Battery => "",
        // Every default icon is a glyph from Segoe Fluent Icons / Segoe MDL2 Assets (built into Windows 11, no Nerd Font
        // needed). Icons sit inside a <span> to pick up the symbol font (`.widget .icon`); text normally uses the system font.
        // Ethernet U+E839, Wi-Fi U+E871..E874 (weak → strong) then U+E701 (full), no network U+EB55.
        WidgetKind::Network => {
            "label: <span>{wifi_icon}</span>\n\
             label_alt: <span>{wifi_icon}</span> {wifi_name}\n\
             ethernet_label: <span>{wifi_icon}</span>\n\
             ethernet_label_alt: <span>{wifi_icon}</span> {ip_addr}\n\
             wifi_icons: [\"\\uE871\", \"\\uE872\", \"\\uE873\", \"\\uE874\", \"\\uE701\"]\n\
             ethernet_icon: \"\\uE839\"\n\
             offline_icon: \"\\uEB55\"\n"
        }
        // CPU / RAM: plain text, so there is no glyph that could be missing.
        WidgetKind::Cpu => "label: 'CPU {info[percent][total]}%'\nlabel_alt: 'CPU {info[histograms][cpu_percent]}'\n",
        WidgetKind::Memory => "label: 'RAM {virtual_mem_free}'\nlabel_alt: 'RAM {virtual_mem_percent}%'\n",
        WidgetKind::Clock => {
            "label: '{%H:%M}<span>{alarm}</span>'\n\
             label_alt: '{%a, %d %b %H:%M}'\n\
             timezones: []\n\
             calendar:\n  blur: true\n  round_corners: true\n  alignment: center\n  direction: {DIR}\n  extended: false\n  show_years: true\n  show_holidays: false\n  show_week_numbers: true\n\
             alarm_icons:\n  enabled: \"\\uE916\"\n  disabled: \"\\uE916\"\n  snooze: \"\\uE823\"\n\
             callbacks:\n  on_left: toggle_calendar\n  on_middle: toggle_label\n  on_right: context_menu\n"
        }
        WidgetKind::Volume => {
            "label: <span>{icon}</span>\n\
             label_alt: <span>{icon}</span>{level}\n\
             tooltip: true\n\
             icons:\n  muted: \"\\uE74F\"\n  '10': \"\\uE992\"\n  '30': \"\\uE993\"\n  '60': \"\\uE994\"\n  '100': \"\\uE995\"\n\
             callbacks:\n  on_left: toggle_volume_menu\n  on_right: toggle_mute\n\
             audio_menu:\n  blur: true\n  round_corners: true\n  round_corners_type: normal\n  border_color: system\n  alignment: right\n  direction: {DIR}\n  show_apps: true\n  show_app_labels: false\n  show_app_icons: true\n  show_apps_expanded: false\n  app_icons:\n    toggle_down: \"\\uE70D\"\n    toggle_up: \"\\uE70E\"\n"
        }
        WidgetKind::Notifications => {
            "label: \"<span>\\uE7ED</span>\"\n\
             label_alt: '{count} notifications'\n\
             icons:\n  new: \"\\uE7ED\"\n  default: \"\\uE7ED\"\n\
             hide_empty: true\n\
             tooltip: false\n\
             callbacks:\n  on_left: toggle_notification\n  on_right: do_nothing\n  on_middle: do_nothing\n"
        }
        WidgetKind::PowerMenu => {
            "label: \"<span>\\uE712</span>\"\n\
             uptime: true\n\
             show_user: true\n\
             menu_style: popup\n\
             popup:\n  blur: true\n  round_corners: true\n  round_corners_type: normal\n  border_color: System\n  alignment: right\n  offset_left: 12\n\
             profile_image_size: 64\n\
             buttons:\n  lock: [\"\\uE72E\", Lock]\n  signout: [\"\\uE77B\", Sign out]\n  sleep: [\"\\uE708\", Sleep]\n  hibernate: [\"\\uE823\", Hibernate]\n  restart: [\"\\uE72C\", Restart]\n  shutdown: [\"\\uE7E8\", Shut Down]\n  cancel: ['', Cancel]\n"
        }
        WidgetKind::ActiveWindow => {
            "label: '{win[title]}'\n\
             label_no_window: ''\n\
             label_icon: true\n\
             label_icon_size: 16\n\
             max_length: 32\n\
             max_length_ellipsis: '...'\n\
             monitor_exclusive: false\n"
        }
    }
}

/// The widget's final options: defaults + `locale` (clock) + user customization (overrides each top-level key).
fn build_options(e: &WidgetEntry, lang: &str, edge: Edge) -> Value {
    // A bar on the bottom edge opens popups upward, on the top edge downward.
    let dir = if edge == Edge::Top { "down" } else { "up" };
    let text = default_options(e.kind).replace("{DIR}", dir);
    // A broken default string is caught by the test `all_widget_kinds_have_valid_default_options`; at runtime it does not panic.
    let mut m: Mapping = serde_yaml::from_str(&text).unwrap_or_default();
    if e.kind == WidgetKind::Clock {
        let loc = locale_of(lang);
        if !loc.is_empty() {
            m.insert("locale".into(), loc.into());
        }
    }
    if e.kind == WidgetKind::Search {
        m.insert("text".into(), if lang.starts_with("vi") { "Tìm kiếm" } else { "Search" }.into());
    }
    for (k, v) in &e.options {
        if let Ok(v) = serde_yaml::to_value(v) {
            m.insert(k.clone().into(), v);
        }
    }
    Value::Mapping(m)
}

fn gen_yaml(cfg: &TaskbarConfig, lang: &str) -> String {
    let [left, center, right] = zones(cfg);
    let names = |z: &[&WidgetEntry]| -> Value { Value::Sequence(z.iter().map(|e| e.kind.key().into()).collect()) };

    let mut widgets = Mapping::new();
    for e in left.iter().chain(&center).chain(&right) {
        let mut def = Mapping::new();
        def.insert("type".into(), type_name(e.kind).into());
        def.insert("options".into(), build_options(e, lang, cfg.edge));
        widgets.insert(e.kind.key().into(), Value::Mapping(def));
    }

    let floating = cfg.style == BarStyle::Floating;
    let map = |pairs: Vec<(&str, Value)>| -> Value {
        Value::Mapping(pairs.into_iter().map(|(k, v)| (Value::from(k), v)).collect())
    };
    let margin = if floating { i64::from(cfg.margin) } else { 0 };
    let bar = map(vec![
        ("enabled", true.into()),
        ("screens", Value::Sequence(vec![if cfg.all_monitors { "*".into() } else { "primary".into() }])),
        ("class_name", "yasb-bar".into()),
        ("alignment", map(vec![("position", (if cfg.edge == Edge::Top { "top" } else { "bottom" }).into()), ("align", "center".into())])),
        ("animation", map(vec![("enabled", true.into()), ("duration", 300.into())])),
        (
            "blur_effect",
            map(vec![
                ("enabled", (cfg.blur != Blur::None).into()),
                ("acrylic", (cfg.blur == Blur::Acrylic).into()),
                ("dark_mode", true.into()),
                ("round_corners", floating.into()),
            ]),
        ),
        (
            "window_flags",
            map(vec![
                ("always_on_top", false.into()),
                ("windows_app_bar", true.into()),
                ("hide_on_fullscreen", cfg.hide_on_fullscreen.into()),
                ("auto_hide", cfg.auto_hide.into()),
            ]),
        ),
        ("dimensions", map(vec![("width", "100%".into()), ("height", i64::from(cfg.height).into())])),
        (
            "padding",
            map(vec![("top", margin.into()), ("left", margin.into()), ("bottom", margin.into()), ("right", margin.into())]),
        ),
        ("widgets", map(vec![("left", names(&left)), ("center", names(&center)), ("right", names(&right))])),
    ]);

    let mut bars = Mapping::new();
    bars.insert(BAR_NAME.into(), bar);

    let mut root = Mapping::new();
    root.insert("watch_stylesheet".into(), false.into());
    root.insert("watch_config".into(), false.into());
    root.insert("debug".into(), false.into());
    root.insert("update_check".into(), false.into());
    root.insert("bars".into(), Value::Mapping(bars));
    root.insert("widgets".into(), Value::Mapping(widgets));

    let body = serde_yaml::to_string(&Value::Mapping(root)).unwrap_or_default();
    format!("# Tạo bởi ShunguangDecor — app ghi đè khi đổi cấu hình. Dựa trên YASB (MIT) / TachyonTaskbar (MIT).\n{body}")
}

/// Keeps only letters, digits, spaces, `-`, `_`, `.` so a font name cannot break the CSS.
fn sanitize_font(s: &str) -> String {
    let kept: String = s.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.')).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn rgba(hex: &str, fallback: &str, alpha: f64) -> String {
    let (r, g, b) = rgb_of(hex).or_else(|| rgb_of(fallback)).unwrap_or((0, 0, 0));
    format!("rgba({r}, {g}, {b}, {alpha})")
}

/// Darkens a color by about 12% (the hover accent color).
fn darken(hex: &str) -> String {
    let (r, g, b) = rgb_of(hex).unwrap_or((0x4c, 0xc2, 0xff));
    let d = |c: u8| (f64::from(c) * 0.88).round() as u8;
    format!("#{:02x}{:02x}{:02x}", d(r), d(g), d(b))
}

fn gen_css(cfg: &TaskbarConfig) -> String {
    let def = TaskbarConfig::default();
    let bg = normalize_hex(&cfg.background).unwrap_or(def.background.clone());
    let accent = normalize_hex(&cfg.accent).unwrap_or(def.accent.clone());
    let text = normalize_hex(&cfg.text_color).unwrap_or(def.text_color.clone());
    let alpha = f64::from(cfg.opacity.min(100)) / 100.0;
    let font = match sanitize_font(&cfg.font_family) {
        f if f.is_empty() => "\"Segoe UI Variable\", \"Segoe UI\"".to_string(),
        f => format!("\"{f}\", \"Segoe UI Variable\", \"Segoe UI\""),
    };
    let mut css = BASE_CSS
        .replace("{{bar_bg}}", &rgba(&bg, &def.background, alpha))
        .replace("{{accent}}", &accent)
        .replace("{{accent_hover}}", &darken(&accent))
        .replace("{{text}}", &text)
        .replace("{{font}}", &font)
        .replace("{{font_size}}", &cfg.font_size.clamp(9, 20).to_string());
    // The bar's border line is on the side opposite the screen edge; a floating bar gets rounded corners and a border all around.
    css.push_str("\n/* Theo cấu hình của app */\n");
    if cfg.style == BarStyle::Floating {
        css.push_str(".yasb-bar {\n    border: 1px solid var(--yasb-white-alpha-10);\n    border-radius: 10px;\n}\n");
    } else if cfg.edge == Edge::Bottom {
        css.push_str(".yasb-bar {\n    border-bottom: none;\n    border-top: 1px solid var(--yasb-bar-border);\n}\n");
    }
    css
}

#[cfg(test)]
mod tests {
    use crate::model::*;
    use super::*;

    #[test]
    fn generated_yaml_parses() {
        let g = generate(&TaskbarConfig::default(), "vi");
        let v: serde_yaml::Value = serde_yaml::from_str(&g.yaml).unwrap();
        let w = &v["bars"]["primary-bar"]["widgets"];
        let names = |z: &str| w[z].as_sequence().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(names("left"), ["windows_button", "search", "apps"]);
        assert_eq!(names("center"), ["clock"]);
        assert_eq!(names("right"), ["tray", "network", "volume", "battery", "notifications"]);
        // Every widget that is mentioned has a definition.
        for z in ["left", "center", "right"] {
            for n in names(z) {
                assert!(v["widgets"][n.as_str()]["type"].is_string(), "{n}");
            }
        }
        assert_eq!(v["bars"]["primary-bar"]["dimensions"]["height"].as_i64(), Some(36));
        assert_eq!(v["bars"]["primary-bar"]["alignment"]["position"].as_str(), Some("bottom"));
        assert_eq!(v["bars"]["primary-bar"]["screens"][0].as_str(), Some("*"));
        assert_eq!(v["widgets"]["clock"]["type"].as_str(), Some("yasb.clock.ClockWidget"));
        assert_eq!(v["widgets"]["windows_button"]["type"].as_str(), Some("yasb.windows_button.WindowsButtonWidget"));
    }

    #[test]
    fn generated_css_has_colors() {
        let c = TaskbarConfig { opacity: 50, background: "#102030".into(), ..TaskbarConfig::default() };
        let g = generate(&c, "en");
        assert!(g.css.contains("rgba(16, 32, 48, 0.5)"), "{}", &g.css[..400]);
        assert!(!g.css.contains("{{"));
    }

    #[test]
    fn generate_sanitizes_hand_built_config() {
        use std::collections::HashSet;
        let mut c = TaskbarConfig { height: 0, opacity: 200, accent: "zzz".into(), ..TaskbarConfig::default() };
        c.widgets.right.push(WidgetEntry::new(WidgetKind::Clock)); // duplicate of the center zone
        let v: serde_yaml::Value = serde_yaml::from_str(&generate(&c, "en").yaml).unwrap();
        assert_eq!(v["bars"]["primary-bar"]["dimensions"]["height"].as_i64(), Some(24));
        let big = TaskbarConfig { height: 5000, ..TaskbarConfig::default() };
        let v2: serde_yaml::Value = serde_yaml::from_str(&generate(&big, "en").yaml).unwrap();
        assert_eq!(v2["bars"]["primary-bar"]["dimensions"]["height"].as_i64(), Some(64));
        let w = &v["bars"]["primary-bar"]["widgets"];
        let mut all = Vec::new();
        for z in ["left", "center", "right"] {
            all.extend(w[z].as_sequence().unwrap().iter().map(|x| x.as_str().unwrap().to_string()));
        }
        assert_eq!(all.len(), all.iter().collect::<HashSet<_>>().len(), "{all:?}");
        assert!(generate(&c, "en").css.contains("rgba(28, 28, 28, 1)"));
    }

    /// F1: a clean Windows 11 has no Nerd Font. Rule: characters in the PUA range (U+E000..U+F8FF and U+F0000..) in the default YAML
    /// may only be glyphs present in Segoe Fluent Icons / Segoe MDL2 Assets, listed explicitly here (cross-checked against
    /// the cmap tables of SegoeIcons.ttf and segmdl2.ttf). Old Nerd Font codes (\u{eba9}, \u{f092e}, \u{f4bc}...) do not pass.
    #[test]
    fn default_yaml_uses_only_segoe_icon_glyphs() {
        const SEGOE: &[char] = &[
            '\u{E839}', '\u{E701}', '\u{E871}', '\u{E872}', '\u{E873}', '\u{E874}', '\u{EB55}', // network
            '\u{E74F}', '\u{E992}', '\u{E993}', '\u{E994}', '\u{E995}', '\u{E70D}', '\u{E70E}', // volume
            '\u{E7ED}', // notifications
            '\u{E916}', '\u{E823}', // clock: alarm
            '\u{E712}', '\u{E72E}', '\u{E77B}', '\u{E708}', '\u{E72C}', '\u{E7E8}', // power menu
        ];
        fn visit(v: &serde_yaml::Value, out: &mut Vec<char>) {
            match v {
                serde_yaml::Value::String(s) => out.extend(s.chars().filter(|c| matches!(*c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF))),
                serde_yaml::Value::Sequence(a) => a.iter().for_each(|x| visit(x, out)),
                serde_yaml::Value::Mapping(m) => m.iter().for_each(|(k, x)| {
                    visit(k, out);
                    visit(x, out);
                }),
                _ => {}
            }
        }
        let c = TaskbarConfig {
            widgets: Zones {
                left: WidgetKind::ALL[..5].iter().map(|k| WidgetEntry::new(*k)).collect(),
                center: WidgetKind::ALL[5..9].iter().map(|k| WidgetEntry::new(*k)).collect(),
                right: WidgetKind::ALL[9..].iter().map(|k| WidgetEntry::new(*k)).collect(),
            },
            ..TaskbarConfig::default()
        };
        let v: serde_yaml::Value = serde_yaml::from_str(&generate(&c, "vi").yaml).unwrap();
        let mut found = Vec::new();
        visit(&v, &mut found);
        assert!(found.len() > 15, "phải thấy các icon mặc định, thấy {}", found.len());
        for ch in found {
            assert!(SEGOE.contains(&ch), "icon U+{:04X} không nằm trong danh sách Segoe", ch as u32);
        }
    }

    #[test]
    fn all_widget_kinds_have_valid_default_options() {
        for k in WidgetKind::ALL {
            let text = default_options(k).replace("{DIR}", "up");
            let v: serde_yaml::Value = serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", k.key()));
            assert!(v.is_mapping() || v.is_null(), "{}", k.key());
        }
        let c = TaskbarConfig {
            widgets: Zones {
            left: WidgetKind::ALL[..5].iter().map(|k| WidgetEntry::new(*k)).collect(),
            center: WidgetKind::ALL[5..9].iter().map(|k| WidgetEntry::new(*k)).collect(),
            right: WidgetKind::ALL[9..].iter().map(|k| WidgetEntry::new(*k)).collect(),
            },
            ..TaskbarConfig::default()
        };
        let v: serde_yaml::Value = serde_yaml::from_str(&generate(&c, "vi").yaml).unwrap();
        for k in WidgetKind::ALL {
            let d = &v["widgets"][k.key()];
            assert_eq!(d["type"].as_str(), Some(type_name(k)));
            assert!(d["options"].is_mapping(), "{}", k.key());
            if !default_options(k).is_empty() {
                assert!(!d["options"].as_mapping().unwrap().is_empty(), "{}", k.key());
            }
        }
    }

    #[test]
    fn popups_follow_edge_and_acrylic_key() {
        let top = TaskbarConfig { edge: Edge::Top, blur: Blur::Acrylic, ..TaskbarConfig::default() };
        let y = generate(&top, "en").yaml;
        let v: serde_yaml::Value = serde_yaml::from_str(&y).unwrap();
        assert_eq!(v["widgets"]["clock"]["options"]["calendar"]["direction"].as_str(), Some("down"));
        assert_eq!(v["widgets"]["volume"]["options"]["audio_menu"]["direction"].as_str(), Some("down"));
        assert!(!y.contains("direction: up"));
        assert_eq!(v["bars"]["primary-bar"]["blur_effect"]["acrylic"].as_bool(), Some(true));
        let bottom = generate(&TaskbarConfig::default(), "en").yaml;
        let v: serde_yaml::Value = serde_yaml::from_str(&bottom).unwrap();
        assert_eq!(v["widgets"]["clock"]["options"]["calendar"]["direction"].as_str(), Some("up"));
        assert_eq!(v["bars"]["primary-bar"]["blur_effect"]["acrylic"].as_bool(), Some(false));
    }

    #[test]
    fn windows_button_center_and_hostile_values_stay_valid() {
        let c = TaskbarConfig {
            windows_button: WinButtonPos::Center,
            font_family: "Evil\"; } * { x".into(),
            ..TaskbarConfig::default()
        };
        let g = generate(&c, "vi");
        let v: serde_yaml::Value = serde_yaml::from_str(&g.yaml).unwrap();
        let w = &v["bars"]["primary-bar"]["widgets"];
        assert_eq!(w["center"][0].as_str(), Some("windows_button"));
        assert_eq!(w["left"][0].as_str(), Some("search"));
        assert!(!g.css.contains("Evil\"; }"));
    }
}
