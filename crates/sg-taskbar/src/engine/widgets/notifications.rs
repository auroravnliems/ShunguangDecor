//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! `yasb.notifications.NotificationsWidget`: the Windows notification count, hidden when there are none (hide_empty);
//! clicking opens the Notification Center (Win+N).

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks};

#[derive(Deserialize, Clone)]
#[serde(default)]
struct Icons {
    new: String,
    default: String,
}

impl Default for Icons {
    fn default() -> Self {
        Icons { new: "\u{f009e}".into(), default: "\u{f009a}".into() }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct NotificationsConfig {
    label: String,
    label_alt: String,
    class_name: String,
    hide_empty: bool,
    tooltip: bool,
    icons: Icons,
    callbacks: Callbacks,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        NotificationsConfig {
            label: "{count} new notifications".into(),
            label_alt: "{count} new notifications".into(),
            class_name: String::new(),
            hide_empty: false,
            tooltip: true,
            icons: Icons::default(),
            callbacks: Callbacks::default(),
        }
    }
}

const MAX_COUNT: u32 = 9;

pub struct Notifications {
    base: Base,
    cfg: NotificationsConfig,
    count: u32,
}

impl Notifications {
    pub fn new(name: &str, opts: &Value) -> Notifications {
        let cfg: NotificationsConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_menu", "do_nothing", "do_nothing");
        let mut base = Base::new(name, &format!("notification-widget {}", cfg.class_name), &cfg.label, Some(&cfg.label_alt), cb);
        base.hidden = cfg.hide_empty;
        Notifications { base, cfg, count: 0 }
    }
}

impl Widget for Notifications {
    fn view(&self, ui: &mut Ui) {
        let icon = if self.count > 0 { &self.cfg.icons.new } else { &self.cfg.icons.default };
        let count = if self.count > MAX_COUNT { format!("{MAX_COUNT}+") } else { self.count.to_string() };
        let has_new = self.count > 0;
        self.base.shell(ui, |ui| {
            self.base.labels(
                ui,
                &|t| t.replace("{count}", &count).replace("{icon}", icon),
                &|_| if has_new { "new-notification".into() } else { String::new() },
            )
        });
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Sys(SysEv::Notifications(n)) => {
                self.count = n;
                self.base.hidden = n == 0 && self.cfg.hide_empty;
                if self.cfg.tooltip {
                    self.base.tooltip = Some(format!("{n} notifications"));
                }
                cx.redraw();
            }
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if base_callback(&name, &args) {
                    return;
                }
                match name.as_str() {
                    "toggle_notification" | "toggle_menu" => {
                        // YASB's own notification menu is not implemented → open Windows' Notification Center
                        let _ = crate::engine::sys::shell::system_function(if crate::engine::win::windows_build() >= 22000 { "notification_center" } else { "quick_settings" });
                    }
                    "toggle_label" => {
                        self.base.show_alt = !self.base.show_alt;
                        cx.redraw();
                    }
                    "clear_notifications" => {}
                    other => crate::engine::log::warn(&format!("Notifications: callback không rõ '{other}'")),
                }
            }
            _ => {}
        }
    }
}
