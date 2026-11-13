//! Configuration of the Wallpaper part (`config/wallpaper.toml`) and its data paths.
//! Fault-tolerant parsing: missing/wrong type → default value (just like TachyonWallpaper's `settings.toml`).

use sg_core::paths::Paths;
use sg_desktop::Monitor;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const MIN_POLL_MS: u32 = 250;
pub const MAX_POLL_MS: u32 = 10_000;
/// `[layout]` key used for the Span / Duplicate modes (one wallpaper for every monitor).
pub const LAYOUT_ALL: &str = "all";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Arrangement {
    /// A separate wallpaper per monitor.
    #[default]
    PerMonitor,
    /// One wallpaper spanning all monitors.
    Span,
    /// The same wallpaper duplicated on every monitor.
    Duplicate,
}

impl Arrangement {
    pub fn parse(s: &str) -> Option<Arrangement> {
        match s {
            "per_monitor" => Some(Arrangement::PerMonitor),
            "span" => Some(Arrangement::Span),
            "duplicate" => Some(Arrangement::Duplicate),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VideoPlayer {
    /// Media Foundation, built into Windows; if the file can't be played, try mpv.
    #[default]
    Auto,
    MediaFoundation,
    Mpv,
}

impl VideoPlayer {
    pub fn parse(s: &str) -> Option<VideoPlayer> {
        match s {
            "auto" => Some(VideoPlayer::Auto),
            "media_foundation" => Some(VideoPlayer::MediaFoundation),
            "mpv" => Some(VideoPlayer::Mpv),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scaling {
    /// Cover the whole screen, cropping the excess (default).
    #[default]
    Fill,
    /// Fit inside, keeping the aspect ratio, with black bars.
    Fit,
    /// Stretch to fill the screen.
    Stretch,
}

impl Scaling {
    pub fn parse(s: &str) -> Option<Scaling> {
        match s {
            "fill" => Some(Scaling::Fill),
            "fit" => Some(Scaling::Fit),
            "stretch" => Some(Scaling::Stretch),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperConfig {
    pub arrangement: Arrangement,
    /// Monitor number (`"1"`, `"2"`…) or `"all"` → wallpaper id in the library.
    pub layout: BTreeMap<String, String>,
    pub scaling: Scaling,
    /// Volume of video/web wallpapers, 0–100 (muted by default).
    pub volume: u8,
    pub video_player: VideoPlayer,
    /// Path to mpv.exe (empty: look next to the exe, in the mpv\ folder, then PATH).
    pub mpv_path: String,
    pub hardware_decoding: bool,
    pub pause_when_covered: bool,
    pub pause_when_app_focused: bool,
    pub pause_per_monitor: bool,
    pub pause_on_battery: bool,
    pub pause_on_battery_saver: bool,
    pub pause_on_remote_desktop: bool,
    /// Process names (e.g. "obs64.exe"): pause while any of them is in the foreground.
    pub pause_apps: Vec<String>,
    pub poll_interval_ms: u32,
    pub trim_memory: bool,
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        WallpaperConfig {
            arrangement: Arrangement::PerMonitor,
            layout: BTreeMap::new(),
            scaling: Scaling::Fill,
            volume: 0,
            video_player: VideoPlayer::Auto,
            mpv_path: String::new(),
            hardware_decoding: true,
            pause_when_covered: true,
            pause_when_app_focused: false,
            pause_per_monitor: true,
            pause_on_battery: true,
            pause_on_battery_saver: true,
            pause_on_remote_desktop: true,
            pause_apps: Vec::new(),
            poll_interval_ms: 750,
            trim_memory: true,
        }
    }
}

fn get_bool(t: &toml::Table, k: &str, d: bool) -> bool {
    t.get(k).and_then(toml::Value::as_bool).unwrap_or(d)
}

fn get_int(t: &toml::Table, k: &str) -> Option<i64> {
    match t.get(k) {
        Some(toml::Value::Integer(n)) => Some(*n),
        Some(toml::Value::Float(f)) if f.is_finite() => Some(f.round() as i64),
        _ => None,
    }
}

impl WallpaperConfig {
    /// Missing/wrong type → default; `volume` clamped to 0–100, `poll_interval_ms` clamped to 250–10000.
    pub fn from_table(t: &toml::Table) -> WallpaperConfig {
        let d = WallpaperConfig::default();
        let layout = t
            .get("layout")
            .and_then(toml::Value::as_table)
            .map(|l| {
                l.iter()
                    .filter_map(|(k, v)| {
                        let id = v.as_str()?.trim();
                        (!id.is_empty()).then(|| (k.trim().to_string(), id.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pause_apps = t
            .get("pause_apps")
            .and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        WallpaperConfig {
            arrangement: t.get("arrangement").and_then(toml::Value::as_str).and_then(Arrangement::parse).unwrap_or(d.arrangement),
            layout,
            scaling: t.get("scaling").and_then(toml::Value::as_str).and_then(Scaling::parse).unwrap_or(d.scaling),
            volume: get_int(t, "volume").map(|n| n.clamp(0, 100) as u8).unwrap_or(d.volume),
            video_player: t.get("video_player").and_then(toml::Value::as_str).and_then(VideoPlayer::parse).unwrap_or(d.video_player),
            mpv_path: t.get("mpv_path").and_then(toml::Value::as_str).map(|s| s.trim().to_string()).unwrap_or_default(),
            hardware_decoding: get_bool(t, "hardware_decoding", d.hardware_decoding),
            pause_when_covered: get_bool(t, "pause_when_covered", d.pause_when_covered),
            pause_when_app_focused: get_bool(t, "pause_when_app_focused", d.pause_when_app_focused),
            pause_per_monitor: get_bool(t, "pause_per_monitor", d.pause_per_monitor),
            pause_on_battery: get_bool(t, "pause_on_battery", d.pause_on_battery),
            pause_on_battery_saver: get_bool(t, "pause_on_battery_saver", d.pause_on_battery_saver),
            pause_on_remote_desktop: get_bool(t, "pause_on_remote_desktop", d.pause_on_remote_desktop),
            pause_apps,
            poll_interval_ms: get_int(t, "poll_interval_ms")
                .map(|n| n.clamp(MIN_POLL_MS as i64, MAX_POLL_MS as i64) as u32)
                .unwrap_or(d.poll_interval_ms),
            trim_memory: get_bool(t, "trim_memory", d.trim_memory),
        }
    }
}

/// All data paths of the Wallpaper part, built from the app's `Paths`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperPaths {
    /// Library: one subfolder per item, containing `TachyonInfo.json`.
    pub library: PathBuf,
    /// The user's Lively customizations: `<id>.json`.
    pub properties: PathBuf,
    /// Thumbnails `<id>.jpg`.
    pub thumbs: PathBuf,
    /// WebView2 data for wallpapers (separate from the app UI's WebView2 folder).
    pub webview: PathBuf,
    /// Copy of the Windows wallpaper from before the first change.
    pub backup_file: PathBuf,
}

impl WallpaperPaths {
    pub fn from_paths(p: &Paths) -> WallpaperPaths {
        WallpaperPaths {
            library: p.local.join("wallpapers"),
            properties: p.local.join("wallpapers-properties"),
            thumbs: p.local.join("wallpapers-thumbs"),
            webview: p.local.join("webview-wallpaper"),
            backup_file: p.backup_dir().join("wallpaper-original.toml"),
        }
    }

    pub fn ensure(&self) {
        for d in [&self.library, &self.properties, &self.thumbs] {
            let _ = std::fs::create_dir_all(d);
        }
    }
}

/// Target of a running wallpaper: a single monitor, or spanning the whole virtual screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Monitor(Monitor),
    Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub kind: TargetKind,
    pub wallpaper: String,
}

impl Target {
    /// Identifying key: the monitor's device name or `"span"`.
    pub fn key(&self) -> String {
        match &self.kind {
            TargetKind::Monitor(m) => m.device.clone(),
            TargetKind::Span => "span".to_string(),
        }
    }
}

/// From the config + the list of real monitors → the jobs to run. Monitors that no longer exist are skipped (no error).
pub fn plan_targets(cfg: &WallpaperConfig, monitors: &[Monitor]) -> Vec<Target> {
    let all_id = || cfg.layout.get(LAYOUT_ALL).or_else(|| cfg.layout.get("1")).cloned();
    match cfg.arrangement {
        Arrangement::PerMonitor => {
            let mut out = Vec::new();
            for (k, id) in &cfg.layout {
                let Ok(n) = k.parse::<u32>() else { continue };
                if let Some(m) = monitors.iter().find(|m| m.number == n) {
                    out.push(Target { kind: TargetKind::Monitor(m.clone()), wallpaper: id.clone() });
                }
            }
            out
        }
        Arrangement::Span => match (all_id(), monitors.is_empty()) {
            (Some(id), false) => vec![Target { kind: TargetKind::Span, wallpaper: id }],
            _ => Vec::new(),
        },
        Arrangement::Duplicate => match all_id() {
            Some(id) => monitors.iter().map(|m| Target { kind: TargetKind::Monitor(m.clone()), wallpaper: id.clone() }).collect(),
            None => Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sg_desktop::Rect;

    fn table(s: &str) -> toml::Table {
        s.parse().unwrap()
    }

    fn mon(number: u32) -> Monitor {
        let r = Rect::new(0, 0, 1920, 1080);
        Monitor { number, device: format!(r"\\.\DISPLAY{number}"), rect: r, work: r, primary: number == 1, dpi: 96, handle: 0 }
    }

    #[test]
    fn config_defaults_match_tachyon() {
        let c = WallpaperConfig::from_table(&toml::Table::new());
        assert_eq!(c, WallpaperConfig::default());
        assert_eq!(c.arrangement, Arrangement::PerMonitor);
        assert_eq!(c.scaling, Scaling::Fill);
        assert_eq!(c.video_player, VideoPlayer::Auto);
        assert_eq!(c.volume, 0);
        assert!(c.hardware_decoding && c.trim_memory);
        assert!(c.pause_when_covered && c.pause_per_monitor && c.pause_on_battery && c.pause_on_battery_saver && c.pause_on_remote_desktop);
        assert!(!c.pause_when_app_focused);
        assert_eq!(c.poll_interval_ms, 750);
        assert!(c.pause_apps.is_empty() && c.layout.is_empty() && c.mpv_path.is_empty());
    }

    #[test]
    fn config_clamps() {
        let c = WallpaperConfig::from_table(&table("volume = 400\npoll_interval_ms = 5"));
        assert_eq!((c.volume, c.poll_interval_ms), (100, 250));
        let c = WallpaperConfig::from_table(&table("volume = -3\npoll_interval_ms = 999999"));
        assert_eq!((c.volume, c.poll_interval_ms), (0, 10_000));
        let c = WallpaperConfig::from_table(&table("volume = \"loud\"\narrangement = 4\nscaling = \"zoom\"\npause_apps = [\" obs64.exe \", 5, \"\"]"));
        assert_eq!(c.volume, 0);
        assert_eq!(c.arrangement, Arrangement::PerMonitor);
        assert_eq!(c.scaling, Scaling::Fill);
        assert_eq!(c.pause_apps, vec!["obs64.exe".to_string()]);
        let c = WallpaperConfig::from_table(&table("arrangement = \"span\"\nvideo_player = \"mpv\"\nscaling = \"fit\"\n[layout]\n\"1\" = \"a\"\n\"2\" = 5\n\"3\" = \" \""));
        assert_eq!((c.arrangement, c.video_player, c.scaling), (Arrangement::Span, VideoPlayer::Mpv, Scaling::Fit));
        assert_eq!(c.layout.len(), 1, "id sai kiểu hoặc rỗng bị bỏ");
    }

    #[test]
    fn layout_ignores_missing_monitor() {
        let two = vec![mon(1), mon(2)];
        let one = vec![mon(1)];
        let per = WallpaperConfig::from_table(&table("[layout]\n\"1\" = \"a\"\n\"2\" = \"b\"\n\"9\" = \"z\"\nall = \"q\""));
        let t = plan_targets(&per, &two);
        assert_eq!(t.iter().map(|t| t.wallpaper.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        let t = plan_targets(&per, &one);
        assert_eq!(t.len(), 1, "màn hình 2 đã rút → bỏ qua, không lỗi");
        assert_eq!(t[0].key(), r"\\.\DISPLAY1");
        assert!(plan_targets(&per, &[]).is_empty());

        let span = WallpaperConfig::from_table(&table("arrangement = \"span\"\n[layout]\nall = \"s\""));
        assert_eq!(plan_targets(&span, &two), vec![Target { kind: TargetKind::Span, wallpaper: "s".into() }]);
        assert!(plan_targets(&span, &[]).is_empty());

        let dup = WallpaperConfig::from_table(&table("arrangement = \"duplicate\"\n[layout]\n\"1\" = \"d\""));
        assert_eq!(plan_targets(&dup, &two).len(), 2, "không có khoá all thì lấy hình nền của màn hình 1");
        assert_eq!(plan_targets(&dup, &one).len(), 1);
        assert!(plan_targets(&WallpaperConfig::default(), &two).is_empty());
    }
}
