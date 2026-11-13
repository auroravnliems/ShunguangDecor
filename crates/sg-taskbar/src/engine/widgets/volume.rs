//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! `yasb.volume.VolumeWidget`: volume icon/level, scroll the wheel to adjust, right-click to mute,
//! the `.audio-menu` menu contains the device list, a master slider, and per-application volume.

use std::rc::Rc;

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks, PopupOptions};
use crate::engine::sys::audio;

#[derive(Deserialize, Clone)]
#[serde(default)]
struct AppIcons {
    toggle_down: String,
    toggle_up: String,
}

impl Default for AppIcons {
    fn default() -> Self {
        AppIcons { toggle_down: "\u{f078}".into(), toggle_up: "\u{f077}".into() }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct AudioMenu {
    blur: bool,
    round_corners: bool,
    round_corners_type: String,
    border_color: String,
    alignment: String,
    direction: String,
    offset_top: i32,
    offset_left: i32,
    show_apps: bool,
    show_app_labels: bool,
    show_app_icons: bool,
    show_apps_expanded: bool,
    app_icons: AppIcons,
}

impl Default for AudioMenu {
    fn default() -> Self {
        AudioMenu {
            blur: true,
            round_corners: true,
            round_corners_type: "normal".into(),
            border_color: "System".into(),
            alignment: "right".into(),
            direction: "down".into(),
            offset_top: 6,
            offset_left: 0,
            show_apps: false,
            show_app_labels: false,
            show_app_icons: true,
            show_apps_expanded: false,
            app_icons: AppIcons::default(),
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum Icons {
    List(Vec<String>),
    Map(std::collections::BTreeMap<String, String>),
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct VolumeConfig {
    label: String,
    label_alt: String,
    class_name: String,
    mute_text: String,
    tooltip: bool,
    scroll_step: i32,
    invert_wheel: bool,
    icons: Icons,
    audio_menu: AudioMenu,
    callbacks: Callbacks,
}

impl Default for VolumeConfig {
    fn default() -> Self {
        let mut m = std::collections::BTreeMap::new();
        m.insert("muted".into(), "\u{eee8}".into());
        m.insert("10".into(), "\u{f026}".into());
        m.insert("30".into(), "\u{f027}".into());
        m.insert("60".into(), "\u{f027}".into());
        m.insert("100".into(), "\u{f028}".into());
        VolumeConfig {
            label: "{icon} {level}".into(),
            label_alt: "{icon} {level}".into(),
            class_name: String::new(),
            mute_text: "mute".into(),
            tooltip: true,
            scroll_step: 2,
            invert_wheel: false,
            icons: Icons::Map(m),
            audio_menu: AudioMenu::default(),
            callbacks: Callbacks::default(),
        }
    }
}

const ID_SLIDER: u32 = 1;
const ID_TOGGLE_APPS: u32 = 2;
const ID_DEVICE: u32 = 100;
const ID_APP_SLIDER: u32 = 1000;
const ID_APP_MUTE: u32 = 2000;

struct AppRow {
    session: audio::Session,
    name: String,
    icon: Option<Rc<Pixmap>>,
    volume: i32,
    muted: bool,
}

pub struct Volume {
    base: Base,
    cfg: VolumeConfig,
    thresholds: Vec<i32>,
    level: Option<(i32, bool)>,
    devices: Vec<(String, String)>,
    current_device: String,
    apps: Vec<AppRow>,
    apps_expanded: bool,
}

impl Volume {
    pub fn new(name: &str, opts: &Value) -> Volume {
        let cfg: VolumeConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_volume_menu", "do_nothing", "toggle_mute");
        let base = Base::new(name, &format!("volume-widget {}", cfg.class_name), &cfg.label, Some(&cfg.label_alt), cb);
        let mut thresholds: Vec<i32> = match &cfg.icons {
            Icons::Map(m) => m.keys().filter_map(|k| k.parse().ok()).collect(),
            Icons::List(_) => Vec::new(),
        };
        thresholds.sort();
        Volume { base, cfg, thresholds, level: None, devices: Vec::new(), current_device: String::new(), apps: Vec::new(), apps_expanded: false }
    }

    fn icon(&self) -> String {
        let Some((v, muted)) = self.level else {
            return match &self.cfg.icons {
                Icons::Map(m) => m.values().next().cloned().unwrap_or_default(),
                Icons::List(l) => l.first().cloned().unwrap_or_default(),
            };
        };
        match &self.cfg.icons {
            Icons::Map(m) => {
                if muted
                    && let Some(i) = m.get("muted") {
                        return i.clone();
                    }
                for t in &self.thresholds {
                    if v <= *t {
                        return m.get(&t.to_string()).cloned().unwrap_or_default();
                    }
                }
                self.thresholds.last().and_then(|t| m.get(&t.to_string()).cloned()).or_else(|| m.get("muted").cloned()).unwrap_or_default()
            }
            Icons::List(l) => {
                let idx = if muted {
                    0
                } else if v == 0 {
                    1
                } else if v <= 33 {
                    2
                } else if v <= 66 {
                    3
                } else {
                    4
                };
                l.get(idx).cloned().unwrap_or_default()
            }
        }
    }

    fn level_text(&self) -> String {
        match self.level {
            None => "No Device".into(),
            Some((_, true)) => self.cfg.mute_text.clone(),
            Some((v, false)) => format!("{v}%"),
        }
    }

    fn refresh(&mut self) {
        self.level = audio::get();
        self.base.tooltip = if !self.cfg.tooltip {
            None
        } else {
            Some(match self.level {
                None => "No audio device connected.".into(),
                Some((v, m)) => format!("Volume {v}% {}", if m { "(Muted)" } else { "" }),
            })
        };
    }

    fn load_menu_data(&mut self, scale: f32) {
        let (devs, cur) = audio::devices();
        self.devices = devs;
        self.current_device = cur;
        self.apps.clear();
        if self.cfg.audio_menu.show_apps {
            let px = (16.0 * scale).round() as u32;
            for s in audio::sessions() {
                let (volume, muted) = s.get();
                let exe = crate::engine::sys::process::exe_path(s.pid).unwrap_or_default();
                let icon = crate::engine::sys::image::exe_icon(&exe, px);
                let name = s.name.clone();
                self.apps.push(AppRow { session: s, name, icon, volume, muted });
            }
        }
        self.apps_expanded = self.cfg.audio_menu.show_apps_expanded;
    }

    fn spec(&self) -> PopupSpec {
        let c = &self.cfg.audio_menu;
        PopupSpec {
            kind: 1,
            class: "audio-menu".into(),
            opts: PopupOptions {
                blur: c.blur,
                round_corners: c.round_corners,
                round_corners_type: c.round_corners_type.clone(),
                border_color: c.border_color.clone(),
                alignment: c.alignment.clone(),
                direction: c.direction.clone(),
                offset_top: c.offset_top,
                offset_left: c.offset_left,
            },
            margins: Edges { top: 10.0, right: 10.0, bottom: 10.0, left: 10.0 },
            dir: Dir::Col,
        }
    }

    fn step(&mut self, up: bool) {
        let Some((v, muted)) = audio::get() else { return };
        let step = self.cfg.scroll_step;
        if up {
            let nv = (v + step).min(100);
            audio::set_volume(nv);
            if muted && nv > 0 {
                audio::set_mute(false);
            }
        } else {
            let nv = (v - step).max(0);
            audio::set_volume(nv);
            if nv == 0 {
                audio::set_mute(true);
            }
        }
        self.refresh();
    }
}

impl Widget for Volume {
    fn view(&self, ui: &mut Ui) {
        let icon = self.icon();
        let level = self.level_text();
        let muted = self.level.is_some_and(|l| l.1);
        let no_dev = self.level.is_none();
        self.base.shell(ui, |ui| {
            self.base.labels(
                ui,
                &|t| t.replace("{icon}", &icon).replace("{level}", &level),
                &|_| {
                    let mut c = Vec::new();
                    if no_dev {
                        c.push("no-device");
                    }
                    if muted {
                        c.push("muted");
                    }
                    c.join(" ")
                },
            )
        });
    }

    fn popup_view(&self, _kind: u32, ui: &mut Ui) {
        let am = &self.cfg.audio_menu;
        let mut container = Node::new("QWidget", "audio-container");
        container.dir = Dir::Col;
        ui.with(container, |ui| {
            if self.devices.len() > 1 {
                for (i, (id, name)) in self.devices.iter().enumerate() {
                    let cls = if *id == self.current_device { "device selected" } else { "device" };
                    let b = ui.button(cls, name.clone(), ID_DEVICE + i as u32);
                    ui.node(b).halign = Align::Start;
                }
            }
        });
        ui.col("system-volume-container", |ui| {
            ui.layout(Dir::Row, |ui| {
                let v = self.level.map(|l| l.0).unwrap_or(0);
                ui.slider("volume-slider", v, 100, ID_SLIDER);
                if am.show_apps && !self.apps.is_empty() {
                    let icon = if self.apps_expanded { &am.app_icons.toggle_up } else { &am.app_icons.toggle_down };
                    let cls = if self.apps_expanded { "toggle-apps expanded" } else { "toggle-apps" };
                    let b = ui.button(cls, icon.clone(), ID_TOGGLE_APPS);
                    if self.cfg.tooltip {
                        ui.node(b).tooltip = Some("Expand application volumes".into());
                    }
                    ui.node(b).item_align = Some((Align::Center, Align::Center));
                }
            });
        });
        if am.show_apps && !self.apps.is_empty() {
            let mut apps = Node::new("QFrame", "apps-container");
            apps.dir = Dir::Col;
            apps.hidden = !self.apps_expanded;
            ui.with(apps, |ui| {
                for (i, a) in self.apps.iter().enumerate() {
                    ui.col("app-volume", |ui| {
                        if am.show_app_labels {
                            ui.label("app-label", a.name.clone());
                        }
                        ui.layout(Dir::Row, |ui| {
                            if am.show_app_icons {
                                let mut f = Node::new("QFrame", "app-icon-container");
                                f.on_press = Some(ID_APP_MUTE + i as u32);
                                if self.cfg.tooltip {
                                    f.tooltip = Some(a.name.clone());
                                    f.tooltip_delay = 800;
                                }
                                ui.with(f, |ui| {
                                    if let Some(p) = &a.icon {
                                        let pix = if a.muted { p.grayscale() } else { p.clone() };
                                        let id = ui.image("app-icon", Image { pix, w: 16.0, h: 16.0, circle: false });
                                        ui.node(id).item_align = Some((Align::Center, Align::Center));
                                    }
                                });
                            }
                            let s = ui.slider("app-slider", a.volume, 100, ID_APP_SLIDER + i as u32);
                            if a.muted {
                                ui.node(s).states |= crate::engine::css::state::DISABLED;
                            }
                        });
                    });
                }
            });
        }
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init | Ev::Sys(SysEv::AudioChanged) => {
                self.refresh();
                cx.redraw();
            }
            Ev::Sys(SysEv::AudioDevicesChanged) => {
                audio::refresh_device();
                self.refresh();
                if cx.popup_open.is_some() {
                    let (d, c) = audio::devices();
                    self.devices = d;
                    self.current_device = c;
                }
                cx.redraw();
            }
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if base_callback(&name, &args) {
                    return;
                }
                match name.as_str() {
                    "toggle_volume_menu" => {
                        if self.level.is_none() {
                            return;
                        }
                        self.load_menu_data(cx.scale);
                        cx.open_popup(self.spec());
                    }
                    "toggle_mute" => {
                        if let Some((_, m)) = audio::get() {
                            audio::set_mute(!m);
                        }
                        self.refresh();
                        cx.redraw();
                    }
                    "toggle_label" => {
                        self.base.show_alt = !self.base.show_alt;
                        cx.redraw();
                    }
                    "update_label" => cx.redraw(),
                    other => crate::engine::log::warn(&format!("Volume: callback không rõ '{other}'")),
                }
            }
            Ev::Wheel { id: BASE, delta } => {
                let d = if self.cfg.invert_wheel { -delta } else { delta };
                if d != 0 {
                    self.step(d > 0);
                    cx.redraw();
                }
            }
            Ev::Slider { id: ID_SLIDER, value, .. } => {
                audio::set_volume(value);
                if value > 0 && self.level.is_some_and(|l| l.1) {
                    audio::set_mute(false);
                }
                self.refresh();
                cx.redraw();
            }
            Ev::Slider { id, value, .. } if (ID_APP_SLIDER..ID_APP_MUTE).contains(&id) => {
                if let Some(a) = self.apps.get_mut((id - ID_APP_SLIDER) as usize)
                    && !a.muted {
                        a.session.set(value);
                        a.volume = value;
                    }
                cx.redraw();
            }
            Ev::Press { id } if id >= ID_APP_MUTE => {
                if let Some(a) = self.apps.get_mut((id - ID_APP_MUTE) as usize) {
                    a.muted = !a.muted;
                    a.session.set_mute(a.muted);
                }
                cx.redraw();
            }
            Ev::Click { id: ID_TOGGLE_APPS, .. } => {
                self.apps_expanded = !self.apps_expanded;
                cx.redraw();
            }
            Ev::Click { id, .. } if (ID_DEVICE..ID_APP_SLIDER).contains(&id) => {
                if let Some((dev, _)) = self.devices.get((id - ID_DEVICE) as usize).cloned() {
                    audio::set_default_device(&dev);
                    self.current_device = dev;
                    self.refresh();
                }
                cx.redraw();
            }
            Ev::PopupClosed => {
                self.apps.clear();
            }
            _ => {}
        }
    }
}
