//! Tauri commands of the Wallpaper part, and thumbnails via `sg://wallpaper/thumb/<id>.jpg`.

use crate::commands::protocol_base;
use crate::state::AppState;
use serde::Serialize;
use serde_json::{Value, json};
use sg_core::config::write_atomic;
use sg_core::ids::ModuleId;
use sg_core::text::I18nText;
use sg_wallpaper::config::WallpaperPaths;
use sg_wallpaper::library::{self, Wallpaper, WallpaperType};
use sg_wallpaper::{EngineState, properties, thumbs};
use std::path::PathBuf;
use tauri::State;

type Res<T> = Result<T, I18nText>;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperItem {
    pub id: String,
    pub title: String,
    /// `video|gif|picture|web|url|videostream|app` (and a few rare Lively types).
    pub kind: String,
    pub source: String,
    pub source_exists: bool,
    pub has_properties: bool,
    pub thumb_url: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PropertiesView {
    /// `LivelyProperties.json` merged with the user's values; `folderDropdown` also carries `items`.
    pub controls: Value,
    /// Current values only: name → value.
    pub values: Value,
}

fn io_err(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("wallpaper.err.io").arg("detail", e)
}

fn str_err(e: String) -> I18nText {
    I18nText::new("wallpaper.err.io").arg("detail", e)
}

fn join_err(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("wallpaper.err.io").arg("detail", e)
}

fn mtime(p: &std::path::Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The form of a wallpaper sent to the UI (creates the thumbnail if it does not exist).
pub fn item_of(paths: &WallpaperPaths, w: &Wallpaper) -> WallpaperItem {
    let thumb_url = thumbs::ensure_thumb(paths, w)
        .map(|p| format!("{}/wallpaper/thumb/{}.jpg?v={}", protocol_base(), w.id, mtime(&p)));
    WallpaperItem {
        id: w.id.clone(),
        title: w.title(),
        kind: w.kind().as_str().to_string(),
        source: w.source(),
        source_exists: w.source_exists(),
        has_properties: w.properties_template().is_some(),
        thumb_url,
    }
}

fn library_changed(state: &AppState) {
    // The library changed: drop the preview frame's cached wallpaper types.
    crate::modules::capture::clear_kind_cache();
    state.services.wallpaper.ctx().emit("wallpaper-library-changed", Value::Null);
}

#[tauri::command]
pub async fn wallpaper_library(state: State<'_, AppState>) -> Res<Vec<WallpaperItem>> {
    let svc = state.services.wallpaper.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let paths = svc.paths().clone();
        library::load_all(&paths).iter().map(|w| item_of(&paths, w)).collect()
    })
    .await
    .map_err(join_err)
}

/// Adds a file or URL (a Lively folder/zip is also accepted: the last entry is taken).
#[tauri::command]
pub async fn wallpaper_add(state: State<'_, AppState>, source: String, kind: Option<String>) -> Res<WallpaperItem> {
    let forced = match kind.as_deref() {
        None | Some("") => None,
        Some(k) => Some(WallpaperType::from_name(k).ok_or_else(|| I18nText::new("wallpaper.err.kind_unknown").arg("kind", k))?),
    };
    let svc = state.services.wallpaper.clone();
    let item = tauri::async_runtime::spawn_blocking(move || {
        let paths = svc.paths().clone();
        library::add(&paths, &source, forced).map(|w| item_of(&paths, &w))
    })
    .await
    .map_err(join_err)?
    .map_err(str_err)?;
    library_changed(&state);
    Ok(item)
}

/// Imports Lively wallpapers: a folder (with `LivelyInfo.json`, or containing several such folders) or a `.zip`.
#[tauri::command]
pub async fn wallpaper_import(state: State<'_, AppState>, path: PathBuf) -> Res<Vec<WallpaperItem>> {
    let svc = state.services.wallpaper.clone();
    let items = tauri::async_runtime::spawn_blocking(move || {
        let paths = svc.paths().clone();
        let found = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            library::import_zip(&paths, &path).map(|w| vec![w])
        } else {
            library::import_folder(&paths, &path)
        };
        found.map(|v| v.iter().map(|w| item_of(&paths, w)).collect::<Vec<_>>())
    })
    .await
    .map_err(join_err)?
    .map_err(str_err)?;
    library_changed(&state);
    Ok(items)
}

