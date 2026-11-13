//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! `yasb.systray.SystrayWidget`: system tray icons. Icons that Windows shows "on the taskbar"
//! (Settings → Taskbar → Other system tray icons, registry `NotifyIconSettings\*\IsPromoted`) sit directly
//! on the bar (`.pinned-container`); the rest sit behind the arrow button `.unpinned-visibility-btn`,
//! which opens a grid-style `.systray-popup` like Windows' hidden-icons tray.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, PopupOptions};
use crate::engine::sys::systray::{self, Click, TrayIcon};
use crate::engine::win::MouseBtn;

#[derive(Deserialize, Clone)]
#[serde(default)]
struct SystrayPopup {
    #[serde(flatten)]
    popup: PopupOptions,
    columns: usize,
}

impl Default for SystrayPopup {
    fn default() -> Self {
        SystrayPopup { popup: PopupOptions { alignment: "center".into(), ..PopupOptions::default() }, columns: 5 }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
struct SystrayConfig {
    class_name: String,
    label_collapsed: String,
    label_expanded: String,
    label_position: String,
    icon_size: u32,
    show_unpinned: bool,
    show_unpinned_button: bool,
    tooltip: bool,
    /// extension: exes that are always shown on the bar / always hidden in the popup (case-insensitive)
    pinned: Vec<String>,
    hidden: Vec<String>,
    /// extension: exes dropped entirely (e.g. UniKey when a dedicated unikey widget already exists)
    ignore: Vec<String>,
    /// follows Windows' "show on the taskbar" setting
    use_windows_settings: bool,
    popup: SystrayPopup,
}

impl Default for SystrayConfig {
    fn default() -> Self {
        SystrayConfig {
            class_name: String::new(),
            label_collapsed: "\u{25b2}".into(),
            label_expanded: "\u{25bc}".into(),
            label_position: "left".into(),
            icon_size: 16,
            show_unpinned: false,
            show_unpinned_button: true,
            tooltip: true,
            pinned: Vec::new(),
            hidden: Vec::new(),
            ignore: Vec::new(),
            use_windows_settings: true,
            popup: SystrayPopup::default(),
        }
    }
}

const ID_ARROW: u32 = 1;
const ID_BAR: u32 = 1000;
const ID_POP: u32 = 5000;
const POPUP: u32 = 1;

pub struct Systray {
    base: Base,
    cfg: SystrayConfig,
    shown: Vec<TrayIcon>,
    tucked: Vec<TrayIcon>,
    pix: HashMap<String, (u64, Rc<Pixmap>)>,
    promoted: Vec<String>,
    promoted_at: Option<Instant>,
    expanded: bool,
}

fn lower_names(v: &[String]) -> Vec<String> {
    v.iter().map(|s| s.trim().to_ascii_lowercase()).collect()
}

/// exes that Windows lets show on the taskbar.
fn promoted_exes() -> Vec<String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::*;
    let mut out = Vec::new();
    unsafe {
        let mut root = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from("Control Panel\\NotifyIconSettings"), Some(0), KEY_READ, &mut root).is_err() {
            return out;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(root, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            i += 1;
            let sub = HSTRING::from_wide(&name[..len as usize]);
            let mut promoted = 0u32;
            let mut sz = 4u32;
            if RegGetValueW(root, &sub, &HSTRING::from("IsPromoted"), RRF_RT_REG_DWORD, None, Some(&mut promoted as *mut _ as *mut _), Some(&mut sz)).is_err() || promoted == 0 {
                continue;
            }
            let mut buf = [0u16; 1024];
            let mut bl = (buf.len() * 2) as u32;
            if RegGetValueW(root, &sub, &HSTRING::from("ExecutablePath"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut bl)).is_ok() {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                let p = String::from_utf16_lossy(&buf[..n]);
                out.push(p.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase());
            }
        }
        let _ = RegCloseKey(root);
    }
    out
}

impl Systray {
    pub fn new(name: &str, opts: &Value) -> Systray {
        let mut cfg: SystrayConfig = options(name, opts);
        cfg.pinned = lower_names(&cfg.pinned);
        cfg.hidden = lower_names(&cfg.hidden);
        cfg.ignore = lower_names(&cfg.ignore);
        let base = Base::new(name, &format!("systray {}", cfg.class_name), "", None, ["do_nothing".into(), "do_nothing".into(), "do_nothing".into()]);
        Systray { base, cfg, shown: Vec::new(), tucked: Vec::new(), pix: HashMap::new(), promoted: Vec::new(), promoted_at: None, expanded: false }
    }

