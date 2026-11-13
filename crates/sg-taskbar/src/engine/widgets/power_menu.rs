//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! `yasb.power_menu.PowerMenuWidget` with `menu_style: popup` (`.power-menu-compact`):
//! avatar, name, account type, email, a "Manage accounts" button and the list of power buttons.

use std::rc::Rc;

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks, PopupOptions};
use crate::engine::sys::shell::{self, power};

#[derive(Deserialize, Clone)]
#[serde(default)]
struct PowerMenuConfig {
    label: String,
    show_user: bool,
    menu_style: String,
    popup: PopupOptions,
    profile_image_size: u32,
    callbacks: Callbacks,
}

impl Default for PowerMenuConfig {
    fn default() -> Self {
        PowerMenuConfig {
            label: "power".into(),
            show_user: false,
            menu_style: "fullscreen".into(),
            popup: PopupOptions::default(),
            profile_image_size: 64,
            callbacks: Callbacks::default(),
        }
    }
}

struct Profile {
    name: String,
    account: String,
    email: Option<String>,
    avatar: Option<Rc<Pixmap>>,
}

const ID_MANAGE: u32 = 1;
const ID_BUTTON: u32 = 100;

pub struct PowerMenu {
    base: Base,
    cfg: PowerMenuConfig,
    /// (button name, icon, label) in the same order as in the config
    buttons: Vec<(String, String, String)>,
    profile: Option<Profile>,
}

impl PowerMenu {
    pub fn new(name: &str, opts: &Value) -> PowerMenu {
        let cfg: PowerMenuConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_power_menu", "do_nothing", "do_nothing");
        let base = Base::new(name, "power-menu-widget", &cfg.label, None, cb);
        let mut buttons = Vec::new();
        if let Some(m) = opts.get("buttons").and_then(|b| b.as_mapping()) {
            for (k, v) in m {
                let Some(k) = k.as_str() else { continue };
                if k == "cancel" {
                    continue;
                }
                let seq = v.as_sequence();
                let icon = seq.and_then(|s| s.first()).and_then(|x| x.as_str()).unwrap_or("").to_string();
                let label = seq.and_then(|s| s.get(1)).and_then(|x| x.as_str()).unwrap_or(k).to_string();
                buttons.push((k.to_string(), icon, label));
            }
        }
        if cfg.menu_style != "popup" {
            crate::engine::log::warn("Power menu: kiểu 'fullscreen' chưa có trong bản Rust, dùng kiểu popup.");
        }
        PowerMenu { base, cfg, buttons, profile: None }
    }

    fn load_profile(&mut self, scale: f32) {
        if !self.cfg.show_user || self.profile.is_some() {
            return;
        }
        let size = self.cfg.profile_image_size;
        let px = (size as f32 * scale).round() as u32;
        let avatar = crate::engine::sys::user::avatar_path().and_then(|p| crate::engine::sys::image::load_file(&p, px, px, true));
        self.profile = Some(Profile {
            name: crate::engine::sys::user::username(),
            account: crate::engine::sys::user::account_type(),
            email: crate::engine::sys::user::email(),
            avatar,
        });
    }

    fn spec(&self) -> PopupSpec {
        PopupSpec { kind: 1, class: "power-menu-compact".into(), opts: self.cfg.popup.clone(), margins: Edges::default(), dir: Dir::Col }
    }

    fn run(name: &str) {
        match name {
            "lock" => power::lock(),
            "signout" => power::signout(),
            "sleep" => power::sleep(),
            "hibernate" => power::hibernate(),
            "restart" => power::restart(),
            "shutdown" => power::shutdown(),
            "force_shutdown" => power::force_shutdown(),
            "force_restart" => power::force_restart(),
            _ => {}
        }
    }
}

impl Widget for PowerMenu {
    fn view(&self, ui: &mut Ui) {
        self.base.shell(ui, |ui| self.base.labels(ui, &|t| t.to_string(), &|_| String::new()));
    }

    fn popup_view(&self, _kind: u32, ui: &mut Ui) {
        let center = Some((Align::Center, Align::Center));
        if let Some(p) = &self.profile {
            ui.col("profile-info", |ui| {
                let size = self.cfg.profile_image_size as f32;
                let mut av = Node::new("QLabel", "profile-avatar");
                av.fixed = (Some(size), Some(size));
                av.halign = Align::Center;
                av.item_align = center;
                if let Some(pix) = &p.avatar {
                    av.content = Content::Image(Image { pix: pix.clone(), w: size, h: size, circle: true });
                }
                ui.add(av);
                let id = ui.label_centered("profile-username", p.name.clone());
                ui.node(id).item_align = center;
                let id = ui.label_centered("profile-account-type", p.account.clone());
                ui.node(id).item_align = center;
                if let Some(e) = &p.email {
                    let id = ui.label_centered("profile-email", e.clone());
                    ui.node(id).item_align = center;
                }
                let b = ui.button("manage-accounts", "Manage accounts", ID_MANAGE);
                ui.node(b).item_align = center;
            });
        }
        ui.col("buttons", |ui| {
            for (i, (name, icon, label)) in self.buttons.iter().enumerate() {
                let mut f = Node::new("QFrame", &format!("button {}", name.replace('_', "-")));
                f.dir = Dir::Row;
                f.on_press = Some(ID_BUTTON + i as u32);
                f.hover_class = true;
                ui.with(f, |ui| {
                    if !icon.is_empty() {
                        ui.label("icon", icon.clone());
                    }
                    ui.label("label", label.clone());
                    ui.stretch();
                });
            }
        });
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if base_callback(&name, &args) {
                    return;
                }
                if name == "toggle_power_menu" {
                    self.load_profile(cx.scale);
                    cx.open_popup(self.spec());
                }
            }
            Ev::Click { id: ID_MANAGE, .. } => {
                cx.close_popup();
                shell::startfile("ms-settings:accounts");
            }
            Ev::Press { id } if id >= ID_BUTTON => {
                if let Some((name, _, _)) = self.buttons.get((id - ID_BUTTON) as usize).cloned() {
                    cx.close_popup();
                    Self::run(&name);
                }
            }
            _ => {}
        }
    }
}