/// Removes the id from the saved configuration's `[layout]`; returns `true` if anything changed.
fn remove_from_layout(cfg: &mut toml::Table, id: &str) -> bool {
    let Some(layout) = cfg.get_mut("layout").and_then(toml::Value::as_table_mut) else { return false };
    let before = layout.len();
    layout.retain(|_, v| v.as_str().map(str::trim) != Some(id));
    layout.len() != before
}

#[tauri::command]
pub async fn wallpaper_delete(app: tauri::AppHandle, state: State<'_, AppState>, id: String) -> Res<()> {
    // In use → remove it from the layout first (the engine closes that player), then delete.
    let mut cfg = state.module_config(ModuleId::Wallpaper);
    if remove_from_layout(&mut cfg, &id) {
        let text = toml::to_string_pretty(&cfg).map_err(io_err)?;
        write_atomic(&state.paths.module_config_file(ModuleId::Wallpaper), text.as_bytes()).map_err(io_err)?;
        if state.host.info(ModuleId::Wallpaper).enabled {
            state.host.apply(ModuleId::Wallpaper, &cfg).map_err(|e| e.to_text())?;
        }
        crate::commands::config_changed(&app, ModuleId::Wallpaper);
    }
    let svc = state.services.wallpaper.clone();
    tauri::async_runtime::spawn_blocking(move || library::delete(svc.paths(), &id)).await.map_err(join_err)?.map_err(str_err)?;
    library_changed(&state);
    Ok(())
}

#[tauri::command]
pub fn wallpaper_rename(state: State<'_, AppState>, id: String, title: String) -> Res<()> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(I18nText::new("wallpaper.err.bad_title"));
    }
    library::rename(state.services.wallpaper.paths(), &id, title).map_err(str_err)?;
    library_changed(&state);
    Ok(())
}

#[tauri::command]
pub fn wallpaper_state(state: State<'_, AppState>) -> EngineState {
    state.services.wallpaper.state()
}

#[tauri::command]
pub async fn wallpaper_toggle_pause(state: State<'_, AppState>) -> Res<bool> {
    let svc = state.services.wallpaper.clone();
    let paused = tauri::async_runtime::spawn_blocking(move || svc.toggle_pause()).await.map_err(join_err)??;
    if let Some(tray) = state.tray() {
        tray.set_wallpaper_paused(paused, state.lang());
    }
    Ok(paused)
}

/// Builds the `PropertiesView` from the merged set of properties.
pub fn properties_view(paths: &WallpaperPaths, w: &Wallpaper) -> Option<PropertiesView> {
    let mut props = properties::load(paths, w)?;
    let mut values = serde_json::Map::new();
    for (name, c) in props.iter_mut() {
        if c.get("type").and_then(Value::as_str) == Some("folderDropdown") {
            let items = properties::folder_items(w, c);
            if let Value::Object(o) = c {
                o.insert("items".into(), json!(items));
            }
        }
        if let Some(v) = c.get("value") {
            values.insert(name.clone(), v.clone());
        }
    }
    Some(PropertiesView { controls: Value::Object(props), values: Value::Object(values) })
}

#[tauri::command]
pub fn wallpaper_properties(state: State<'_, AppState>, id: String) -> Option<PropertiesView> {
    let paths = state.services.wallpaper.paths();
    let w = library::find(paths, &id)?;
    properties_view(paths, &w)
}

#[tauri::command]
pub async fn wallpaper_set_property(state: State<'_, AppState>, id: String, name: String, value: Value) -> Res<()> {
    let svc = state.services.wallpaper.clone();
    tauri::async_runtime::spawn_blocking(move || match svc.engine() {
        Some(engine) => engine.set_property(&id, &name, value),
        // The part is off: only save the value, it will be used the next time it is enabled.
        None => {
            let w = library::find(svc.paths(), &id).ok_or("wallpaper not found")?;
            properties::set_value(svc.paths(), &w, &name, value).map(|_| ())
        }
    })
    .await
    .map_err(join_err)?
    .map_err(str_err)
}

#[tauri::command]
pub async fn wallpaper_reset_properties(state: State<'_, AppState>, id: String) -> Res<()> {
    let svc = state.services.wallpaper.clone();
    tauri::async_runtime::spawn_blocking(move || match svc.engine() {
        Some(engine) => engine.reset_properties(&id),
        None => {
            properties::reset(svc.paths(), &id);
            Ok(())
        }
    })
    .await
    .map_err(join_err)?
    .map_err(str_err)
}

