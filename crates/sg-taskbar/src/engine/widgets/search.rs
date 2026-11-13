//! `yasb.search.SearchWidget` (a ShunguangDecor extension): a magnifier icon (+ the text "Search" in the app language when
//! `label: true`); clicking opens the Windows search box (Win+S). The text is filled in by the config generator according to the app language.

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks};
use crate::engine::sys::start;

#[derive(Deserialize, Clone)]
#[serde(default)]
struct Config {
    /// true: show the text next to the magnifier
    label: bool,
    text: String,
    icon: String,
    class_name: String,
    callbacks: Callbacks,
}

impl Default for Config {
    fn default() -> Self {
        Config { label: false, text: "Search".into(), icon: "\u{e721}".into(), class_name: String::new(), callbacks: Callbacks::default() }
    }
}

pub struct Search {
    base: Base,
    cfg: Config,
}

impl Search {
    pub fn new(name: &str, opts: &Value) -> Search {
        let cfg: Config = options(name, opts);
        let cb = cfg.callbacks.resolve("open_search", "do_nothing", "do_nothing");
        let base = Base::new(name, &format!("search-widget {}", cfg.class_name), "", None, cb);
        Search { base, cfg }
    }
}

impl Widget for Search {
    fn view(&self, ui: &mut Ui) {
        self.base.shell(ui, |ui| {
            ui.label_centered("icon", self.cfg.icon.clone());
            if self.cfg.label {
                ui.label_centered("label", self.cfg.text.clone());
            }
        });
    }

    fn event(&mut self, ev: Ev, _cx: &mut Cx) {
        if let Ev::Click { id: BASE, btn } = ev {
            let (name, args) = split_callback(self.base.callback_for(btn));
            if !base_callback(&name, &args) && name == "open_search" {
                start::open_search();
            }
        }
    }
}
