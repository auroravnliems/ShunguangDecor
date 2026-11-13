//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! `AuroraWidget` (specific to TachyonTaskbar): an emoji-image mascot whose expression changes with the time of day and
//! CPU load; clicking it makes it "bounce" (class `bounce` + transition `-tachyon-image-scale`) and switches to a random expression;
//! right-click opens the `.aurora-card` card (a time-of-day class `dawn|day|sunset|night` lets the stylesheet set the banner background).

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use chrono::Timelike;
use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, PopupOptions};
use crate::engine::sys::stats;

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct Moods {
    dawn: Vec<String>,
    day: Vec<String>,
    sunset: Vec<String>,
    night: Vec<String>,
    busy: Vec<String>,
    click: Vec<String>,
    card: Vec<String>,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct AuroraConfig {
    class_name: String,
    size: u32,
    card_image_size: u32,
    /// image folder, relative to the config folder
    folder: String,
    moods: Moods,
    busy_cpu: f32,
    greetings: HashMap<String, Vec<String>>,
    name: String,
    /// number of background banners for the card: each time it opens, a random class `b01`..`bNN` is chosen
    banners: u32,
    popup: PopupOptions,
}

impl Default for AuroraConfig {
    fn default() -> Self {
        AuroraConfig {
            class_name: String::new(),
            size: 26,
            card_image_size: 110,
            folder: "assets/emoji".into(),
            moods: Moods::default(),
            busy_cpu: 85.0,
            greetings: HashMap::new(),
            name: "Aurora".into(),
            banners: 0,
            popup: PopupOptions::default(),
        }
    }
}

const ID_CARD_CLOSE: u32 = 1;
const POPUP: u32 = 1;

pub struct Aurora {
    base: Base,
    cfg: AuroraConfig,
    current: String,
    card_img: String,
    greeting: String,
    cache: HashMap<(String, u32), Option<Rc<Pixmap>>>,
    bounce_until: Option<Instant>,
    react_until: Option<Instant>,
    cpu: f32,
    seed: u64,
    scale: f32,
    period: &'static str,
    ticks: u32,
}

fn period_of(h: u32) -> &'static str {
    match h {
        5..=10 => "dawn",
        11..=16 => "day",
        17..=19 => "sunset",
        _ => "night",
    }
}

impl Aurora {
    pub fn new(name: &str, opts: &Value) -> Aurora {
        let cfg: AuroraConfig = options(name, opts);
        let base = Base::new(name, &format!("aurora-widget {}", cfg.class_name), "", None, ["react".into(), "do_nothing".into(), "toggle_card".into()]);
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
        Aurora {
            base,
            cfg,
            current: String::new(),
            card_img: String::new(),
            greeting: String::new(),
            cache: HashMap::new(),
            bounce_until: None,
            react_until: None,
            cpu: 0.0,
            seed,
            scale: 1.0,
            period: "day",
            ticks: 0,
        }
    }

    fn rand(&mut self, n: usize) -> usize {
        // xorshift: good enough for picking a random image
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        if n == 0 {
            0
        } else {
            (self.seed % n as u64) as usize
        }
    }

    fn pick(&mut self, list: &[String]) -> Option<String> {
        if list.is_empty() {
            return None;
        }
        let i = self.rand(list.len());
        Some(list[i].clone())
    }

    fn mood_list(&self) -> Vec<String> {
        let m = &self.cfg.moods;
        if self.cpu >= self.cfg.busy_cpu && !m.busy.is_empty() {
            return m.busy.clone();
        }
        let l = match self.period {
            "dawn" => &m.dawn,
            "sunset" => &m.sunset,
            "night" => &m.night,
            _ => &m.day,
        };
        if l.is_empty() {
            m.day.clone()
        } else {
            l.clone()
        }
    }

    fn path(&self, file: &str) -> String {
        let p = std::path::Path::new(file);
        if p.is_absolute() {
            return file.to_string();
        }
        crate::engine::config::active_dir().join(&self.cfg.folder).join(file).display().to_string()
    }

    fn image(&mut self, file: &str, size: u32) -> Option<Rc<Pixmap>> {
        let px = (size as f32 * self.scale).round() as u32;
        let key = (file.to_string(), px);
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        let p = crate::engine::sys::image::load_file_fit(&self.path(file), px, px, crate::engine::sys::image::Fit::Contain);
        if self.cache.len() > 40 {
            self.cache.clear();
        }
        self.cache.insert(key, p.clone());
        p
    }

    fn greet(&mut self) {
        let list = self.cfg.greetings.get(self.period).cloned().unwrap_or_default();
        self.greeting = self.pick(&list).unwrap_or_else(|| {
            match self.period {
                "dawn" => "Chào buổi sáng! Hôm nay mình cùng làm thật nhiều việc nhé.",
                "day" => "Nghỉ tay uống miếng nước rồi làm tiếp nào~",
                "sunset" => "Hoàng hôn đẹp ghê, sắp xong việc chưa?",
                _ => "Khuya rồi đó, nhớ đi ngủ sớm nha.",
            }
            .to_string()
        });
    }

    fn update_mood(&mut self, force: bool) -> bool {
        let h = chrono::Local::now().hour();
        let p = period_of(h);
        let period_changed = p != self.period;
        self.period = p;
        if self.react_until.is_some_and(|t| Instant::now() < t) && !force {
            return false;
        }
        self.react_until = None;
        let list = self.mood_list();
        if (force || period_changed || !list.contains(&self.current) || self.current.is_empty())
            && let Some(f) = self.pick(&list) {
                self.current = f;
                if period_changed {
                    self.greet();
                }
                return true;
            }
        false
    }
}