#[tauri::command]
pub fn wallpaper_find_mpv(state: State<'_, AppState>) -> Option<String> {
    let cfg = sg_wallpaper::WallpaperConfig::from_table(&state.module_config(ModuleId::Wallpaper));
    sg_wallpaper::find_mpv(&cfg).map(|p| p.to_string_lossy().into_owned())
}

/// Only for manual testing (debug builds): simulates an Explorer restart without killing the real Explorer.
#[tauri::command]
pub fn wallpaper_debug_taskbar_created(state: State<'_, AppState>) {
    if cfg!(debug_assertions)
        && let Some(e) = state.services.wallpaper.engine()
    {
        e.simulate_taskbar_created();
    }
}

/// `/wallpaper/thumb/<id>.jpg` → a valid id; anything unknown → `None`.
pub fn parse_thumb_route(path: &str) -> Option<String> {
    let id = path.strip_prefix("/wallpaper/thumb/")?.strip_suffix(".jpg")?;
    library::valid_id(id).then(|| id.to_string())
}

/// The thumbnail JPEG; missing → `None` (404).
pub fn serve_thumb(state: &AppState, path: &str) -> Option<Vec<u8>> {
    let id = parse_thumb_route(path)?;
    let paths = state.services.wallpaper.paths();
    std::fs::read(thumbs::thumb_path(paths, &id)).ok()
}

/// Each `Range` response is at most 4 MiB: an open-ended range is truncated and the browser asks for the rest itself.
pub const MEDIA_MAX_RANGE: u64 = 4 * 1024 * 1024;

/// `/wallpaper/media/<id>` → a valid id; anything unknown (containing `..`, a `/`, or an extra suffix) → `None`.
pub fn parse_media_route(path: &str) -> Option<String> {
    let id = path.strip_prefix("/wallpaper/media/")?;
    library::valid_id(id).then(|| id.to_string())
}

/// The result of parsing the `Range` header for a file `len` bytes long.
#[derive(Debug, PartialEq, Eq)]
pub enum RangeSpec {
    /// No `Range`: return the whole file (200).
    Whole,
    /// A closed range `[start, end]`, clamped to the file and to `MEDIA_MAX_RANGE` (206).
    Part(u64, u64),
    /// Bad syntax / outside the file size (416).
    Unsatisfiable,
    /// A GIF without `Range` that is larger than `GIF_MAX_WHOLE` (413).
    TooLarge,
}

/// A GIF without `Range` is served whole (`<img>` does not request ranges), but only up to this size.
pub const GIF_MAX_WHOLE: u64 = 256 * 1024 * 1024;

/// Decides how to serve based on the kind (video/GIF), the `Range` header and the file size. A video without `Range` is treated as
/// `bytes=0-` (206, at most 4 MiB) so a whole video is never read into RAM. A pure function so tests need no large file.
pub fn plan_media(is_video: bool, header: Option<&str>, len: u64, gif_limit: u64) -> RangeSpec {
    match (header, is_video) {
        (None, true) => parse_range(Some("bytes=0-"), len),
        (None, false) if len > gif_limit => RangeSpec::TooLarge,
        (h, _) => parse_range(h, len),
    }
}

/// `bytes=a-b`, `bytes=a-` (open-ended) and `bytes=-n` (suffix). Multiple ranges or bad syntax → 416
/// (browsers ask for a single range; better to refuse than to read a whole large file).
pub fn parse_range(header: Option<&str>, len: u64) -> RangeSpec {
    let Some(h) = header else { return RangeSpec::Whole };
    let Some(spec) = h.trim().strip_prefix("bytes=") else { return RangeSpec::Unsatisfiable };
    if len == 0 || spec.contains(',') {
        return RangeSpec::Unsatisfiable;
    }
    let Some((a, b)) = spec.split_once('-') else { return RangeSpec::Unsatisfiable };
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = if a.is_empty() {
        // Suffix: the last n bytes.
        let Ok(n) = b.parse::<u64>() else { return RangeSpec::Unsatisfiable };
        if n == 0 {
            return RangeSpec::Unsatisfiable;
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let Ok(start) = a.parse::<u64>() else { return RangeSpec::Unsatisfiable };
        let end = if b.is_empty() {
            len - 1
        } else {
            match b.parse::<u64>() {
                Ok(e) => e.min(len - 1),
                Err(_) => return RangeSpec::Unsatisfiable,
            }
        };
        (start, end)
    };
    if start >= len || end < start {
        return RangeSpec::Unsatisfiable;
    }
    RangeSpec::Part(start, end.min(start.saturating_add(MEDIA_MAX_RANGE - 1)))
}

/// Content-Type by media file extension.
pub fn media_content_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("ogv") => "video/ogg",
        Some("avi") => "video/x-msvideo",
        Some("wmv") => "video/x-ms-wmv",
        Some("gif") => "image/gif",
        _ => "application/octet-stream",
    }
}

