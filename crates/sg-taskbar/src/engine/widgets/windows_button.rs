//! `yasb.windows_button.WindowsButtonWidget` (a ShunguangDecor extension): a Windows button at the bar's corner.
//! The logo is 4 squares drawn with D2D in the text color (`.windows-button-widget .logo`); hovering changes the background like a
//! YASB button (`.windows-button-widget:hover`); clicking opens the real Start menu, like pressing the Windows key.

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks};
use crate::engine::sys::start;

#[derive(Deserialize, Clone)]
#[serde(default)]
struct Config {
    class_name: String,
    /// logo side length (logical px)
    size: f32,
    callbacks: Callbacks,
}

impl Default for Config {
    fn default() -> Self {
        Config { class_name: String::new(), size: 16.0, callbacks: Callbacks::default() }
    }
}

pub struct WindowsButton {
    base: Base,
    size: f32,
}

impl WindowsButton {
    pub fn new(name: &str, opts: &Value) -> WindowsButton {
        let cfg: Config = options(name, opts);
        let cb = cfg.callbacks.resolve("open_start", "do_nothing", "do_nothing");
        let base = Base::new(name, &format!("windows-button-widget {}", cfg.class_name), "", None, cb);
        WindowsButton { base, size: cfg.size.clamp(8.0, 48.0) }
    }
}

impl Widget for WindowsButton {
    fn view(&self, ui: &mut Ui) {
        let size = self.size;
        self.base.shell(ui, |ui| {
            let mut n = Node::new("QLabel", "logo");
            n.halign = Align::Center;
            n.content = Content::WinLogo(size);
            ui.add(n);
        });
    }

    fn event(&mut self, ev: Ev, _cx: &mut Cx) {
        if let Ev::Click { id: BASE, btn } = ev {
            let (name, args) = split_callback(self.base.callback_for(btn));
            if !base_callback(&name, &args) && name == "open_start" {
                start::open_start();
            }
        }
    }
}
