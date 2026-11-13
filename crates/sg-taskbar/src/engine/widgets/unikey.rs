//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! `UnikeyWidget` (specific to TachyonTaskbar): shows UniKey's typing mode (VI/EN) and switches it on click.
//! The state is read from UniKey's tray icon (tooltip "Click to turn on/off Vietnamese mode", with the icon
//! color as a fallback: red V / green E) and the mode is switched by "clicking" that icon like the real tray,
//! because UniKey ignores simulated Ctrl+Shift key presses.

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks};
use crate::engine::sys::systray::{self, Click, TrayIcon};

#[derive(Deserialize, Clone)]
#[serde(default)]
struct UnikeyConfig {
    label: String,
    label_alt: String,
    class_name: String,
    /// text for `{mode}`
    vietnamese: String,
    english: String,
    off: String,
    /// path to UniKeyNT.exe to launch when it is not running (empty = auto-detect)
    unikey_path: String,
    callbacks: Callbacks,
}

impl Default for UnikeyConfig {
    fn default() -> Self {
        UnikeyConfig {
            label: "{mode}".into(),
            label_alt: "{mode_long}".into(),
            class_name: String::new(),
            vietnamese: "VI".into(),
            english: "EN".into(),
            off: "—".into(),
            unikey_path: String::new(),
            callbacks: Callbacks::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Off,
    Vi,
    En,
}

pub struct Unikey {
    base: Base,
    cfg: UnikeyConfig,
    icon: Option<TrayIcon>,
    mode: Mode,
    last_exe: String,
}

/// Guesses the mode from UniKey's icon: tooltip first, color second.
fn detect(ic: &TrayIcon) -> Mode {
    let tip = ic.tip.to_ascii_lowercase();
    if tip.contains("turn on vietnamese") || tip.contains("english mode") && !tip.contains("turn on english") {
        return Mode::En;
    }
    if tip.contains("turn off vietnamese") || tip.contains("turn on english") || tip.contains("vietnamese mode") {
        return Mode::Vi;
    }
    // dominant color: red V, green E
    if let Some(img) = &ic.image {
        let (mut red, mut blue) = (0u32, 0u32);
        for px in img.2.as_chunks::<4>().0 {
            let (b, g, r, a) = (px[0] as i32, px[1] as i32, px[2] as i32, px[3]);
            if a < 128 {
                continue;
            }
            if r > 140 && r > g + 50 && r > b + 50 {
                red += 1;
            } else if b > 140 && b > r + 50 && b > g + 20 {
                blue += 1;
            }
        }
        if red > blue {
            return Mode::Vi;
        }
        if blue > red {
            return Mode::En;
        }
    }
    Mode::Vi
}

impl Unikey {
    pub fn new(name: &str, opts: &Value) -> Unikey {
        let cfg: UnikeyConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_mode", "toggle_label", "open_menu");
        let base = Base::new(name, &format!("unikey-widget {}", cfg.class_name), &cfg.label, Some(&cfg.label_alt), cb);
        Unikey { base, cfg, icon: None, mode: Mode::Off, last_exe: String::new() }
    }

    fn refresh(&mut self) -> bool {
        let icon = systray::snapshot().into_iter().find(|i| {
            let n = i.exe_name().to_ascii_lowercase();
            n == "unikeynt.exe" || n == "unikey.exe" || n.starts_with("unikey")
        });
        let mode = icon.as_ref().map(detect).unwrap_or(Mode::Off);
        if let Some(i) = &icon {
            self.last_exe = i.exe.clone();
            if !i.tip.is_empty() {
                crate::engine::log::debug(&format!("UniKey tooltip: {:?} → {:?}", i.tip, mode));
            }
        }
        let changed = mode != self.mode || icon.as_ref().map(|i| i.rev) != self.icon.as_ref().map(|i| i.rev);
        self.icon = icon;
        self.mode = mode;
        self.base.tooltip = Some(match mode {
            Mode::Vi => "UniKey: đang gõ tiếng Việt\nBấm để chuyển sang tiếng Anh · chuột phải: menu UniKey".into(),
            Mode::En => "UniKey: đang gõ tiếng Anh\nBấm để chuyển sang tiếng Việt · chuột phải: menu UniKey".into(),
            Mode::Off => "UniKey chưa chạy — bấm để mở".into(),
        });
        changed
    }

    fn launch(&self) {
        let path = if !self.cfg.unikey_path.is_empty() {
            crate::engine::sys::shell::expand_user(&self.cfg.unikey_path)
        } else if !self.last_exe.is_empty() {
            self.last_exe.clone()
        } else {
            // path of UniKey as registered to start with Windows
            run_key_path().unwrap_or_default()
        };
        if !path.is_empty() {
            crate::engine::sys::shell::startfile(&path);
        }
    }
}

fn run_key_path() -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::*;
    unsafe {
        let key = HSTRING::from("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        for name in ["UniKey", "UniKeyNT", "Unikey"] {
            let mut buf = [0u16; 1024];
            let mut len = (buf.len() * 2) as u32;
            if RegGetValueW(HKEY_CURRENT_USER, &key, &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)).is_ok() {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                let s = String::from_utf16_lossy(&buf[..n]);
                let s = s.trim().trim_matches('"');
                let exe = s.split(".exe").next().map(|x| format!("{}.exe", x.trim_matches('"'))).unwrap_or_default();
                if !exe.is_empty() {
                    return Some(exe);
                }
            }
        }
        None
    }
}

impl Widget for Unikey {
    fn view(&self, ui: &mut Ui) {
        let (short, long, class) = match self.mode {
            Mode::Vi => (self.cfg.vietnamese.clone(), "Tiếng Việt".to_string(), "vi"),
            Mode::En => (self.cfg.english.clone(), "English".to_string(), "en"),
            Mode::Off => (self.cfg.off.clone(), "UniKey tắt".to_string(), "off"),
        };
        let vals = [("{mode}", short), ("{mode_long}", long)];
        self.base.shell(ui, |ui| self.base.labels(ui, &|t| fill_placeholders(t, &vals), &|_| class.to_string()));
    }

    fn interval(&self) -> u32 {
        2000
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init => {
                systray::ensure_started(true, (16.0 * cx.scale * 2.0).round() as u32);
                self.refresh();
                cx.redraw();
            }
            Ev::Sys(SysEv::Systray) | Ev::Timer => {
                if self.refresh() {
                    cx.redraw();
                }
            }
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if base_callback(&name, &args) {
                    return;
                }
                match name.as_str() {
                    "toggle_mode" => match &self.icon {
                        Some(ic) => systray::click(ic, Click::Left),
                        None => self.launch(),
                    },
                    "open_menu" => {
                        if let Some(ic) = &self.icon {
                            systray::click(ic, Click::Right);
                        }
                    }
                    "toggle_label" => {
                        self.base.show_alt = !self.base.show_alt;
                        cx.redraw();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
