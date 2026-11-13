//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! `yasb.home.HomeWidget`: a Home button that opens a menu (About, Settings, Task Manager, optional folders, power).

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks, PopupOptions};
use crate::engine::sys::shell::{self, power};

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct MenuItem {
    title: Option<String>,
    path: Option<String>,
    uri: Option<String>,
    command: Option<String>,
    separator: Option<bool>,
    args: Option<Vec<String>>,
    shell: Option<bool>,
    show_window: Option<bool>,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct MenuLabels {
    shutdown: String,
    restart: String,
    hibernate: String,
    logout: String,
    lock: String,
    sleep: String,
    system: String,
    about: String,
    task_manager: String,
}

impl Default for MenuLabels {
    fn default() -> Self {
        MenuLabels {
            shutdown: "Shutdown".into(),
            restart: "Restart".into(),
            hibernate: "Hibernate".into(),
            logout: "Logout".into(),
            lock: "Lock".into(),
            sleep: "Sleep".into(),
            system: "System Settings".into(),
            about: "About This PC".into(),
            task_manager: "Task Manager".into(),
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct HomeConfig {
    label: String,
    menu_list: Option<Vec<MenuItem>>,
    power_menu: bool,
    system_menu: bool,
    blur: bool,
    round_corners: bool,
    round_corners_type: String,
    border_color: String,
    alignment: String,
    direction: String,
    offset_top: i32,
    offset_left: i32,
    menu_labels: MenuLabels,
    callbacks: Callbacks,
}

impl Default for HomeConfig {
    fn default() -> Self {
        HomeConfig {
            label: "\u{e71a}".into(),
            menu_list: None,
            power_menu: true,
            system_menu: true,
            blur: false,
            round_corners: true,
            round_corners_type: "normal".into(),
            border_color: "System".into(),
            alignment: "left".into(),
            direction: "down".into(),
            offset_top: 6,
            offset_left: 0,
            menu_labels: MenuLabels::default(),
            callbacks: Callbacks::default(),
        }
    }
}

#[derive(Clone)]
enum Action {
    About,
    Settings,
    TaskManager,
    Item(MenuItem),
    Hibernate,
    Sleep,
    Restart,
    Shutdown,
    Lock,
    Logout,
}

enum Entry {
    Sep,
    Item(String, Action),
}

pub struct Home {
    base: Base,
    cfg: HomeConfig,
    entries: Vec<Entry>,
}

impl Home {
    pub fn new(name: &str, opts: &Value) -> Home {
        let cfg: HomeConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_menu", "do_nothing", "do_nothing");
        let base = Base::new(name, "home-widget", &cfg.label, None, cb);
        let l = &cfg.menu_labels;
        let mut entries = Vec::new();
        if cfg.system_menu {
            entries.push(Entry::Item(l.about.clone(), Action::About));
            entries.push(Entry::Sep);
            entries.push(Entry::Item(l.system.clone(), Action::Settings));
            entries.push(Entry::Item(l.task_manager.clone(), Action::TaskManager));
            entries.push(Entry::Sep);
        }
        if let Some(list) = &cfg.menu_list {
            for it in list {
                if it.separator.unwrap_or(false) {
                    entries.push(Entry::Sep);
                } else if let Some(t) = &it.title {
                    entries.push(Entry::Item(t.clone(), Action::Item(it.clone())));
                }
            }
        }
        if cfg.menu_list.as_ref().is_some_and(|l| !l.is_empty()) && cfg.power_menu {
            entries.push(Entry::Sep);
        }
        if cfg.power_menu {
            entries.push(Entry::Item(l.hibernate.clone(), Action::Hibernate));
            entries.push(Entry::Item(l.sleep.clone(), Action::Sleep));
            entries.push(Entry::Item(l.restart.clone(), Action::Restart));
            entries.push(Entry::Item(l.shutdown.clone(), Action::Shutdown));
            entries.push(Entry::Sep);
            entries.push(Entry::Item(l.lock.clone(), Action::Lock));
            entries.push(Entry::Item(l.logout.clone(), Action::Logout));
        }
        Home { base, cfg, entries }
    }

    fn spec(&self) -> PopupSpec {
        let c = &self.cfg;
        PopupSpec {
            kind: 1,
            class: "home-menu".into(),
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
            margins: Edges::default(),
            dir: Dir::Col,
        }
    }

    fn run(action: &Action) {
        match action {
            Action::About => shell::shell_open("winver", None),
            Action::Settings => shell::startfile("ms-settings:"),
            Action::TaskManager => shell::shell_open("taskmgr", None),
            Action::Hibernate => power::hibernate(),
            Action::Sleep => power::sleep(),
            Action::Restart => power::restart(),
            Action::Shutdown => power::shutdown(),
            Action::Lock => power::lock(),
            Action::Logout => power::signout(),
            Action::Item(it) => {
                let n = [it.command.is_some(), it.uri.is_some(), it.path.is_some()].iter().filter(|x| **x).count();
                if n != 1 {
                    crate::engine::log::error("Home menu item must define exactly one of: 'path', 'uri', or 'command'.");
                    return;
                }
                if let Some(cmd) = &it.command {
                    let show = it.show_window.unwrap_or(false);
                    match &it.args {
                        Some(args) if !it.shell.unwrap_or(false) => {
                            let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                            shell::run(cmd, &a, show);
                        }
                        Some(args) => shell::run_shell(&format!("{cmd} {}", args.join(" ")), show),
                        None => shell::run_shell(cmd, show),
                    }
                } else if let Some(uri) = &it.uri {
                    shell::startfile(uri);
                } else if let Some(p) = &it.path {
                    let p = shell::expand_user(p);
                    if std::path::Path::new(&p).exists() {
                        shell::startfile(&p);
                    } else {
                        crate::engine::log::error(&format!("The system cannot find the file specified: '{p}'"));
                    }
                }
            }
        }
    }
}

impl Widget for Home {
    fn view(&self, ui: &mut Ui) {
        self.base.shell(ui, |ui| self.base.labels(ui, &|t| t.to_string(), &|_| String::new()));
    }

    fn popup_view(&self, _kind: u32, ui: &mut Ui) {
        for (i, e) in self.entries.iter().enumerate() {
            match e {
                Entry::Sep => {
                    ui.separator();
                }
                Entry::Item(title, _) => {
                    let mut item = Node::new("QWidget", "");
                    item.on_press = Some(i as u32 + 1);
                    ui.with(item, |ui| {
                        ui.label("menu-item", title.clone());
                    });
                }
            }
        }
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if !base_callback(&name, &args) && name == "toggle_menu" {
                    cx.open_popup(self.spec());
                }
            }
            Ev::Press { id } if id > 0 => {
                if let Some(Entry::Item(_, a)) = self.entries.get(id as usize - 1) {
                    let a = a.clone();
                    cx.close_popup();
                    Self::run(&a);
                }
            }
            _ => {}
        }
    }
}
