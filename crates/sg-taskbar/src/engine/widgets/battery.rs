//! `yasb.battery.BatteryWidget` (a ShunguangDecor extension): `{percent}%` + an icon by level/charging state,
//! read via `GetSystemPowerStatus`. A machine without a battery (desktop PC) → the widget is hidden.

use serde::Deserialize;
use serde_yaml::Value;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use super::*;
use crate::engine::config::{options, Callbacks};

#[derive(Deserialize, Clone)]
#[serde(default)]
struct Config {
    label: String,
    class_name: String,
    update_interval: u32,
    callbacks: Callbacks,
}

impl Default for Config {
    fn default() -> Self {
        Config { label: "<span>{icon}</span> {percent}%".into(), class_name: String::new(), update_interval: 15_000, callbacks: Callbacks::default() }
    }
}

/// Reads the battery: (percentage, plugged in and charging). `None` if there is no battery or Windows does not know the level.
pub fn battery_reading(ac_line: u8, flag: u8, percent: u8) -> Option<(u8, bool)> {
    // BatteryFlag: 128 = no battery, 255 = unknown; BatteryLifePercent 255 = unknown
    if flag & 128 != 0 || flag == 255 || percent > 100 {
        return None;
    }
    Some((percent, flag & 8 != 0 || ac_line == 1))
}

#[derive(Debug, PartialEq, Eq)]
pub struct BatteryLabel {
    pub icon: char,
    pub text: String,
    /// status class: `charging`, `status-low` (≤ 20%) or `status-normal`
    pub class: &'static str,
}

/// The label to display for one reading; `None` (no battery) → hide the widget. Icons follow Segoe MDL2:
/// E850.. = battery 0–90%, E83F = 100%; charging uses E85A.. and E83E.
pub fn battery_label(reading: Option<(u8, bool)>) -> Option<BatteryLabel> {
    let (percent, charging) = reading?;
    let level = u32::from(percent.min(100) / 10);
    let icon = match (charging, level) {
        (true, 10) => '\u{e83e}',
        (true, l) => char::from_u32(0xe85a + l)?,
        (false, 10) => '\u{e83f}',
        (false, l) => char::from_u32(0xe850 + l)?,
    };
    let class = if charging {
        "charging"
    } else if percent <= 20 {
        "status-low"
    } else {
        "status-normal"
    };
    Some(BatteryLabel { icon, text: format!("{percent}%"), class })
}

fn read_system() -> Option<(u8, bool)> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    battery_reading(s.ACLineStatus, s.BatteryFlag, s.BatteryLifePercent)
}

pub struct Battery {
    base: Base,
    cfg: Config,
    current: Option<BatteryLabel>,
}

impl Battery {
    pub fn new(name: &str, opts: &Value) -> Battery {
        let cfg: Config = options(name, opts);
        let cb = cfg.callbacks.resolve("do_nothing", "do_nothing", "do_nothing");
        let mut base = Base::new(name, &format!("battery-widget {}", cfg.class_name), &cfg.label, None, cb);
        base.hidden = true;
        Battery { base, cfg, current: None }
    }

    fn sample(&mut self) -> bool {
        let now = battery_label(read_system());
        let changed = now != self.current;
        self.base.hidden = now.is_none();
        self.base.tooltip = now.as_ref().map(|b| b.text.clone());
        self.current = now;
        changed
    }
}

impl Widget for Battery {
    fn view(&self, ui: &mut Ui) {
        let Some(b) = &self.current else {
            // no battery: hide the root node so the bar leaves no empty gap
            self.base.shell(ui, |_| {});
            return;
        };
        let vals = [("{icon}", b.icon.to_string()), ("{percent}", b.text.trim_end_matches('%').to_string())];
        let class = b.class;
        self.base.shell(ui, |ui| self.base.labels(ui, &|t| fill_placeholders(t, &vals), &|_| class.to_string()));
    }

    fn interval(&self) -> u32 {
        self.cfg.update_interval.clamp(5_000, 300_000)
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init | Ev::Timer => {
                if self.sample() || matches!(ev, Ev::Init) {
                    cx.redraw();
                }
            }
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                let _ = base_callback(&name, &args);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_label_cases() {
        let full = battery_label(battery_reading(1, 8, 100)).unwrap();
        assert_eq!(full, BatteryLabel { icon: '\u{e83e}', text: "100%".into(), class: "charging" });
        let low = battery_label(battery_reading(0, 2, 15)).unwrap();
        assert_eq!(low, BatteryLabel { icon: '\u{e851}', text: "15%".into(), class: "status-low" });
        let mid = battery_label(battery_reading(0, 1, 55)).unwrap();
        assert_eq!((mid.icon, mid.class), ('\u{e855}', "status-normal"));
        // desktop PC: flag 128 (no battery) → hidden; unknown level → hidden
        assert_eq!(battery_label(battery_reading(1, 128, 255)), None);
        assert_eq!(battery_label(battery_reading(1, 255, 255)), None);
        assert_eq!(battery_label(battery_reading(0, 1, 255)), None);
        assert_eq!(battery_label(None), None);
    }
}