/// The media route's response.
#[derive(Debug)]
pub enum MediaReply {
    NotFound,
    /// 413: the GIF is too large to return whole.
    TooLarge,
    /// 416: carries the total file size for `Content-Range: bytes */<total>`.
    Unsatisfiable(u64),
    /// 200 (`range = None`) or 206 (`range = Some((start, end))`).
    Data { body: Vec<u8>, content_type: &'static str, total: u64, range: Option<(u64, u64)> },
}

/// Whether the file extension is in the library's video set or is `gif`: the media route serves only these extensions
/// (it does not grant the WebView arbitrary file read access, no matter where `LivelyInfo.json` points).
pub fn media_ext_ok(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| e == "gif" || library::VIDEO_EXT.contains(&e.as_str()))
}

/// The media file of a video/GIF wallpaper in the library (unknown id, wrong kind, wrong extension, missing file → `None`).
fn media_file(state: &AppState, id: &str) -> Option<(PathBuf, bool)> {
    let w = library::find(state.services.wallpaper.paths(), id)?;
    if !matches!(w.kind(), WallpaperType::Video | WallpaperType::Gif)
        || !w.source_exists()
        || !media_ext_ok(std::path::Path::new(&w.source()))
    {
        return None;
    }
    Some((PathBuf::from(w.source()), w.kind() == WallpaperType::Video))
}

/// Serves `/wallpaper/media/<id>` with the `Range` header (if any).
pub fn serve_media(state: &AppState, path: &str, range: Option<&str>) -> MediaReply {
    let Some((file, is_video)) = parse_media_route(path).and_then(|id| media_file(state, &id)) else { return MediaReply::NotFound };
    serve_media_file(&file, is_video, range)
}

/// The file-reading part of `serve_media` (needs no `AppState`, so it can be tested with a temp file).
pub fn serve_media_file(file: &std::path::Path, is_video: bool, range: Option<&str>) -> MediaReply {
    use std::io::{Read, Seek, SeekFrom};
    let content_type = media_content_type(file);
    let Ok(mut f) = std::fs::File::open(file) else { return MediaReply::NotFound };
    let Ok(total) = f.metadata().map(|m| m.len()) else { return MediaReply::NotFound };
    match plan_media(is_video, range, total, GIF_MAX_WHOLE) {
        RangeSpec::Unsatisfiable => MediaReply::Unsatisfiable(total),
        RangeSpec::TooLarge => MediaReply::TooLarge,
        RangeSpec::Whole => {
            // Only reached for a GIF <= GIF_MAX_WHOLE: allocate exactly the file size.
            let mut body = Vec::with_capacity(total as usize);
            if f.read_to_end(&mut body).is_err() {
                return MediaReply::NotFound;
            }
            MediaReply::Data { body, content_type, total, range: None }
        }
        RangeSpec::Part(start, end) => {
            let mut body = Vec::with_capacity((end - start + 1) as usize);
            if f.seek(SeekFrom::Start(start)).is_err() || f.take(end - start + 1).read_to_end(&mut body).is_err() {
                return MediaReply::NotFound;
            }
            // The file shrank midway: report exactly the range that was read.
            let got = body.len() as u64;
            if got == 0 {
                return MediaReply::Unsatisfiable(total);
            }
            MediaReply::Data { body, content_type, total, range: Some((start, start + got - 1)) }
        }
    }
}