    fn refresh(&mut self) {
        if self.promoted_at.is_none_or(|t| t.elapsed().as_secs() >= 5) {
            self.promoted = if self.cfg.use_windows_settings { promoted_exes() } else { Vec::new() };
            self.promoted_at = Some(Instant::now());
        }
        let _ = systray::prune();
        let all = systray::snapshot();
        self.shown.clear();
        self.tucked.clear();
        for ic in all {
            if ic.hidden || ic.image.is_none() {
                continue;
            }
            let exe = ic.exe_name().to_ascii_lowercase();
            if self.cfg.ignore.contains(&exe) {
                continue;
            }
            let on_bar = if self.cfg.hidden.contains(&exe) {
                false
            } else {
                self.cfg.show_unpinned || self.cfg.pinned.contains(&exe) || self.promoted.contains(&exe)
            };
            let key = ic.key();
            let fresh = self.pix.get(&key).is_none_or(|(g, _)| *g != ic.rev);
            if fresh
                && let Some(img) = &ic.image {
                    let p = Pixmap::new(img.0, img.1, img.2.clone());
                    // the image is captured at 2× the icon size; gfx scales it down when drawing
                    self.pix.insert(key, (ic.rev, p));
                }
            if on_bar {
                self.shown.push(ic);
            } else {
                self.tucked.push(ic);
            }
        }
        let live: Vec<String> = self.shown.iter().chain(self.tucked.iter()).map(|i| i.key()).collect();
        self.pix.retain(|k, _| live.contains(k));
    }

    fn icon_node(&self, ic: &TrayIcon, id: u32, size: f32) -> Node {
        let mut n = Node::new("QLabel", "button");
        n.halign = Align::Center;
        n.on_click = Some(id);
        if let Some((_, p)) = self.pix.get(&ic.key()) {
            n.content = Content::Image(Image { pix: p.clone(), w: size, h: size, circle: false });
        }
        if self.cfg.tooltip {
            let t = if ic.tip.trim().is_empty() { ic.exe_name().trim_end_matches(".exe").to_string() } else { ic.tip.clone() };
            n.tooltip = Some(t);
            n.tooltip_delay = 50;
        }
        n
    }

    fn spec(&self) -> PopupSpec {
        PopupSpec { kind: POPUP, class: "systray-popup".into(), opts: self.cfg.popup.popup.clone(), margins: Edges::default(), dir: Dir::Col }
    }

    fn icon_for(&self, id: u32) -> Option<TrayIcon> {
        if id >= ID_POP {
            self.tucked.get((id - ID_POP) as usize).cloned()
        } else if id >= ID_BAR {
            self.shown.get((id - ID_BAR) as usize).cloned()
        } else {
            None
        }
    }
}

impl Widget for Systray {
    fn view(&self, ui: &mut Ui) {
        let size = self.cfg.icon_size as f32;
        self.base.shell(ui, |ui| {
            let arrow = |ui: &mut Ui, me: &Systray| {
                if me.cfg.show_unpinned_button && !me.tucked.is_empty() {
                    let (class, text) = if me.expanded {
                        ("unpinned-visibility-btn expanded", &me.cfg.label_expanded)
                    } else {
                        ("unpinned-visibility-btn", &me.cfg.label_collapsed)
                    };
                    ui.button(class, text.clone(), ID_ARROW);
                }
            };
            if self.cfg.label_position != "right" {
                arrow(ui, self);
            }
            ui.row("pinned-container", |ui| {
                for (i, ic) in self.shown.iter().enumerate() {
                    ui.add(self.icon_node(ic, ID_BAR + i as u32, size));
                }
            });
            if self.cfg.label_position == "right" {
                arrow(ui, self);
            }
        });
    }

    fn popup_view(&self, _kind: u32, ui: &mut Ui) {
        let size = self.cfg.icon_size as f32;
        let cols = self.cfg.popup.columns.max(1);
        ui.col("systray-popup-grid", |ui| {
            for (r, chunk) in self.tucked.chunks(cols).enumerate() {
                ui.layout(Dir::Row, |ui| {
                    for (c, ic) in chunk.iter().enumerate() {
                        ui.add(self.icon_node(ic, ID_POP + (r * cols + c) as u32, size));
                    }
                    if chunk.len() < cols && r > 0 {
                        ui.stretch();
                    }
                });
            }
        });
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init => {
                systray::ensure_started(true, (self.cfg.icon_size as f32 * cx.scale * 2.0).round() as u32);
                self.refresh();
                cx.redraw();
            }
            Ev::Sys(SysEv::Systray) | Ev::Timer => {
                let before: Vec<(String, u64, String)> = self.shown.iter().chain(&self.tucked).map(|i| (i.key(), i.rev, i.tip.clone())).collect();
                self.refresh();
                let after: Vec<(String, u64, String)> = self.shown.iter().chain(&self.tucked).map(|i| (i.key(), i.rev, i.tip.clone())).collect();
                if before != after {
                    cx.redraw();
                }
                if cx.popup_open == Some(POPUP) && self.tucked.is_empty() {
                    cx.close_popup();
                }
            }
            Ev::Click { id: ID_ARROW, .. } => {
                if cx.popup_open == Some(POPUP) {
                    cx.close_popup();
                } else {
                    self.expanded = true;
                    cx.open_popup(self.spec());
                }
                cx.redraw();
            }
            Ev::PopupClosed => {
                self.expanded = false;
                cx.redraw();
            }
            Ev::Click { id, btn } if id >= ID_BAR => {
                let Some(ic) = self.icon_for(id) else { return };
                if id >= ID_POP {
                    cx.close_popup();
                }
                systray::click(
                    &ic,
                    match btn {
                        MouseBtn::Left => Click::Left,
                        MouseBtn::Right => Click::Right,
                        MouseBtn::Middle => Click::Middle,
                    },
                );
            }
            _ => {}
        }
    }

    fn interval(&self) -> u32 {
        // clean up icons of apps that exited without notice
        3000
    }
}
