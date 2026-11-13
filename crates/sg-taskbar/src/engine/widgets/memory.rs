//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! `yasb.memory.MemoryWidget`: `{virtual_mem_percent}`, `{virtual_mem_used}`/`{virtual_mem_total}`...
//! (`naturalsize(binary, gnu)` format such as "7.2G"), progress bar and threshold classes.

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks};
use crate::engine::sys::stats::{self, naturalsize};

#[derive(Deserialize, Clone)]
#[serde(default)]
struct MemoryConfig {
    label: String,
    label_alt: String,
    class_name: String,
    update_interval: u32,
    hide_decimal: bool,
    memory_thresholds: Thresholds,
    progress_bar: ProgressBarConfig,
    callbacks: Callbacks,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig {
            label: "\u{efc5} {virtual_mem_free}".into(),
            label_alt: "\u{efc5} {virtual_mem_percent}%".into(),
            class_name: String::new(),
            update_interval: 5000,
            hide_decimal: false,
            memory_thresholds: Thresholds { low: 25.0, medium: 50.0, high: 90.0 },
            progress_bar: ProgressBarConfig::default(),
            callbacks: Callbacks::default(),
        }
    }
}

pub struct Memory {
    base: Base,
    cfg: MemoryConfig,
    m: stats::Mem,
}

impl Memory {
    pub fn new(name: &str, opts: &Value) -> Memory {
        let cfg: MemoryConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_label", "do_nothing", "do_nothing");
        let base = Base::new(name, &format!("memory-widget {}", cfg.class_name), &cfg.label, Some(&cfg.label_alt), cb);
        Memory { base, cfg, m: stats::Mem::default() }
    }
}

impl Widget for Memory {
    fn view(&self, ui: &mut Ui) {
        let hd = self.cfg.hide_decimal;
        let d = if hd { 0 } else { 1 };
        let m = &self.m;
        let vals = [
            ("{virtual_mem_free}", naturalsize(m.avail, d)),
            ("{virtual_mem_percent}", py_num(m.percent, hd)),
            ("{virtual_mem_total}", naturalsize(m.total, d)),
            ("{virtual_mem_avail}", naturalsize(m.avail, d)),
            ("{virtual_mem_used}", naturalsize(m.used, d)),
            ("{virtual_mem_outof}", format!("{} / {}", naturalsize(m.used, d), naturalsize(m.total, d))),
            ("{swap_mem_free}", naturalsize(m.swap_total - m.swap_used, d)),
            ("{swap_mem_percent}", py_num(m.swap_percent, hd)),
            ("{swap_mem_total}", naturalsize(m.swap_total, d)),
            ("{swap_mem_used}", naturalsize(m.swap_used, d)),
        ];
        let status = self.cfg.memory_thresholds.class(m.percent);
        self.base.shell(ui, |ui| {
            let left = self.cfg.progress_bar.position != "right";
            if left {
                self.cfg.progress_bar.build(ui, m.percent);
            }
            self.base.labels(ui, &|t| fill_placeholders(t, &vals), &|p| if p.icon { String::new() } else { status.to_string() });
            if !left {
                self.cfg.progress_bar.build(ui, m.percent);
            }
        });
    }

    fn interval(&self) -> u32 {
        self.cfg.update_interval.max(250)
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init | Ev::Timer => {
                self.m = stats::memory();
                let m = &self.m;
                self.base.tooltip = Some(format!(
                    "RAM {:.0}%  ·  đang dùng {} / {}  ·  còn trống {}",
                    m.percent,
                    naturalsize(m.used, 1),
                    naturalsize(m.total, 1),
                    naturalsize(m.avail, 1)
                ));
                cx.redraw();
            }
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if base_callback(&name, &args) {
                    return;
                }
                if name == "toggle_label" {
                    self.base.show_alt = !self.base.show_alt;
                    cx.redraw();
                }
            }
            _ => {}
        }
    }
}
