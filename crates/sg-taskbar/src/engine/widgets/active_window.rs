//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! `yasb.active_window.ActiveWindowWidget`: icon + title of the active window (`{win[title]}`...).

use std::rc::Rc;

use regex::Regex;
use serde::Deserialize;
use serde_yaml::Value;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetParent};

use super::*;
use crate::engine::config::{options, Callbacks};
use crate::engine::sys::process::{self, WindowInfo};

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct Rewrite {
    pattern: String,
    replacement: String,
    case: Option<String>,
}

#[derive(Deserialize, Clone, Default)]
#[serde(default)]
struct IgnoreWindow {
    classes: Vec<String>,
    processes: Vec<String>,
    titles: Vec<String>,
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct ActiveWindowConfig {
    label: String,
    label_alt: String,
    class_name: String,
    label_no_window: Option<String>,
    label_icon: bool,
    label_icon_size: u32,
    max_length: Option<usize>,
    max_length_ellipsis: String,
    monitor_exclusive: bool,
    rewrite: Vec<Rewrite>,
    ignore_window: IgnoreWindow,
    callbacks: Callbacks,
}

impl Default for ActiveWindowConfig {
    fn default() -> Self {
        ActiveWindowConfig {
            label: "{win[title]}".into(),
            label_alt: "[class_name='{win[class_name]}' exe='{win[process][name]}' hwnd={win[hwnd]}]".into(),
            class_name: String::new(),
            label_no_window: None,
            label_icon: true,
            label_icon_size: 16,
            max_length: None,
            max_length_ellipsis: "...".into(),
            monitor_exclusive: true,
            rewrite: Vec::new(),
            ignore_window: IgnoreWindow::default(),
            callbacks: Callbacks::default(),
        }
    }
}

const IGNORED_TITLES: &[&str] = &["", " ", "FolderView", "Program Manager", "python3", "pythonw3", "YasbBar", "Search", "Start"];
const IGNORED_CLASSES: &[&str] =
    &["WorkerW", "TopLevelWindowForOverflowXamlIsland", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Windows.UI.Core.CoreWindow"];
const IGNORED_PROCESSES: &[&str] = &["SearchHost.exe", "komorebi.exe", "yasb.exe", "Flow.Launcher.exe", "tachyon-taskbar.exe"];

pub struct ActiveWindow {
    base: Base,
    cfg: ActiveWindowConfig,
    win: Option<WindowInfo>,
    app_name: String,
    icon: Option<Rc<Pixmap>>,
    tracked: Option<isize>,
    no_window: bool,
    scale: f32,
    monitor: isize,
    rewrites: Vec<(Regex, String, Option<String>)>,
}

impl ActiveWindow {
    pub fn new(name: &str, opts: &Value) -> ActiveWindow {
        let mut cfg: ActiveWindowConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_label", "do_nothing", "do_nothing");
        let mut base = Base::new(name, &format!("active-window-widget {}", cfg.class_name), "", None, cb);
        cfg.ignore_window.classes.extend(IGNORED_CLASSES.iter().map(|s| s.to_string()));
        cfg.ignore_window.processes.extend(IGNORED_PROCESSES.iter().map(|s| s.to_string()));
        cfg.ignore_window.titles.extend(IGNORED_TITLES.iter().map(|s| s.to_string()));
        let rewrites = cfg
            .rewrite
            .iter()
            .filter(|r| !r.pattern.is_empty() && !r.replacement.is_empty())
            .filter_map(|r| match Regex::new(&r.pattern) {
                Ok(re) => Some((re, python_repl(&r.replacement), r.case.clone())),
                Err(e) => {
                    crate::engine::log::warn(&format!("Invalid regex pattern '{}': {e}", r.pattern));
                    None
                }
            })
            .collect();
        base.hidden = cfg.label_no_window.as_deref().unwrap_or("").is_empty();
        ActiveWindow { base, cfg, win: None, app_name: String::new(), icon: None, tracked: None, no_window: true, scale: 1.0, monitor: 0, rewrites }
    }

    fn rewrite(&self, text: &str) -> String {
        let mut r = text.to_string();
        for (re, rep, case) in &self.rewrites {
            if re.is_match(&r) {
                r = re.replace_all(&r, rep.as_str()).into_owned();
                if let Some(c) = case {
                    r = match c.as_str() {
                        "lower" => r.to_lowercase(),
                        "upper" => r.to_uppercase(),
                        "title" => r.split(' ').map(capitalize).collect::<Vec<_>>().join(" "),
                        "capitalize" => capitalize(&r.to_lowercase()),
                        _ => r,
                    };
                }
            }
        }
        r
    }

    fn set_no_window(&mut self) {
        self.tracked = None;
        self.no_window = true;
        if self.cfg.label_no_window.as_deref().unwrap_or("").is_empty() {
            self.base.hidden = true;
        }
    }

    fn on_focus(&mut self, h: isize) {
        let hwnd = HWND(h as *mut _);
        let Some(mut info) = process::window_info(hwnd) else { return };
        if info.title.is_empty() || info.title == crate::engine::win::APP_TITLE || crate::engine::win::is_engine_window(&process::class_name(hwnd), info.pid, std::process::id()) {
            return;
        }
        if self.cfg.monitor_exclusive && info.monitor != self.monitor {
            self.set_no_window();
            return;
        }
        if unsafe { GetForegroundWindow() } != hwnd {
            return;
        }
        let ig = &self.cfg.ignore_window;
        if ig.titles.iter().any(|t| t == info.title.trim()) || ig.classes.contains(&info.class_name) || ig.processes.contains(&info.process_name) {
            if IGNORED_TITLES.contains(&info.title.as_str()) {
                self.set_no_window();
            }
            return;
        }
        let app = process::app_name(info.pid);
        info.title = self.rewrite(&info.title);
        info.process_name = self.rewrite(&info.process_name);
        self.app_name = self.rewrite(&app);
        if let Some(max) = self.cfg.max_length
            && info.title.chars().count() > max {
                info.title = format!("{}{}", info.title.chars().take(max).collect::<String>(), self.cfg.max_length_ellipsis);
            }
        if self.cfg.label_icon {
            let px = (self.cfg.label_icon_size as f32 * self.scale).round() as u32;
            self.icon = process::window_hicon(hwnd, px > 16)
                .and_then(|i| crate::engine::sys::image::from_hicon(i, px))
                .or_else(|| crate::engine::sys::image::exe_icon(&info.exe_path, px));
        }
        self.tracked = Some(h);
        self.win = Some(info);
        self.no_window = false;
        self.base.hidden = false;
    }

    fn format(&self, template: &str) -> String {
        let Some(w) = &self.win else { return template.to_string() };
        let re = Regex::new(r"\{\{|\}\}|\{win\[(\w+)\](?:\[(\w+)\])?\}").unwrap();
        re.replace_all(template, |c: &regex::Captures| {
            let m = &c[0];
            if m == "{{" {
                return "{".to_string();
            }
            if m == "}}" {
                return "}".to_string();
            }
            match (c.get(1).map(|x| x.as_str()), c.get(2).map(|x| x.as_str())) {
                (Some("title"), None) => w.title.clone(),
                (Some("class_name"), None) => w.class_name.clone(),
                (Some("hwnd"), None) => w.hwnd.to_string(),
                (Some("app_name"), None) => self.app_name.clone(),
                (Some("process"), Some("name")) => w.process_name.clone(),
                (Some("process"), Some("pid")) => w.pid.to_string(),
                (Some("process"), Some("exe")) | (Some("process"), Some("path")) => w.exe_path.clone(),
                _ => m.to_string(),
            }
        })
        .into_owned()
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Python uses `\1` for groups; Rust regex uses `${1}`.
fn python_repl(s: &str) -> String {
    let re = Regex::new(r"\\(\d+)").unwrap();
    re.replace_all(&s.replace('$', "$$"), "$${$1}").into_owned()
}

impl Widget for ActiveWindow {
    fn view(&self, ui: &mut Ui) {
        self.base.shell(ui, |ui| {
            if self.cfg.label_icon {
                let mut n = Node::new("QLabel", "label icon");
                n.hidden = self.no_window || self.icon.is_none();
                if let Some(p) = &self.icon {
                    let s = self.cfg.label_icon_size as f32;
                    n.content = Content::Image(Image { pix: p.clone(), w: s, h: s, circle: false });
                }
                ui.add(n);
            }
            let text = if self.no_window {
                self.cfg.label_no_window.clone().unwrap_or_default()
            } else {
                let t = if self.base.show_alt { &self.cfg.label_alt } else { &self.cfg.label };
                self.format(t)
            };
            ui.label("label", text);
        });
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init => {
                self.scale = cx.scale;
                self.monitor = cx.monitor;
                let fg = unsafe { GetForegroundWindow() };
                if !fg.0.is_null() {
                    self.on_focus(fg.0 as isize);
                }
                cx.redraw();
            }
            Ev::Sys(SysEv::Foreground(h)) | Ev::Sys(SysEv::MoveSizeEnd(h)) => {
                self.on_focus(h);
                cx.redraw();
            }
            Ev::Sys(SysEv::NameChange(h)) => {
                if self.tracked == Some(h) {
                    self.on_focus(h);
                    cx.redraw();
                }
            }
            Ev::Sys(SysEv::Destroy(h)) => {
                if self.tracked != Some(h) {
                    return;
                }
                let hwnd = HWND(h as *mut _);
                if unsafe { GetParent(hwnd) }.is_ok_and(|p| !p.0.is_null()) {
                    return;
                }
                let fg = unsafe { GetForegroundWindow() };
                if !fg.0.is_null() && fg != hwnd {
                    self.on_focus(fg.0 as isize);
                    if self.tracked == Some(fg.0 as isize) {
                        cx.redraw();
                        return;
                    }
                }
                self.set_no_window();
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