impl Widget for Aurora {
    fn view(&self, ui: &mut Ui) {
        let size = self.cfg.size as f32;
        let bounce = self.bounce_until.is_some_and(|t| Instant::now() < t);
        let busy = self.cpu >= self.cfg.busy_cpu;
        let px = (self.cfg.size as f32 * self.scale).round() as u32;
        self.base.shell(ui, |ui| {
            let mut class = format!("avatar {}", self.period);
            if bounce {
                class.push_str(" bounce");
            }
            if busy {
                class.push_str(" busy");
            }
            let mut n = Node::new("QLabel", &class);
            n.halign = Align::Center;
            n.fixed = (Some(size), Some(size));
            if let Some(Some(p)) = self.cache.get(&(self.current.clone(), px)) {
                n.content = Content::Image(Image { pix: p.clone(), w: size, h: size, circle: false });
            }
            ui.add(n);
        });
    }

    fn popup_view(&self, _kind: u32, ui: &mut Ui) {
        let s = self.cfg.card_image_size as f32;
        let px = (self.cfg.card_image_size as f32 * self.scale).round() as u32;
        ui.row(&format!("aurora-card-body {}", self.period), |ui| {
            let mut n = Node::new("QLabel", "aurora-card-image");
            n.fixed = (Some(s), Some(s));
            n.halign = Align::Center;
            if let Some(Some(p)) = self.cache.get(&(self.card_img.clone(), px)) {
                n.content = Content::Image(Image { pix: p.clone(), w: s, h: s, circle: false });
            }
            ui.add(n);
            ui.col("aurora-card-text", |ui| {
                ui.label("aurora-card-title", self.cfg.name.clone());
                let now = chrono::Local::now();
                let wd = ["Chủ nhật", "Thứ hai", "Thứ ba", "Thứ tư", "Thứ năm", "Thứ sáu", "Thứ bảy"];
                use chrono::Datelike;
                ui.label("aurora-card-date", format!("{}, {:02}/{:02}/{}  ·  {:02}:{:02}", wd[now.weekday().num_days_from_sunday() as usize], now.day(), now.month(), now.year(), now.hour(), now.minute()));
                let id = ui.label("aurora-card-greeting", self.greeting.clone());
                ui.node(id).word_wrap = true;
                let m = stats::memory();
                ui.label("aurora-card-stats", format!("CPU {:.0}%   ·   RAM {:.0}%  ({} / {})", self.cpu, m.percent, stats::naturalsize(m.used, 1), stats::naturalsize(m.total, 1)));
                ui.stretch();
                let b = ui.button("aurora-card-button", "Hẹn gặp lại~", ID_CARD_CLOSE);
                ui.node(b).item_align = Some((Align::End, Align::Center));
            });
        });
    }

    fn interval(&self) -> u32 {
        250
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init => {
                self.scale = cx.scale;
                self.cpu = stats::cpu_percent();
                self.update_mood(true);
                self.greet();
                let cur = self.current.clone();
                let _ = self.image(&cur, self.cfg.size);
                self.base.tooltip = Some(self.greeting.clone());
                cx.redraw();
            }
            Ev::Timer => {
                self.ticks += 1;
                let mut dirty = false;
                if self.bounce_until.is_some_and(|t| Instant::now() >= t) {
                    self.bounce_until = None;
                    dirty = true;
                }
                // every 5 seconds, re-check CPU/time
                if self.ticks.is_multiple_of(20) {
                    self.cpu = stats::cpu_percent();
                    if self.update_mood(false) {
                        dirty = true;
                    }
                    self.base.tooltip = Some(self.greeting.clone());
                }
                if dirty {
                    let cur = self.current.clone();
                    let _ = self.image(&cur, self.cfg.size);
                    cx.redraw();
                }
            }
            Ev::Click { id: ID_CARD_CLOSE, .. } => cx.close_popup(),
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if base_callback(&name, &args) {
                    return;
                }
                match name.as_str() {
                    "react" => {
                        let list = if self.cfg.moods.click.is_empty() { self.mood_list() } else { self.cfg.moods.click.clone() };
                        // do not repeat the image currently shown
                        for _ in 0..4 {
                            if let Some(f) = self.pick(&list)
                                && (f != self.current || list.len() < 2) {
                                    self.current = f;
                                    break;
                                }
                        }
                        let cur = self.current.clone();
                        let _ = self.image(&cur, self.cfg.size);
                        self.bounce_until = Some(Instant::now() + std::time::Duration::from_millis(220));
                        self.react_until = Some(Instant::now() + std::time::Duration::from_secs(8));
                        self.greet();
                        self.base.tooltip = Some(self.greeting.clone());
                        cx.redraw();
                    }
                    "toggle_card" => {
                        if cx.popup_open == Some(POPUP) {
                            cx.close_popup();
                            return;
                        }
                        let list = if self.cfg.moods.card.is_empty() { self.mood_list() } else { self.cfg.moods.card.clone() };
                        self.card_img = self.pick(&list).unwrap_or_else(|| self.current.clone());
                        let ci = self.card_img.clone();
                        let _ = self.image(&ci, self.cfg.card_image_size);
                        self.greet();
                        let banner = if self.cfg.banners > 0 { format!(" b{:02}", self.rand(self.cfg.banners as usize) + 1) } else { String::new() };
                        cx.open_popup(PopupSpec {
                            kind: POPUP,
                            class: format!("aurora-card {}{banner}", self.period),
                            opts: self.cfg.popup.clone(),
                            margins: Edges::default(),
                            dir: Dir::Col,
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
