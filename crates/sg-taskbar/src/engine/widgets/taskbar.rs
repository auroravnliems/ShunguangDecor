//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! `yasb.taskbar.TaskbarWidget` (slim version): apps pinned to the Windows taskbar (in the right order) + running
//! apps. Each app is an `.app-container` (with `pinned`, `running`, `foreground`, `minimized` added) containing
//! an `.app-icon`; click to open / switch to / minimize, middle-click opens a new window, right-click shows a menu.

use std::rc::Rc;
use std::time::Instant;

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::options;
use crate::engine::sys::apps::{self, AppWindow, Pinned};

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct IgnoreApps {
    processes: Vec<String>,
    titles: Vec<String>,
    classes: Vec<String>,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct TitleLabel {
    enabled: bool,
    show: String,
    min_length: usize,
    max_length: usize,
}

impl Default for TitleLabel {
    fn default() -> Self {
        TitleLabel { enabled: false, show: "focused".into(), min_length: 10, max_length: 30 }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct TaskbarConfig {
    class_name: String,
    icon_size: u32,
    tooltip: bool,
    /// extension: show Windows pinned apps
    show_pinned: bool,
    /// extension: also show running apps that are not pinned
    show_running: bool,
    monitor_exclusive: bool,
    title_label: TitleLabel,
    ignore_apps: IgnoreApps,
    update_interval: u32,
}

impl Default for TaskbarConfig {
    fn default() -> Self {
        TaskbarConfig {
            class_name: String::new(),
            icon_size: 16,
            tooltip: true,
            show_pinned: true,
            show_running: true,
            monitor_exclusive: false,
            title_label: TitleLabel::default(),
            ignore_apps: IgnoreApps::default(),
            update_interval: 1000,
        }
    }
}

struct Item {
    name: String,
    pinned: Option<Pinned>,
    windows: Vec<AppWindow>,
    icon: Option<Rc<Pixmap>>,
}

const ID_APP: u32 = 100;
const M_NEW: u32 = 1;
const M_CLOSE: u32 = 2;
const M_CLOSE_ALL: u32 = 3;
const M_LOCATION: u32 = 4;

pub struct Taskbar {
    base: Base,
    cfg: TaskbarConfig,
    pinned: Vec<Pinned>,
    pinned_at: Option<Instant>,
    items: Vec<Item>,
    foreground: isize,
    menu_for: Option<usize>,
    scale: f32,
    monitor: isize,
}

impl Taskbar {
    pub fn new(name: &str, opts: &Value) -> Taskbar {
        let mut cfg: TaskbarConfig = options(name, opts);
        cfg.ignore_apps.processes = cfg.ignore_apps.processes.iter().map(|s| s.to_ascii_lowercase()).collect();
        let base = Base::new(name, &format!("taskbar-widget {}", cfg.class_name), "", None, ["do_nothing".into(), "do_nothing".into(), "do_nothing".into()]);
        Taskbar { base, cfg, pinned: Vec::new(), pinned_at: None, items: Vec::new(), foreground: 0, menu_for: None, scale: 1.0, monitor: 0 }
    }

    fn ignored(&self, w: &AppWindow) -> bool {
        let ig = &self.cfg.ignore_apps;
        let exe = w.exe.rsplit('\\').next().unwrap_or("");
        if ig.processes.iter().any(|p| p == exe) || ig.titles.iter().any(|t| t == &w.title) {
            return true;
        }
        if !ig.classes.is_empty() {
            let c = crate::engine::sys::process::class_name(windows::Win32::Foundation::HWND(w.hwnd as *mut _));
            if ig.classes.contains(&c) {
                return true;
            }
        }
        if self.cfg.monitor_exclusive && crate::engine::win::monitor_of_window(windows::Win32::Foundation::HWND(w.hwnd as *mut _)) != self.monitor {
            return true;
        }
        false
    }

    /// Rebuilds the list: pinned apps in order, then the other running apps (merged by exe/AUMID).
    fn refresh(&mut self) -> bool {
        if self.cfg.show_pinned && self.pinned_at.is_none_or(|t| t.elapsed().as_secs() >= 10) {
            self.pinned = apps::pinned();
            self.pinned_at = Some(Instant::now());
        }
        let wins: Vec<AppWindow> = apps::windows().into_iter().filter(|w| !self.ignored(w)).collect();
        let px = (self.cfg.icon_size as f32 * self.scale).round() as u32;
        let mut items: Vec<Item> = Vec::new();
        let mut used = vec![false; wins.len()];
        if self.cfg.show_pinned {
            for p in &self.pinned {
                let mut ws = Vec::new();
                for (i, w) in wins.iter().enumerate() {
                    if !used[i] && apps::matches(p, w) {
                        used[i] = true;
                        ws.push(w.clone());
                    }
                }
                items.push(Item { name: p.name.clone(), pinned: Some(p.clone()), windows: ws, icon: apps::shell_icon(&p.lnk, px) });
            }
        }
        if self.cfg.show_running {
            for (i, w) in wins.iter().enumerate() {
                if used[i] {
                    continue;
                }
                let key = |x: &AppWindow| if x.aumid.is_empty() { x.exe.clone() } else { x.aumid.clone() };
                if let Some(it) = items.iter_mut().find(|it| it.pinned.is_none() && it.windows.first().is_some_and(|f| key(f) == key(w))) {
                    it.windows.push(w.clone());
                    continue;
                }
                let name = crate::engine::sys::process::file_description(&w.exe).unwrap_or_else(|| w.title.clone());
                let icon = crate::engine::sys::process::window_hicon(windows::Win32::Foundation::HWND(w.hwnd as *mut _), px > 16)
                    .and_then(|h| crate::engine::sys::image::from_hicon(h, px))
                    .or_else(|| apps::shell_icon(&w.exe, px));
                items.push(Item { name, pinned: None, windows: vec![w.clone()], icon });
            }
        }
        let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize };
        let sig = |v: &[Item]| v.iter().map(|i| (i.name.clone(), i.windows.iter().map(|w| (w.hwnd, w.minimized, w.title.clone())).collect::<Vec<_>>())).collect::<Vec<_>>();
        let changed = sig(&items) != sig(&self.items) || fg != self.foreground;
        self.items = items;
        self.foreground = fg;
        changed
    }

    fn class_of(&self, it: &Item) -> String {
        let mut c = String::from("app-container");
        if it.windows.len() > 1 {
            c.push_str(" grouped");
        }
        if it.pinned.is_some() {
            c.push_str(" pinned");
        }
        if !it.windows.is_empty() {
            if it.windows.iter().any(|w| w.hwnd == self.foreground) {
                c.push_str(" foreground");
            } else if it.windows.iter().all(|w| w.minimized) {
                c.push_str(" running minimized");
            } else {
                c.push_str(" running");
            }
        }
        c
    }

    fn launch_new(it: &Item) {
        if let Some(p) = &it.pinned {
            apps::launch(&p.lnk);
        } else if let Some(w) = it.windows.first() {
            apps::launch(&w.exe);
        }
    }

    fn on_left(&mut self, idx: usize) {
        let Some(it) = self.items.get(idx) else { return };
        if it.windows.is_empty() {
            Self::launch_new(it);
            return;
        }
        if let Some(pos) = it.windows.iter().position(|w| w.hwnd == self.foreground) {
            if it.windows.len() == 1 {
                apps::minimize(it.windows[0].hwnd);
            } else {
                // multiple windows: switch to the next window
                let next = &it.windows[(pos + 1) % it.windows.len()];
                apps::activate(next.hwnd);
            }
        } else {
            apps::activate(it.windows[0].hwnd);
        }
    }
}

impl Widget for Taskbar {
    fn view(&self, ui: &mut Ui) {
        let size = self.cfg.icon_size as f32;
        self.base.shell(ui, |ui| {
            for (i, it) in self.items.iter().enumerate() {
                let mut c = Node::new("QFrame", &self.class_of(it));
                c.dir = Dir::Row;
                c.on_click = Some(ID_APP + i as u32);
                if !it.windows.is_empty() {
                    c.on_hover = Some(ID_APP + i as u32);
                }
                if self.cfg.tooltip {
                    let t = match it.windows.len() {
                        0 => it.name.clone(),
                        1 => it.windows[0].title.clone(),
                        n => format!("{} · {n} cửa sổ", it.name),
                    };
                    c.tooltip = Some(t);
                }
                ui.with(c, |ui| {
                    let mut n = Node::new("QLabel", "app-icon");
                    n.halign = Align::Center;
                    if let Some(p) = &it.icon {
                        n.content = Content::Image(Image { pix: p.clone(), w: size, h: size, circle: false });
                    }
                    ui.add(n);
                    let tl = &self.cfg.title_label;
                    let focused = it.windows.iter().any(|w| w.hwnd == self.foreground);
                    if tl.enabled && !it.windows.is_empty() && (tl.show == "always" || focused) {
                        let w = it.windows.iter().find(|w| w.hwnd == self.foreground).unwrap_or(&it.windows[0]);
                        let mut t: String = w.title.chars().take(tl.max_length).collect();
                        if w.title.chars().count() > tl.max_length {
                            t.push('…');
                        }
                        ui.label("app-title", t);
                    }
                });
            }
        });
    }

    fn interval(&self) -> u32 {
        self.cfg.update_interval.max(300)
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init => {
                self.scale = cx.scale;
                self.monitor = cx.monitor;
                self.refresh();
                cx.redraw();
            }
            Ev::Timer | Ev::Sys(SysEv::Foreground(_)) | Ev::Sys(SysEv::Destroy(_)) | Ev::Sys(SysEv::NameChange(_)) => {
                if self.refresh() {
                    cx.redraw();
                }
            }
            Ev::Hover { id } if id >= ID_APP => {
                // thumbnail of the app's focused (foreground) window, otherwise its first window
                let Some(it) = self.items.get((id - ID_APP) as usize) else { return };
                let w = it.windows.iter().find(|w| w.hwnd == self.foreground).or_else(|| it.windows.first());
                if let Some(w) = w {
                    cx.thumbnail(w.hwnd);
                }
            }
            Ev::Click { id, btn } if id >= ID_APP => {
                let idx = (id - ID_APP) as usize;
                match btn {
                    crate::engine::win::MouseBtn::Left => self.on_left(idx),
                    crate::engine::win::MouseBtn::Middle => {
                        if let Some(it) = self.items.get(idx) {
                            Self::launch_new(it);
                        }
                    }
                    crate::engine::win::MouseBtn::Right => {
                        let Some(it) = self.items.get(idx) else { return };
                        self.menu_for = Some(idx);
                        let mut m = vec![MenuEntry::Item { id: M_NEW, text: format!("Mở {}", it.name), enabled: true }];
                        if let Some(p) = &it.pinned
                            && !p.target.is_empty() {
                                m.push(MenuEntry::Item { id: M_LOCATION, text: "Mở vị trí file".into(), enabled: true });
                            }
                        if !it.windows.is_empty() {
                            m.push(MenuEntry::Sep);
                            if it.windows.len() > 1 {
                                m.push(MenuEntry::Item { id: M_CLOSE_ALL, text: format!("Đóng tất cả {} cửa sổ", it.windows.len()), enabled: true });
                            } else {
                                m.push(MenuEntry::Item { id: M_CLOSE, text: "Đóng cửa sổ".into(), enabled: true });
                            }
                        }
                        cx.context_menu(m);
                    }
                }
                let _ = self.refresh();
                cx.redraw();
            }
            Ev::Click { id, .. } if (M_NEW..=M_LOCATION).contains(&id) => {
                let Some(it) = self.menu_for.and_then(|i| self.items.get(i)) else { return };
                match id {
                    M_NEW => Self::launch_new(it),
                    M_CLOSE | M_CLOSE_ALL => {
                        for w in &it.windows {
                            apps::close(w.hwnd);
                        }
                    }
                    M_LOCATION => {
                        if let Some(p) = &it.pinned {
                            crate::engine::sys::shell::run("explorer.exe", &[&format!("/select,{}", p.target)], true);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
