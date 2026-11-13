//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! Widgets: the equivalent of YASB's `BaseWidget`. Each widget rebuilds its own node tree
//! (with the same class names as the Python version) and receives mouse/system events via `event`.

pub mod active_window;
pub mod aurora;
pub mod battery;
pub mod clock;
pub mod cpu;
pub mod home;
pub mod memory;
pub mod notifications;
pub mod power_menu;
pub mod search;
pub mod systray;
pub mod taskbar;
pub mod unikey;
pub mod volume;
pub mod wifi;
pub mod windows_button;

use regex::Regex;
use serde_yaml::Value;

use crate::engine::config::PopupOptions;
use crate::engine::css::Edges;
use crate::engine::ui::tree::*;
use crate::engine::win::MouseBtn;

/// Special node id: clicking the widget body → runs the on_left/on_middle/on_right callback.
pub const BASE: u32 = 0;

#[derive(Debug, Clone)]
pub enum SysEv {
    Foreground(isize),
    NameChange(isize),
    Destroy(isize),
    MoveSizeEnd(isize),
    AudioChanged,
    AudioDevicesChanged,
    Notifications(u32),
    /// system tray icons changed (added/modified/removed)
    Systray,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // `released` is for the beep when a slider is released
pub enum Ev {
    Click { id: u32, btn: MouseBtn },
    Press { id: u32 },
    Wheel { id: u32, delta: i32 },
    Slider { id: u32, value: i32, released: bool },
    Timer,
    /// the mouse rested on a node with `on_hover` long enough
    Hover { id: u32 },
    Sys(SysEv),
    PopupClosed,
    Init,
}

#[derive(Clone, Debug)]
pub struct PopupSpec {
    pub kind: u32,
    pub class: String,
    pub opts: PopupOptions,
    /// the popup's layout margins (`layout.setContentsMargins`)
    pub margins: Edges,
    pub dir: Dir,
}

/// An item of a QMenu-style context menu (`.context-menu`).
#[derive(Clone, Debug)]
pub enum MenuEntry {
    Item { id: u32, text: String, enabled: bool },
    Sep,
}

#[allow(dead_code)] // ContextMenu: for widgets' right-click menus (clock...)
pub enum Req {
    OpenPopup(PopupSpec),
    ClosePopup,
    /// context menu at the mouse position; the chosen item is returned via `Ev::Click { id }`
    ContextMenu(Vec<MenuEntry>),
    /// DWM thumbnail popup of a window (anchored to the node being hovered); cancelled when the mouse leaves/clicks/the window closes
    Thumbnail(isize),
}

/// Context sent along with an event.
pub struct Cx {
    pub dirty: bool,
    pub requests: Vec<Req>,
    /// this widget's popup is currently open (kind)
    pub popup_open: Option<u32>,
    pub monitor: isize,
    pub scale: f32,
}

impl Cx {
    pub fn redraw(&mut self) {
        self.dirty = true;
    }
    pub fn open_popup(&mut self, spec: PopupSpec) {
        self.requests.push(Req::OpenPopup(spec));
    }
    pub fn close_popup(&mut self) {
        self.requests.push(Req::ClosePopup);
    }
    pub fn thumbnail(&mut self, hwnd: isize) {
        self.requests.push(Req::Thumbnail(hwnd));
    }
    #[allow(dead_code)]
    pub fn context_menu(&mut self, entries: Vec<MenuEntry>) {
        self.requests.push(Req::ContextMenu(entries));
    }
}

pub trait Widget {
    fn view(&self, ui: &mut Ui);
    fn popup_view(&self, _kind: u32, _ui: &mut Ui) {}
    fn event(&mut self, ev: Ev, cx: &mut Cx);
    /// timer period (ms), 0 = none
    fn interval(&self) -> u32 {
        0
    }
}

// ------------------------------------------------------------------ YASB-style labels

#[derive(Clone, Debug)]
pub struct Part {
    pub icon: bool,
    pub class: String,
    pub text: String,
}

/// Splits a label like `build_widget_label`: `<span ...>x</span>` segments become icons, the rest becomes labels.
pub fn parse_label(content: &str, alt: bool) -> Vec<Part> {
    let re = Regex::new(r"(?s)(<span.*?>.*?</span>)").unwrap();
    let class_re = Regex::new(r#"class=(["'])([^"']+?)["']"#).unwrap();
    let tag_re = Regex::new(r"(?s)<span.*?>|</span>").unwrap();
    let mut parts = Vec::new();
    let mut last = 0;
    let push = |s: &str, parts: &mut Vec<Part>| {
        let s = s.trim();
        if s.is_empty() {
            return;
        }
        if s.contains("<span") && s.contains("</span>") {
            let class = class_re.captures(s).map(|c| c[2].to_string()).unwrap_or_else(|| "icon".into());
            let text = tag_re.replace_all(s, "").trim().to_string();
            parts.push(Part { icon: true, class, text });
        } else {
            parts.push(Part { icon: false, class: if alt { "label alt".into() } else { "label".into() }, text: s.to_string() });
        }
    };
    for m in re.find_iter(content) {
        push(&content[last..m.start()], &mut parts);
        push(m.as_str(), &mut parts);
        last = m.end();
    }
    push(&content[last..], &mut parts);
    parts
}

/// The part common to every widget: CSS class, mouse callbacks, primary/secondary labels.
pub struct Base {
    pub class: String,
    pub cb: [String; 3],
    pub label: Vec<Part>,
    pub label_alt: Vec<Part>,
    pub show_alt: bool,
    pub tooltip: Option<String>,
    pub hidden: bool,
}

impl Base {
    pub fn new(name: &str, class: &str, label: &str, label_alt: Option<&str>, cb: [String; 3]) -> Base {
        let _ = name;
        Base {
            class: class.trim().to_string(),
            cb,
            label: parse_label(label, false),
            label_alt: label_alt.map(|l| parse_label(l, true)).unwrap_or_default(),
            show_alt: false,
            tooltip: None,
            hidden: false,
        }
    }

    /// Widget root → `.widget <class>` → `.widget-container` → content.
    pub fn shell(&self, ui: &mut Ui, inner: impl FnOnce(&mut Ui)) -> NodeId {
        let mut root = Node::new("Widget", "");
        root.on_click = Some(BASE);
        root.on_wheel = Some(BASE);
        root.tooltip = self.tooltip.clone();
        root.hidden = self.hidden;
        let class = format!("widget {}", self.class);
        ui.with(root, |ui| {
            ui.row(&class, |ui| {
                ui.row("widget-container", inner);
            });
        })
    }

    /// Builds the QLabels from the primary and secondary labels; `fill` substitutes placeholders, `extra` adds status classes.
    pub fn labels(&self, ui: &mut Ui, fill: &dyn Fn(&str) -> String, extra: &dyn Fn(&Part) -> String) {
        for (parts, is_alt) in [(&self.label, false), (&self.label_alt, true)] {
            for p in parts.iter() {
                let ex = extra(p);
                let class = if ex.is_empty() { p.class.clone() } else { format!("{} {ex}", p.class) };
                let id = ui.label_centered(&class, fill(&p.text));
                ui.node(id).hidden = is_alt != self.show_alt;
            }
        }
    }

    pub fn callback_for(&self, btn: MouseBtn) -> &str {
        match btn {
            MouseBtn::Left => &self.cb[0],
            MouseBtn::Middle => &self.cb[1],
            MouseBtn::Right => &self.cb[2],
        }
    }
}

/// The `progress_bar` of cpu/memory/battery... (`core.utils.progress_bar`).
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(default)]
pub struct ProgressBarConfig {
    pub enabled: bool,
    pub progress_type: String,
    pub size: u32,
    pub thickness: u32,
    pub radius: u32,
    pub color: Value,
    pub background_color: String,
    pub position: String,
    pub animation: bool,
}

impl Default for ProgressBarConfig {
    fn default() -> Self {
        ProgressBarConfig {
            enabled: false,
            progress_type: "circular".into(),
            size: 18,
            thickness: 3,
            radius: 0,
            color: Value::String("#00C800".into()),
            background_color: "#3C3C3C".into(),
            position: "left".into(),
            animation: true,
        }
    }
}

impl ProgressBarConfig {
    /// `.progress-container` → the progress bar, with exactly the size ProgressBar.setFixedSize gives.
    pub fn build(&self, ui: &mut Ui, value: f32) {
        if !self.enabled {
            return;
        }
        let colors: Vec<crate::engine::css::Color> = match &self.color {
            Value::Sequence(v) => v.iter().filter_map(|c| c.as_str()).filter_map(crate::engine::css::parse_color).collect(),
            Value::String(c) => crate::engine::css::parse_color(c).into_iter().collect(),
            _ => Vec::new(),
        };
        let (s, t) = (self.size as f32, self.thickness as f32);
        let (w, h) = match self.progress_type.as_str() {
            "linear_horizontal" => (s, t),
            "linear_vertical" => (t, s),
            _ => (s, s),
        };
        ui.row("progress-container", |ui| {
            let mut n = Node::new("QWidget", "progress-bar");
            n.fixed = (Some(w), Some(h));
            n.item_align = Some((Align::Center, Align::Center));
            n.content = Content::Progress(Progress {
                value,
                kind: self.progress_type.clone(),
                thickness: t,
                radius: self.radius as f32,
                colors,
                background: crate::engine::css::parse_color(&self.background_color).unwrap_or(crate::engine::css::Color::rgba8(60, 60, 60, 255)),
                animate: self.animation,
            });
            ui.add(n);
        });
    }
}

/// Substitutes `{key}` placeholders from the table (unknown ones are left as is).
pub fn fill_placeholders(t: &str, vals: &[(&str, String)]) -> String {
    let mut out = t.to_string();
    for (k, v) in vals {
        if out.contains(k) {
            out = out.replace(k, v);
        }
    }
    out
}

/// Python-style number: `round(x)` when hide_decimal, otherwise 1 decimal digit ("12.3").
pub fn py_num(v: f32, hide_decimal: bool) -> String {
    if hide_decimal {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

#[derive(serde::Deserialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct Thresholds {
    pub low: f32,
    pub medium: f32,
    pub high: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { low: 25.0, medium: 50.0, high: 90.0 }
    }
}

impl Thresholds {
    pub fn class(&self, v: f32) -> &'static str {
        if v <= self.low {
            "status-low"
        } else if v <= self.medium {
            "status-medium"
        } else if v <= self.high {
            "status-high"
        } else {
            "status-critical"
        }
    }
}

