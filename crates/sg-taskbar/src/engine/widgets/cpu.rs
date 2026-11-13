//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! `yasb.cpu.CpuWidget`: CPU % (`{info[percent][total]}`), core count, text bar chart, progress bar,
//! class `status-low|medium|high|critical` by threshold like the Python version.

use std::collections::VecDeque;

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks};
use crate::engine::sys::stats;

#[derive(Deserialize, Clone)]
#[serde(default)]
struct CpuConfig {
    label: String,
    label_alt: String,
    class_name: String,
    update_interval: u32,
    histogram_icons: Vec<String>,
    histogram_num_columns: usize,
    hide_decimal: bool,
    cpu_thresholds: Thresholds,
    progress_bar: ProgressBarConfig,
    callbacks: Callbacks,
}

impl Default for CpuConfig {
    fn default() -> Self {
        CpuConfig {
            label: "\u{f4bc} {info[percent][total]}%".into(),
            label_alt: "\u{f4bc} {info[histograms][cpu_percent]}".into(),
            class_name: String::new(),
            update_interval: 1000,
            histogram_icons: ["\u{2581}", "\u{2581}", "\u{2582}", "\u{2583}", "\u{2584}", "\u{2585}", "\u{2586}", "\u{2587}", "\u{2588}"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            histogram_num_columns: 10,
            hide_decimal: false,
            cpu_thresholds: Thresholds::default(),
            progress_bar: ProgressBarConfig::default(),
            callbacks: Callbacks::default(),
        }
    }
}

pub struct Cpu {
    base: Base,
    cfg: CpuConfig,
    percent: f32,
    history: VecDeque<f32>,
    cores: (u32, u32),
}

impl Cpu {
    pub fn new(name: &str, opts: &Value) -> Cpu {
        let cfg: CpuConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_label", "do_nothing", "do_nothing");
        let base = Base::new(name, &format!("cpu-widget {}", cfg.class_name), &cfg.label, Some(&cfg.label_alt), cb);
        let n = cfg.histogram_num_columns.max(1);
        Cpu { base, cfg, percent: 0.0, history: VecDeque::from(vec![0.0; n]), cores: stats::cpu_cores() }
    }

    fn histogram(&self) -> String {
        let icons = &self.cfg.histogram_icons;
        if icons.is_empty() {
            return String::new();
        }
        self.history
            .iter()
            .map(|v| {
                let i = ((v / 100.0) * (icons.len() - 1) as f32).round() as usize;
                icons[i.min(icons.len() - 1)].as_str()
            })
            .collect()
    }

    fn sample(&mut self) {
        self.percent = stats::cpu_percent();
        self.history.push_back(self.percent);
        while self.history.len() > self.cfg.histogram_num_columns.max(1) {
            self.history.pop_front();
        }
        self.base.tooltip = Some(format!("CPU {}%  ·  {} nhân / {} luồng", py_num(self.percent, true), self.cores.0, self.cores.1));
    }
}

impl Widget for Cpu {
    fn view(&self, ui: &mut Ui) {
        let hd = self.cfg.hide_decimal;
        let vals = [
            ("{info[percent][total]}", py_num(self.percent, hd)),
            ("{info[cores][physical]}", self.cores.0.to_string()),
            ("{info[cores][total]}", self.cores.1.to_string()),
            ("{info[histograms][cpu_percent]}", self.histogram()),
        ];
        let status = self.cfg.cpu_thresholds.class(self.percent);
        self.base.shell(ui, |ui| {
            let left = self.cfg.progress_bar.position != "right";
            if left {
                self.cfg.progress_bar.build(ui, self.percent);
            }
            self.base.labels(ui, &|t| fill_placeholders(t, &vals), &|p| if p.icon { String::new() } else { status.to_string() });
            if !left {
                self.cfg.progress_bar.build(ui, self.percent);
            }
        });
    }

    fn interval(&self) -> u32 {
        self.cfg.update_interval.max(250)
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init | Ev::Timer => {
                self.sample();
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
