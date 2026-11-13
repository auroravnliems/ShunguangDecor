//! Tauri commands for the live-desktop preview frame, and images served via `sg://capture/<monitor>.jpg`
//! (the whole desktop) or `sg://capture/<monitor>-icons.png` (icon layer only, transparent background).

use crate::commands::protocol_base;
use crate::modules::wallpaper::GIF_MAX_WHOLE;
use crate::state::AppState;
use serde::Serialize;
use sg_capture::ViewMode;
use sg_wallpaper::library::{self, Wallpaper, WallpaperType};
use sg_wallpaper::{Arrangement, RunningView, Scaling, WallpaperService};
use std::collections::BTreeMap;
use std::sync::Mutex;
use tauri::State;

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MonitorView {
    pub number: u32,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

#[tauri::command]
pub fn list_monitors() -> Vec<MonitorView> {
    sg_desktop::monitors()
        .into_iter()
        .map(|m| MonitorView { number: m.number, width: m.rect.width(), height: m.rect.height(), primary: m.primary })
        .collect()
}

#[tauri::command]
pub fn capture_start(state: State<'_, AppState>, viewer: u64, monitor: u32, seq: u64, mode: Option<String>) {
    // `mode`: "icons" = icon layer only (PNG with alpha); missing/unknown = "full" as before.
    let mode = mode.as_deref().map_or(ViewMode::Full, ViewMode::parse);
    state.services.capture.start_seq(viewer, monitor, mode, seq);
}

#[tauri::command]
pub fn capture_stop(state: State<'_, AppState>, viewer: u64, seq: u64) {
    state.services.capture.stop_seq(viewer, seq);
}

fn parse_monitor(stem: &str) -> Option<u32> {
    if stem.is_empty() || stem.len() > 3 || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    stem.parse().ok()
}

/// `/capture/<monitor>.jpg` → monitor number; an unknown path → `None`.
pub fn parse_route(path: &str) -> Option<u32> {
    parse_monitor(path.strip_prefix("/capture/")?.strip_suffix(".jpg")?)
}

/// `/capture/<monitor>-icons.png` → monitor number (icon-only frame); unknown → `None`.
pub fn parse_icons_route(path: &str) -> Option<u32> {
    parse_monitor(path.strip_prefix("/capture/")?.strip_suffix("-icons.png")?)
}

/// The latest frame: (bytes, content-type). `.jpg` = whole desktop, `-icons.png` = icon layer; not available yet → `None` (404).
pub fn serve(state: &AppState, path: &str) -> Option<(Vec<u8>, &'static str)> {
    if let Some(monitor) = parse_icons_route(path) {
        return state.services.capture.latest_icons(monitor).map(|(_, png)| (png.as_ref().clone(), "image/png"));
    }
    let monitor = parse_route(path)?;
    state.services.capture.latest(monitor).map(|(_, jpeg)| (jpeg.as_ref().clone(), "image/jpeg"))
}

/// A video/GIF wallpaper running on a monitor, so the preview frame can play that very file on a lower layer.
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LiveSource {
    /// `video` or `gif`.
    pub kind: String,
    /// `sg://wallpaper/media/<id>` (with `?v=` based on the file's modified time so the WebView doesn't keep a stale copy).
    pub url: String,
    /// The scaling mode from the wallpaper config: `fill` | `fit` | `stretch`.
    pub scaling: String,
    /// The matching CSS `object-fit` value: `cover` | `contain` | `fill`.
    pub object_fit: String,
    /// Currently paused (the user pressed pause, or the Wallpaper part's auto-pause rules).
    pub paused: bool,
}

fn scaling_name(s: Scaling) -> (&'static str, &'static str) {
    match s {
        Scaling::Fill => ("fill", "cover"),
        Scaling::Fit => ("fit", "contain"),
        Scaling::Stretch => ("stretch", "fill"),
    }
}

fn file_mtime(w: &Wallpaper) -> u64 {
    std::fs::metadata(w.source())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// The live playback source for `monitor`: only when the arrangement is per-monitor, the wallpaper running on that monitor is
/// a Video/GIF, and the file still exists. Kept separate from `AppState` so it can be tested with fake data.
pub fn live_source_for(
    arrangement: Arrangement,
    scaling: Scaling,
    manual_pause: bool,
    running: &[RunningView],
    monitor: u32,
    find: impl Fn(&str) -> Option<Wallpaper>,
) -> Option<LiveSource> {
    // Span/duplicate: one wallpaper covers several monitors and a small frame can't reproduce it correctly -> keep the capture path.
    if arrangement != Arrangement::PerMonitor {
        return None;
    }
    let r = running.iter().find(|r| r.monitor == monitor)?;
    let w = find(&r.wallpaper_id)?;
    let kind = match w.kind() {
        WallpaperType::Video => "video",
        WallpaperType::Gif => "gif",
        _ => return None,
    };
    if !w.source_exists() || !crate::modules::wallpaper::media_ext_ok(std::path::Path::new(&w.source())) {
        return None;
    }
    // The GIF is too large to return as a whole file (the media route returns 413): keep the capture path.
    if w.kind() == WallpaperType::Gif && std::fs::metadata(w.source()).is_ok_and(|m| m.len() > GIF_MAX_WHOLE) {
        return None;
    }
    let (scaling, object_fit) = scaling_name(scaling);
    Some(LiveSource {
        kind: kind.into(),
        url: format!("{}/wallpaper/media/{}?v={}", protocol_base(), w.id, file_mtime(&w)),
        scaling: scaling.into(),
        object_fit: object_fit.into(),
        paused: manual_pause || r.paused,
    })
}

/// `None` = use the capture path (`full` mode) as before. The UI calls this again whenever it receives a `wallpaper-state` event
/// (wallpaper changed, paused/resumed) or a `module-config-changed` event of the Wallpaper part (scaling/arrangement changed).
// `async`: runs on Tauri's background thread, so JSON/metadata is not read on the UI thread.
#[tauri::command(async)]
pub fn capture_live_source(state: State<'_, AppState>, monitor: u32) -> Option<LiveSource> {
    let svc = &state.services.wallpaper;
    let st = svc.state();
    let cfg = svc.config();
    let paths = svc.paths();
    live_source_for(cfg.arrangement, cfg.scaling, st.manual_pause, &st.running, monitor, |id| library::find(paths, id))
}

/// Whether the wallpaper on `monitor` is a dynamic kind that can only be captured (web, stream, app) and is running: in that case
/// the preview frame captures every 100 ms. Video/GIF already have a live playback layer; still images don't need to be fast.
pub fn is_dynamic_capture(svc: &WallpaperService, monitor: u32) -> bool {
    let st = svc.state();
    if st.manual_pause {
        return false;
    }
    let Some(r) = st.running.iter().find(|r| r.monitor == monitor || r.monitor == 0) else { return false };
    if r.paused || r.backend == "windows-wallpaper" {
        return false;
    }
    // 100 ms cycle: don't read the library JSON every time; remember the kind by the running wallpaper's id (one id per monitor).
    let mut cache = KIND_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dynamic) = cache.get(&r.wallpaper_id) {
        return *dynamic;
    }
    let dynamic = library::find(svc.paths(), &r.wallpaper_id)
        .is_some_and(|w| !matches!(w.kind(), WallpaperType::Video | WallpaperType::Gif | WallpaperType::Picture));
    if cache.len() >= 32 {
        cache.clear();
    }
    cache.insert(r.wallpaper_id.clone(), dynamic);
    dynamic
}

/// Cache of "is this wallpaper a dynamic kind that can only be captured", keyed by id.
static KIND_CACHE: Mutex<BTreeMap<String, bool>> = Mutex::new(BTreeMap::new());

/// Clears the kind cache when the wallpaper library changes (an id can change kind).
pub fn clear_kind_cache() {
    KIND_CACHE.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_route_parsing() {
        assert_eq!(parse_route("/capture/1.jpg"), Some(1));
        assert_eq!(parse_route("/capture/12.jpg"), Some(12));
        assert_eq!(parse_route("/capture/../1.jpg"), None);
        assert_eq!(parse_route("/capture/a.jpg"), None);
        assert_eq!(parse_route("/capture/1.png"), None);
        assert_eq!(parse_route("/capture/.jpg"), None);
        assert_eq!(parse_route("/capture/1/2.jpg"), None);
        assert_eq!(parse_route("/cursor/1.jpg"), None);
        assert_eq!(parse_route("/capture/1-icons.png"), None);
    }

    #[test]
    fn icons_route_parsing() {
        assert_eq!(parse_icons_route("/capture/1-icons.png"), Some(1));
        assert_eq!(parse_icons_route("/capture/12-icons.png"), Some(12));
        assert_eq!(parse_icons_route("/capture/../1-icons.png"), None);
        assert_eq!(parse_icons_route("/capture/a-icons.png"), None);
        assert_eq!(parse_icons_route("/capture/1.png"), None);
        assert_eq!(parse_icons_route("/capture/-icons.png"), None);
        assert_eq!(parse_icons_route("/capture/1.jpg"), None);
    }

    fn make_wallpaper(dir: &std::path::Path, id: &str, ty: i64, file: &str, write_file: bool) -> Wallpaper {
        let d = dir.join(id);
        std::fs::create_dir_all(&d).unwrap();
        if write_file {
            std::fs::write(d.join(file), b"x").unwrap();
        }
        let info = format!(r#"{{"Type":{ty},"FileName":"{file}"}}"#);
        std::fs::write(d.join("LivelyInfo.json"), info).unwrap();
        library::load_one(&d).unwrap()
    }

    fn rv(monitor: u32, id: &str, paused: bool) -> RunningView {
        RunningView { monitor, wallpaper_id: id.into(), backend: "mpv".into(), paused }
    }

    #[test]
    fn live_source_only_for_video_gif() {
        let tmp = std::env::temp_dir().join(format!("sg-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // Type indexes: 1 web, 7 video, 8 gif, 11 picture.
        let all = [
            make_wallpaper(&tmp, "vid", 7, "a.mp4", true),
            make_wallpaper(&tmp, "gif", 8, "a.gif", true),
            make_wallpaper(&tmp, "web", 1, "index.html", true),
            make_wallpaper(&tmp, "pic", 11, "a.png", true),
            make_wallpaper(&tmp, "gone", 7, "missing.mp4", false),
        ];
        let find = |id: &str| all.iter().find(|w| w.id == id).cloned();
        let go = |arr, sc, mp, running: &[RunningView], m| live_source_for(arr, sc, mp, running, m, find);
        let pm = Arrangement::PerMonitor;

        let v = go(pm, Scaling::Fill, false, &[rv(1, "vid", false)], 1).unwrap();
        assert_eq!((v.kind.as_str(), v.scaling.as_str(), v.object_fit.as_str(), v.paused), ("video", "fill", "cover", false));
        assert!(v.url.contains("/wallpaper/media/vid?v="), "{}", v.url);
        let g = go(pm, Scaling::Fit, false, &[rv(2, "gif", false)], 2).unwrap();
        assert_eq!((g.kind.as_str(), g.object_fit.as_str()), ("gif", "contain"));
        assert_eq!(go(pm, Scaling::Stretch, false, &[rv(1, "vid", false)], 1).unwrap().object_fit, "fill");
        // Not a video/GIF, or the file is missing -> no playback source.
        assert!(go(pm, Scaling::Fill, false, &[rv(1, "web", false)], 1).is_none());
        assert!(go(pm, Scaling::Fill, false, &[rv(1, "pic", false)], 1).is_none());
        assert!(go(pm, Scaling::Fill, false, &[rv(1, "gone", false)], 1).is_none());
        assert!(go(pm, Scaling::Fill, false, &[rv(1, "khong-co", false)], 1).is_none());
        // Another monitor with nothing running.
        assert!(go(pm, Scaling::Fill, false, &[rv(1, "vid", false)], 2).is_none());
        // Only for the per-monitor arrangement.
        assert!(go(Arrangement::Span, Scaling::Fill, false, &[rv(0, "vid", false)], 1).is_none());
        assert!(go(Arrangement::Duplicate, Scaling::Fill, false, &[rv(1, "vid", false)], 1).is_none());
        // Paused: the user pressed pause, or the auto-pause rules.
        assert!(go(pm, Scaling::Fill, true, &[rv(1, "vid", false)], 1).unwrap().paused);
        assert!(go(pm, Scaling::Fill, false, &[rv(1, "vid", true)], 1).unwrap().paused);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