/// Thumbnail of the wallpaper running on the primary monitor (or spanned): for the Home page.
pub fn running_main_thumb(state: &AppState) -> Option<(String, String)> {
    let svc = &state.services.wallpaper;
    let st = svc.state();
    let r = st.running.iter().find(|r| r.monitor == 1 || r.monitor == 0).or(st.running.first())?;
    let paths = svc.paths();
    let w = library::find(paths, &r.wallpaper_id)?;
    let p = thumbs::ensure_thumb(paths, &w)?;
    Some((w.title(), format!("{}/wallpaper/thumb/{}.jpg?v={}", protocol_base(), w.id, mtime(&p))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_route_parsing() {
        assert_eq!(parse_thumb_route("/wallpaper/thumb/abc123.jpg"), Some("abc123".into()));
        assert_eq!(parse_thumb_route("/wallpaper/thumb/a-b_c.jpg"), Some("a-b_c".into()));
        assert_eq!(parse_thumb_route("/wallpaper/thumb/../x.jpg"), None);
        assert_eq!(parse_thumb_route("/wallpaper/thumb/a/b.jpg"), None);
        assert_eq!(parse_thumb_route("/wallpaper/thumb/.jpg"), None);
        assert_eq!(parse_thumb_route("/wallpaper/thumb/a.png"), None);
        assert_eq!(parse_thumb_route("/wallpaper/current"), None);
        assert_eq!(parse_thumb_route("/cursor/thumb/a.jpg"), None);
    }

    #[test]
    fn media_route_parsing() {
        assert_eq!(parse_media_route("/wallpaper/media/abc123"), Some("abc123".into()));
        assert_eq!(parse_media_route("/wallpaper/media/a-b_c.1"), Some("a-b_c.1".into()));
        assert_eq!(parse_media_route("/wallpaper/media/../x"), None);
        assert_eq!(parse_media_route("/wallpaper/media/a/b"), None);
        assert_eq!(parse_media_route("/wallpaper/media/a..b"), None);
        assert_eq!(parse_media_route("/wallpaper/media/.hidden"), None);
        assert_eq!(parse_media_route("/wallpaper/media/"), None);
        assert_eq!(parse_media_route("/wallpaper/media/C:\\x"), None);
        assert_eq!(parse_media_route("/wallpaper/thumb/a.jpg"), None);
    }

    #[test]
    fn range_header_parsing() {
        use RangeSpec::*;
        let l = 1000;
        assert_eq!(parse_range(None, l), Whole);
        assert_eq!(parse_range(Some("bytes=0-99"), l), Part(0, 99));
        assert_eq!(parse_range(Some("bytes=900-999"), l), Part(900, 999));
        assert_eq!(parse_range(Some("bytes=900-5000"), l), Part(900, 999), "kết thúc quá cỡ thì kẹp lại");
        assert_eq!(parse_range(Some("bytes=500-"), l), Part(500, 999), "mở cuối");
        assert_eq!(parse_range(Some("bytes=-100"), l), Part(900, 999), "hậu tố");
        assert_eq!(parse_range(Some("bytes=-5000"), l), Part(0, 999), "hậu tố dài hơn tệp = cả tệp");
        // Out of range / malformed -> 416, no panic.
        assert_eq!(parse_range(Some("bytes=1000-1001"), l), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=5000-"), l), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), l), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=50-10"), l), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=a-b"), l), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), l), Unsatisfiable);
        assert_eq!(parse_range(Some("lines=0-1"), l), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=99999999999999999999-"), l), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-0"), 0), Unsatisfiable, "tệp rỗng");
        // Each response is at most 4 MiB.
        let big = 100 * 1024 * 1024;
        assert_eq!(parse_range(Some("bytes=10-"), big), Part(10, 10 + MEDIA_MAX_RANGE - 1));
        assert_eq!(parse_range(Some("bytes=0-99999999"), big), Part(0, MEDIA_MAX_RANGE - 1));
        assert_eq!(parse_range(Some("bytes=18446744073709551614-"), u64::MAX), Part(u64::MAX - 1, u64::MAX - 1));
    }

    #[test]
    fn plan_media_by_kind_and_size() {
        use RangeSpec::*;
        let big = 100 * 1024 * 1024;
        // Video without Range -> like bytes=0- (206, at most 4 MiB), never Whole.
        assert_eq!(plan_media(true, None, big, GIF_MAX_WHOLE), Part(0, MEDIA_MAX_RANGE - 1));
        assert_eq!(plan_media(true, None, 1000, GIF_MAX_WHOLE), Part(0, 999));
        assert_eq!(plan_media(true, None, 0, GIF_MAX_WHOLE), Unsatisfiable);
        // GIF without Range: the whole file if <= the limit, refused (413) if larger.
        assert_eq!(plan_media(false, None, GIF_MAX_WHOLE, GIF_MAX_WHOLE), Whole);
        assert_eq!(plan_media(false, None, 5, GIF_MAX_WHOLE), Whole);
        assert_eq!(plan_media(false, None, GIF_MAX_WHOLE + 1, GIF_MAX_WHOLE), TooLarge);
        // With a Range, follow the Range for both kinds.
        assert_eq!(plan_media(false, Some("bytes=0-9"), GIF_MAX_WHOLE * 2, GIF_MAX_WHOLE), Part(0, 9));
        assert_eq!(plan_media(true, Some("bytes=-5"), 100, GIF_MAX_WHOLE), Part(95, 99));
    }

    #[test]
    fn media_ext_whitelist() {
        use std::path::Path;
        assert!(media_ext_ok(Path::new("a.mp4")));
        assert!(media_ext_ok(Path::new("a.MKV")));
        assert!(media_ext_ok(Path::new("a.gif")));
        assert!(media_ext_ok(Path::new("a.m2ts")));
        assert!(!media_ext_ok(Path::new("a.exe")));
        assert!(!media_ext_ok(Path::new("a.json")));
        assert!(!media_ext_ok(Path::new("noext")));
    }

    fn temp_file(name: &str, data: &[u8]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sg-media-test-{}-{name}", std::process::id()));
        std::fs::write(&d, data).unwrap();
        d
    }

    type Parts = (Vec<u8>, &'static str, u64, Option<(u64, u64)>);

    fn data_of(r: MediaReply) -> Parts {
        match r {
            MediaReply::Data { body, content_type, total, range } => (body, content_type, total, range),
            other => panic!("cần Data, được {other:?}"),
        }
    }

    #[test]
    fn serve_file_ranges() {
        let bytes: Vec<u8> = (0..100u8).collect();
        let f = temp_file("r.mp4", &bytes);
        // Full range.
        let got = data_of(serve_media_file(&f, true, Some("bytes=10-19")));
        assert_eq!(got, (bytes[10..20].to_vec(), "video/mp4", 100, Some((10, 19))));
        // Open-ended.
        let (b, _, _, r) = data_of(serve_media_file(&f, true, Some("bytes=90-")));
        assert_eq!((b, r), (bytes[90..].to_vec(), Some((90, 99))));
        // Suffix.
        let (b, _, _, r) = data_of(serve_media_file(&f, true, Some("bytes=-5")));
        assert_eq!((b, r), (bytes[95..].to_vec(), Some((95, 99))));
        // Out of range → 416 with the total size.
        assert!(matches!(serve_media_file(&f, true, Some("bytes=100-")), MediaReply::Unsatisfiable(100)));
        assert!(matches!(serve_media_file(&f, true, Some("bytes=5-2")), MediaReply::Unsatisfiable(100)));
        // Video without Range: like bytes=0- (206, not a 200 with the whole file).
        let (b, _, _, r) = data_of(serve_media_file(&f, true, None));
        assert_eq!((b, r), (bytes.clone(), Some((0, 99))));
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn serve_file_video_without_range_is_capped() {
        let n = (MEDIA_MAX_RANGE + 1000) as usize;
        let f = temp_file("big.webm", &vec![7u8; n]);
        let (b, _, total, r) = data_of(serve_media_file(&f, true, None));
        assert_eq!(b.len() as u64, MEDIA_MAX_RANGE);
        assert_eq!((total, r), (n as u64, Some((0, MEDIA_MAX_RANGE - 1))));
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn serve_file_gif_whole_and_missing() {
        let bytes = vec![1u8, 2, 3, 4, 5];
        let f = temp_file("g.gif", &bytes);
        let got = data_of(serve_media_file(&f, false, None));
        assert_eq!(got, (bytes, "image/gif", 5, None));
        let _ = std::fs::remove_file(&f);
        assert!(matches!(serve_media_file(&f, false, None), MediaReply::NotFound));
    }

    #[test]
    fn media_content_types() {
        let t = |n: &str| media_content_type(std::path::Path::new(n));
        assert_eq!(t("a.MP4"), "video/mp4");
        assert_eq!(t("a.webm"), "video/webm");
        assert_eq!(t("a.gif"), "image/gif");
        assert_eq!(t("a.xyz"), "application/octet-stream");
        assert_eq!(t("noext"), "application/octet-stream");
    }

    #[test]
    fn layout_removal() {
        let mut t: toml::Table = "volume = 3\n[layout]\n\"1\" = \"a\"\n\"2\" = \"b\"\nall = \"a\"\n".parse().unwrap();
        assert!(remove_from_layout(&mut t, "a"));
        let l = t["layout"].as_table().unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l["2"].as_str(), Some("b"));
        assert!(!remove_from_layout(&mut t, "zzz"));
        assert!(!remove_from_layout(&mut toml::Table::new(), "a"));
    }
}