/// Splits a callback string `exec "a b" c` into (name, arguments).
pub fn split_callback(s: &str) -> (String, Vec<String>) {
    if !s.contains(' ') {
        return (s.to_string(), Vec::new());
    }
    let re = Regex::new(r#"".+?"|[^ ]+"#).unwrap();
    let mut it = re.find_iter(s).map(|m| m.as_str().trim_matches('"').to_string());
    let name = it.next().unwrap_or_default();
    (name, it.collect())
}

/// BaseWidget's shared callback. Returns true if handled.
pub fn base_callback(name: &str, args: &[String]) -> bool {
    match name {
        "default" | "do_nothing" | "" => true,
        "exec" => {
            if let Some(cmd) = args.first()
                && !crate::engine::sys::shell::system_function(cmd) {
                    let mut line = cmd.clone();
                    for a in &args[1..] {
                        line.push(' ');
                        if a.contains(' ') {
                            line.push_str(&format!("\"{a}\""));
                        } else {
                            line.push_str(a);
                        }
                    }
                    crate::engine::sys::shell::run_shell(&line, false);
                }
            true
        }
        _ => false,
    }
}

/// Creates a widget according to `type` in the config.
pub fn create(name: &str, type_name: &str, options: &Value) -> Option<Box<dyn Widget>> {
    let short = type_name.rsplit('.').next().unwrap_or(type_name);
    let w: Box<dyn Widget> = match short {
        "HomeWidget" => Box::new(home::Home::new(name, options)),
        "ClockWidget" => Box::new(clock::Clock::new(name, options)),
        "VolumeWidget" => Box::new(volume::Volume::new(name, options)),
        "NotificationsWidget" => Box::new(notifications::Notifications::new(name, options)),
        "PowerMenuWidget" => Box::new(power_menu::PowerMenu::new(name, options)),
        "ActiveWindowWidget" => Box::new(active_window::ActiveWindow::new(name, options)),
        "CpuWidget" => Box::new(cpu::Cpu::new(name, options)),
        "MemoryWidget" => Box::new(memory::Memory::new(name, options)),
        "WifiWidget" => Box::new(wifi::Wifi::new(name, options)),
        "SystrayWidget" => Box::new(systray::Systray::new(name, options)),
        "TaskbarWidget" => Box::new(taskbar::Taskbar::new(name, options)),
        "UnikeyWidget" => Box::new(unikey::Unikey::new(name, options)),
        "AuroraWidget" => Box::new(aurora::Aurora::new(name, options)),
        "WindowsButtonWidget" => Box::new(windows_button::WindowsButton::new(name, options)),
        "SearchWidget" => Box::new(search::Search::new(name, options)),
        "BatteryWidget" => Box::new(battery::Battery::new(name, options)),
        _ => {
            crate::engine::log::warn(&format!("Widget '{name}': kiểu '{type_name}' chưa được hỗ trợ trong bản Rust, bỏ qua."));
            return None;
        }
    };
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_widget_types_are_registered() {
        for t in ["yasb.windows_button.WindowsButtonWidget", "yasb.search.SearchWidget", "yasb.battery.BatteryWidget"] {
            assert!(create("w", t, &Value::Null).is_some(), "{t}");
        }
        assert!(create("w", "yasb.nope.NopeWidget", &Value::Null).is_none());
    }

    #[test]
    fn label_split() {
        let p = parse_label("{%H:%M}<span>{alarm}</span>", false);
        assert_eq!(p.len(), 2);
        assert!(!p[0].icon && p[0].class == "label");
        assert!(p[1].icon && p[1].class == "icon" && p[1].text == "{alarm}");
        let p = parse_label("<span class=\"x y\">A</span> text", true);
        assert_eq!(p[0].class, "x y");
        assert_eq!(p[1].class, "label alt");
        let (n, a) = split_callback("exec \"C:\\Program Files\\x.exe\" --flag");
        assert_eq!(n, "exec");
        assert_eq!(a, vec!["C:\\Program Files\\x.exe", "--flag"]);
    }
}
