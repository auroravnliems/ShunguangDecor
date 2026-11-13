//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! `yasb.wifi.WifiWidget`: network status. Wi-Fi uses `wifi_icons` by signal strength, wired connections use
//! `ethernet_label`/`ethernet_icon` as in YASB. Adds the placeholders `{download_speed}`, `{upload_speed}`,
//! `{link_speed}` and the class `wifi|ethernet|offline` on the label so the stylesheet can recolor it by state.

use serde::Deserialize;
use serde_yaml::Value;

use super::*;
use crate::engine::config::{options, Callbacks};
use crate::engine::sys::stats::{self, Net, NetKind};

#[derive(Deserialize, Clone)]
#[serde(default)]
struct WifiConfig {
    label: String,
    label_alt: String,
    class_name: String,
    update_interval: u32,
    wifi_icons: Vec<String>,
    ethernet_label: String,
    ethernet_label_alt: String,
    ethernet_icon: String,
    /// extension: label shown when offline (defaults to `label`)
    offline_label: Option<String>,
    offline_icon: String,
    get_exact_wifi_strength: bool,
    hide_if_ethernet: bool,
    callbacks: Callbacks,
}

impl Default for WifiConfig {
    fn default() -> Self {
        WifiConfig {
            label: "{wifi_icon}".into(),
            label_alt: "{wifi_icon} {wifi_name}".into(),
            class_name: String::new(),
            update_interval: 1000,
            wifi_icons: ["\u{f092e}", "\u{f091f}", "\u{f0922}", "\u{f0925}", "\u{f0928}"].iter().map(|s| s.to_string()).collect(),
            ethernet_label: "{wifi_icon}".into(),
            ethernet_label_alt: "{wifi_icon} {ip_addr}".into(),
            ethernet_icon: "\u{eba9}".into(),
            offline_label: None,
            offline_icon: String::new(),
            get_exact_wifi_strength: false,
            hide_if_ethernet: false,
            callbacks: Callbacks::default(),
        }
    }
}

pub struct Wifi {
    base: Base,
    cfg: WifiConfig,
    eth: Base,
    offline: Option<Base>,
    net: Net,
}

impl Wifi {
    pub fn new(name: &str, opts: &Value) -> Wifi {
        let cfg: WifiConfig = options(name, opts);
        let cb = cfg.callbacks.resolve("toggle_label", "do_nothing", "do_nothing");
        let class = format!("wifi-widget {}", cfg.class_name);
        let base = Base::new(name, &class, &cfg.label, Some(&cfg.label_alt), cb.clone());
        let eth = Base::new(name, &class, &cfg.ethernet_label, Some(&cfg.ethernet_label_alt), cb.clone());
        let offline = cfg.offline_label.as_ref().map(|l| Base::new(name, &class, l, Some(l), cb));
        Wifi { base, cfg, eth, offline, net: Net::default() }
    }

    fn icon(&self) -> String {
        match self.net.kind {
            NetKind::Ethernet | NetKind::Other => self.cfg.ethernet_icon.clone(),
            NetKind::None => {
                if self.cfg.offline_icon.is_empty() {
                    self.cfg.wifi_icons.first().cloned().unwrap_or_default()
                } else {
                    self.cfg.offline_icon.clone()
                }
            }
            NetKind::Wifi => {
                let icons = &self.cfg.wifi_icons;
                if icons.is_empty() {
                    return String::new();
                }
                let q = self.net.quality as usize;
                // like YASB: 4 levels in 25% steps
                let i = if q == 0 { 0 } else { (q.min(99) / 25 + 1).min(icons.len() - 1) };
                icons[i].clone()
            }
        }
    }

    fn state_class(&self) -> &'static str {
        match self.net.kind {
            NetKind::None => "offline",
            NetKind::Wifi => "wifi",
            _ => "ethernet",
        }
    }
}

impl Widget for Wifi {
    fn view(&self, ui: &mut Ui) {
        let n = &self.net;
        let name = match n.kind {
            NetKind::None => "Không có mạng".to_string(),
            _ => n.name.clone(),
        };
        let vals = [
            ("{wifi_icon}", self.icon()),
            ("{wifi_name}", name),
            ("{wifi_strength}", n.quality.to_string()),
            ("{ip_addr}", if n.ip.is_empty() { "—".into() } else { n.ip.clone() }),
            ("{download_speed}", stats::speed(n.down)),
            ("{upload_speed}", stats::speed(n.up)),
            ("{link_speed}", if n.link_mbps >= 1000 { format!("{} Gbps", n.link_mbps / 1000) } else { format!("{} Mbps", n.link_mbps) }),
        ];
        let b = match n.kind {
            NetKind::None => self.offline.as_ref().unwrap_or(&self.base),
            NetKind::Wifi => &self.base,
            _ => &self.eth,
        };
        let state = self.state_class();
        b.shell(ui, |ui| b.labels(ui, &|t| fill_placeholders(t, &vals), &|_| state.to_string()));
    }

    fn interval(&self) -> u32 {
        self.cfg.update_interval.max(500)
    }

    fn event(&mut self, ev: Ev, cx: &mut Cx) {
        match ev {
            Ev::Init | Ev::Timer => {
                let old = (self.net.kind, self.net.name.clone(), self.net.quality, self.net.ip.clone());
                self.net = stats::network();
                let n = &self.net;
                let tip = match n.kind {
                    NetKind::None => "Không có kết nối mạng".to_string(),
                    NetKind::Wifi => format!("Wi-Fi: {}  ·  sóng {}%\nIP {}\n↓ {}   ↑ {}", n.name, n.quality, n.ip, stats::speed(n.down), stats::speed(n.up)),
                    _ => format!("Mạng dây: {}  ·  {} Mbps\nIP {}\n↓ {}   ↑ {}", n.name, n.link_mbps, n.ip, stats::speed(n.down), stats::speed(n.up)),
                };
                let hide = self.cfg.hide_if_ethernet && matches!(n.kind, NetKind::Ethernet | NetKind::Other);
                for b in [&mut self.base, &mut self.eth].into_iter().chain(self.offline.as_mut()) {
                    b.tooltip = Some(tip.clone());
                    b.hidden = hide;
                }
                let new = (self.net.kind, self.net.name.clone(), self.net.quality, self.net.ip.clone());
                let uses_speed = [&self.cfg.label, &self.cfg.label_alt, &self.cfg.ethernet_label, &self.cfg.ethernet_label_alt]
                    .iter()
                    .any(|l| l.contains("_speed}"));
                if old != new || uses_speed || matches!(ev, Ev::Init) {
                    cx.redraw();
                }
            }
            Ev::Click { id: BASE, btn } => {
                let (name, args) = split_callback(self.base.callback_for(btn));
                if base_callback(&name, &args) {
                    return;
                }
                match name.as_str() {
                    "toggle_label" => {
                        let alt = !self.base.show_alt;
                        for b in [&mut self.base, &mut self.eth].into_iter().chain(self.offline.as_mut()) {
                            b.show_alt = alt;
                        }
                        cx.redraw();
                    }
                    "open_settings" | "open_network_settings" => crate::engine::sys::shell::startfile("ms-settings:network"),
                    "open_quick_settings" => crate::engine::sys::shell::send_combo(
                        &[windows::Win32::UI::Input::KeyboardAndMouse::VK_LWIN],
                        Some(windows::Win32::UI::Input::KeyboardAndMouse::VK_A),
                    ),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
