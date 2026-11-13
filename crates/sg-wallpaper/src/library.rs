// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Wallpaper library. Each wallpaper is a subdirectory of `library\` containing `TachyonInfo.json`
//! (the same format as Lively's `LivelyInfo.json`, so a Lively wallpaper folder can be copied in and used directly).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

use crate::config::WallpaperPaths;
use crate::util::new_id;

pub const INFO_FILE: &str = "TachyonInfo.json";
pub const LIVELY_INFO_FILE: &str = "LivelyInfo.json";
pub const PROPERTIES_FILE: &str = "LivelyProperties.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallpaperType {
    App,
    Web,
    WebAudio,
    Url,
    Bizhawk,
    Unity,
    Godot,
    Video,
    Gif,
    UnityAudio,
    VideoStream,
    Picture,
}

impl WallpaperType {
    const ALL: [(WallpaperType, &'static str); 12] = [
        (WallpaperType::App, "app"),
        (WallpaperType::Web, "web"),
        (WallpaperType::WebAudio, "webaudio"),
        (WallpaperType::Url, "url"),
        (WallpaperType::Bizhawk, "bizhawk"),
        (WallpaperType::Unity, "unity"),
        (WallpaperType::Godot, "godot"),
        (WallpaperType::Video, "video"),
        (WallpaperType::Gif, "gif"),
        (WallpaperType::UnityAudio, "unityaudio"),
        (WallpaperType::VideoStream, "videostream"),
        (WallpaperType::Picture, "picture"),
    ];

    pub fn as_str(self) -> &'static str {
        Self::ALL.iter().find(|(t, _)| *t == self).map(|(_, s)| *s).unwrap_or("web")
    }

    pub fn from_index(i: i64) -> Option<Self> {
        Self::ALL.get(usize::try_from(i).ok()?).map(|(t, _)| *t)
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, n)| n.eq_ignore_ascii_case(s)).map(|(t, _)| *t)
    }

    pub fn label(self) -> &'static str {
        match self {
            WallpaperType::Video | WallpaperType::VideoStream => "Video",
            WallpaperType::Gif => "GIF",
            WallpaperType::Web | WallpaperType::WebAudio => "Web",
            WallpaperType::Url => "Trang web",
            WallpaperType::Picture => "Ảnh",
            _ => "Ứng dụng",
        }
    }

}

// Lively writes Type as a number (the Newtonsoft default). We write it as a number so Lively can read it too,
// but we accept both numbers and strings when reading.
impl Serialize for WallpaperType {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let idx = Self::ALL.iter().position(|(t, _)| t == self).unwrap_or(1);
        s.serialize_i64(idx as i64)
    }
}

impl<'de> Deserialize<'de> for WallpaperType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        let t = match &v {
            serde_json::Value::Number(n) => n.as_i64().and_then(WallpaperType::from_index),
            serde_json::Value::String(s) => WallpaperType::from_name(s),
            _ => None,
        };
        t.ok_or_else(|| serde::de::Error::custom(format!("Type không hợp lệ: {v}")))
    }
}

fn default_type() -> WallpaperType {
    WallpaperType::Web
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct WallpaperInfo {
    #[serde(default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub contact: Option<String>,
    #[serde(default = "default_type")]
    pub r#type: WallpaperType,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub is_absolute_path: bool,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub version: i64,
}

impl WallpaperInfo {
    pub fn new(kind: WallpaperType, title: &str, file_name: &str, absolute: bool) -> Self {
        WallpaperInfo {
            app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            title: Some(title.to_string()),
            thumbnail: None,
            preview: None,
            desc: None,
            author: None,
            license: None,
            contact: None,
            r#type: kind,
            file_name: Some(file_name.to_string()),
            arguments: None,
            is_absolute_path: absolute,
            id: None,
            tags: None,
            version: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Wallpaper {
    /// Directory name in the library — used as the id.
    pub id: String,
    pub dir: PathBuf,
    pub info: WallpaperInfo,
}

impl Wallpaper {
    pub fn kind(&self) -> WallpaperType {
        self.info.r#type
    }

    pub fn title(&self) -> String {
        self.info
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| self.id.clone())
    }

    /// Absolute path to the main file, or the URL for the `url`/`videostream` types.
    pub fn source(&self) -> String {
        let f = self.info.file_name.clone().unwrap_or_default();
        if matches!(self.kind(), WallpaperType::Url | WallpaperType::VideoStream) || self.info.is_absolute_path {
            return f;
        }
        self.dir.join(f).to_string_lossy().into_owned()
    }

    pub fn source_exists(&self) -> bool {
        match self.kind() {
            WallpaperType::Url | WallpaperType::VideoStream => true,
            _ => Path::new(&self.source()).exists(),
        }
    }

    pub fn thumbnail_path(&self) -> Option<PathBuf> {
        let t = self.info.thumbnail.as_ref()?;
        let p = if Path::new(t).is_absolute() { PathBuf::from(t) } else { self.dir.join(t) };
        p.exists().then_some(p)
    }

    /// The original LivelyProperties.json file (customization of web/video wallpapers).
    pub fn properties_template(&self) -> Option<PathBuf> {
        let p = self.dir.join(PROPERTIES_FILE);
        p.exists().then_some(p)
    }

    /// Root directory of the content (to compute folderDropdown's relative paths).
    pub fn content_dir(&self) -> PathBuf {
        if self.info.is_absolute_path {
            Path::new(&self.source()).parent().map(Path::to_path_buf).unwrap_or_else(|| self.dir.clone())
        } else {
            self.dir.clone()
        }
    }
}

pub fn read_info(dir: &Path) -> Option<WallpaperInfo> {
    for name in [INFO_FILE, LIVELY_INFO_FILE] {
        let p = dir.join(name);
        if let Ok(text) = std::fs::read_to_string(&p) {
            // Some Lively files have a UTF-8 BOM.
            let text = text.trim_start_matches('\u{feff}');
            match serde_json::from_str::<WallpaperInfo>(text) {
                Ok(info) => return Some(info),
                Err(e) => logln!("Bỏ qua {}: {e}", p.display()),
            }
        }
    }
    None
}

pub fn load_one(dir: &Path) -> Option<Wallpaper> {
    let info = read_info(dir)?;
    let id = dir.file_name()?.to_string_lossy().into_owned();
    Some(Wallpaper { id, dir: dir.to_path_buf(), info })
}

pub fn load_all(paths: &WallpaperPaths) -> Vec<Wallpaper> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&paths.library) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                if let Some(w) = load_one(&e.path()) {
                    out.push(w);
                }
            }
        }
    }
    out.sort_by_key(|w| w.title().to_lowercase());
    out
}

pub fn find(paths: &WallpaperPaths, id: &str) -> Option<Wallpaper> {
    if !valid_id(id) {
        return None;
    }
    let dir = paths.library.join(id);
    if !dir.is_dir() {
        return None;
    }
    load_one(&dir)
}

fn write_info(dir: &Path, info: &WallpaperInfo) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(info).map_err(std::io::Error::other)?;
    std::fs::write(dir.join(INFO_FILE), text)
}

pub const VIDEO_EXT: &[&str] = &["mp4", "m4v", "webm", "mkv", "mov", "avi", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "3gp", "ogv"];
const PICTURE_EXT: &[&str] = &["jpg", "jpeg", "png", "bmp", "jfif", "webp", "tif", "tiff", "heic"];
const WEB_EXT: &[&str] = &["html", "htm"];
const APP_EXT: &[&str] = &["exe"];

/// Guesses the wallpaper type from the file extension.
pub fn detect_type(path: &Path) -> Option<WallpaperType> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let e = ext.as_str();
    if VIDEO_EXT.contains(&e) {
        Some(WallpaperType::Video)
    } else if e == "gif" {
        Some(WallpaperType::Gif)
    } else if PICTURE_EXT.contains(&e) {
        Some(WallpaperType::Picture)
    } else if WEB_EXT.contains(&e) {
        Some(WallpaperType::Web)
    } else if APP_EXT.contains(&e) {
        Some(WallpaperType::App)
    } else {
        None
    }
}

/// A valid id: letters, digits, `-`, `_`, `.` (a directory name in the library), no `..`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && !id.starts_with('.')
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// Whether the wallpaper's source is still usable. The error starts with `source_missing` so the UI can translate it.
pub fn check_source(w: &Wallpaper) -> Result<(), String> {
    if w.source_exists() { Ok(()) } else { Err(format!("source_missing: {}", w.source())) }
}

pub fn is_url(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Adds a file or URL to the library. If a wallpaper already points at this exact source, the existing one is returned.
pub fn add(paths: &WallpaperPaths, source: &str, forced: Option<WallpaperType>) -> Result<Wallpaper, String> {
    let source = source.trim().trim_matches('"');
    if source.is_empty() {
        return Err("Chưa nhập đường dẫn hoặc URL".into());
    }

    if !is_url(source) {
        let p = Path::new(source);
        if p.is_dir() {
            return import_folder(paths, p).and_then(|mut v| v.pop().ok_or_else(|| "Thư mục không có hình nền nào".to_string()));
        }
        if p.extension().map(|e| e.eq_ignore_ascii_case("zip")).unwrap_or(false) {
            return import_zip(paths, p);
        }
    }

    // Reuse it if it already exists.
    let normalized = normalize_source(source);
    if let Some(existing) = load_all(paths).into_iter().find(|w| normalize_source(&w.source()) == normalized) {
        return Ok(existing);
    }

    let (kind, title, file_name) = if is_url(source) {
        let kind = forced.unwrap_or(WallpaperType::Url);
        let title = source
            .split("://")
            .nth(1)
            .unwrap_or(source)
            .trim_end_matches('/')
            .chars()
            .take(60)
            .collect::<String>();
        (kind, title, source.to_string())
    } else {
        let p = Path::new(source);
        if !p.exists() {
            return Err(format!("Không tìm thấy file: {source}"));
        }
        let abs = std::fs::canonicalize(p)
            .map(|c| strip_verbatim(&c))
            .unwrap_or_else(|_| p.to_path_buf());
        let kind = forced
            .or_else(|| detect_type(&abs))
            .ok_or_else(|| format!("Không hỗ trợ loại file này: {}", abs.display()))?;
        let title = abs.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        (kind, title, abs.to_string_lossy().into_owned())
    };

    let id = new_id();
    let dir = paths.library.join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Không tạo được thư mục thư viện: {e}"))?;
    let info = WallpaperInfo::new(kind, &title, &file_name, !matches!(kind, WallpaperType::Url));

    write_info(&dir, &info).map_err(|e| format!("Không ghi được {INFO_FILE}: {e}"))?;
    logln!("Đã thêm hình nền {id}: {file_name} ({})", kind.as_str());
    Ok(Wallpaper { id, dir, info })
}

pub fn normalize_source(s: &str) -> String {
    s.trim().trim_end_matches(['/', '\\']).replace('/', "\\").to_lowercase()
}

fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

/// Imports a Lively wallpaper folder: a folder with LivelyInfo.json, or a folder containing several such folders.
pub fn import_folder(paths: &WallpaperPaths, src: &Path) -> Result<Vec<Wallpaper>, String> {
    let mut candidates = Vec::new();
    if read_info(src).is_some() {
        candidates.push(src.to_path_buf());
    } else if let Ok(rd) = std::fs::read_dir(src) {
        for e in rd.flatten() {
            if e.path().is_dir() && read_info(&e.path()).is_some() {
                candidates.push(e.path());
            }
        }
    }
    if candidates.is_empty() {
        return Err(format!("Không thấy {LIVELY_INFO_FILE} hoặc {INFO_FILE} trong {}", src.display()));
    }
    let mut out = Vec::new();
    for c in candidates {
        let id = new_id();
        let dst = paths.library.join(&id);
        copy_dir(&c, &dst).map_err(|e| format!("Chép {} thất bại: {e}", c.display()))?;
        normalize_info_file(&dst);
        if let Some(w) = load_one(&dst) {
            logln!("Đã nhập {} → {id}", c.display());
            out.push(w);
        }
    }
    Ok(out)
}

/// Limits when extracting a wallpaper zip (the same as `sg_core::archive::Limits::default()`).
pub fn default_limits() -> sg_core::archive::Limits {
    sg_core::archive::Limits::default()
}

/// Imports a wallpaper .zip file (Lively format: a zip with LivelyInfo.json at its root).
pub fn import_zip(paths: &WallpaperPaths, zip_path: &Path) -> Result<Wallpaper, String> {
    import_zip_with(paths, zip_path, &default_limits())
}

/// Safe extraction: an entry name containing `..`, an absolute path or a drive letter → the whole zip is rejected;
/// an entry count or byte count (counted for real during extraction) over the limit → rejected. Any error deletes the half-written directory.
pub fn import_zip_with(paths: &WallpaperPaths, zip_path: &Path, limits: &sg_core::archive::Limits) -> Result<Wallpaper, String> {
    use std::io::Read;
    let file = std::fs::File::open(zip_path).map_err(|e| format!("Không mở được zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("File zip hỏng: {e}"))?;
    if archive.len() > limits.max_entries {
        return Err(format!("Zip có quá nhiều mục ({} > {})", archive.len(), limits.max_entries));
    }
    let id = new_id();
    let dst = paths.library.join(&id);
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
        let mut total: u64 = 0;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let raw = String::from_utf8_lossy(entry.name_raw()).into_owned();
            if raw.contains(':') || raw.starts_with(['/', '\\']) {
                return Err(format!("Zip có đường dẫn không an toàn: {raw}"));
            }
            // enclosed_name blocks `..` and absolute paths (zip slip).
            let Some(rel) = entry.enclosed_name() else {
                return Err(format!("Zip có đường dẫn không an toàn: {raw}"));
            };
            let out = dst.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let remaining = limits.max_total_bytes.saturating_sub(total);
            let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            let copied = std::io::copy(&mut (&mut entry).take(remaining + 1), &mut f).map_err(|e| e.to_string())?;
            if copied > remaining {
                return Err("Zip giải nén ra quá lớn".to_string());
            }
            total += copied;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&dst);
        return Err(e);
    }
    // Some zips are packed with an extra wrapper folder around the content.
    if read_info(&dst).is_none() {
        let inner: Vec<PathBuf> = std::fs::read_dir(&dst)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
        if inner.len() == 1 && read_info(&inner[0]).is_some() {
            let tmp = paths.library.join(format!("{id}.tmp"));
            std::fs::rename(&inner[0], &tmp).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&dst);
            std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
        }
    }
    if read_info(&dst).is_none() {
        let _ = std::fs::remove_dir_all(&dst);
        return Err("Zip không chứa LivelyInfo.json / TachyonInfo.json".into());
    }
    normalize_info_file(&dst);
    logln!("Đã nhập zip {} → {id}", zip_path.display());
    load_one(&dst).ok_or_else(|| "Không đọc được hình nền vừa nhập".into())
}

/// Also writes TachyonInfo.json (LivelyInfo.json is left untouched so it can still be opened with Lively).
fn normalize_info_file(dir: &Path) {
    if !dir.join(INFO_FILE).exists() {
        if let Some(info) = read_info(dir) {
            let _ = write_info(dir, &info);
        }
    }
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let target = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), target)?;
        }
    }
    Ok(())
}

/// Deletes a wallpaper from the library (only removes the directory inside the library, never touching the user's original files).
pub fn delete(paths: &WallpaperPaths, id: &str) -> Result<(), String> {
    let w = find(paths, id).ok_or("Không có hình nền này")?;
    std::fs::remove_dir_all(&w.dir).map_err(|e| format!("Không xoá được: {e}"))?;
    let _ = std::fs::remove_file(paths.properties.join(format!("{id}.json")));
    let _ = std::fs::remove_file(crate::thumbs::thumb_path(paths, id));
    Ok(())
}

pub fn rename(paths: &WallpaperPaths, id: &str, title: &str) -> Result<(), String> {
    let mut w = find(paths, id).ok_or("Không có hình nền này")?;
    w.info.title = Some(title.trim().to_string());
    write_info(&w.dir, &w.info).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lively_numeric_and_string_types() {
        let a: WallpaperInfo = serde_json::from_str(r#"{"Title":"Fluids","Type":1,"FileName":"index.html"}"#).unwrap();
        assert_eq!(a.r#type, WallpaperType::Web);
        let b: WallpaperInfo = serde_json::from_str(r#"{"Type":"video","FileName":"a.mp4","IsAbsolutePath":true}"#).unwrap();
        assert_eq!(b.r#type, WallpaperType::Video);
        assert!(b.is_absolute_path);
        let c: WallpaperInfo = serde_json::from_str(r#"{"Type":11}"#).unwrap();
        assert_eq!(c.r#type, WallpaperType::Picture);
        // Written out as a number, like Lively.
        let text = serde_json::to_string(&c).unwrap();
        assert!(text.contains("\"Type\":11"));
    }

    #[test]
    fn detects_types_by_extension() {
        assert_eq!(detect_type(Path::new("a.MP4")), Some(WallpaperType::Video));
        assert_eq!(detect_type(Path::new("a.gif")), Some(WallpaperType::Gif));
        assert_eq!(detect_type(Path::new("a.jpeg")), Some(WallpaperType::Picture));
        assert_eq!(detect_type(Path::new("index.html")), Some(WallpaperType::Web));
        assert_eq!(detect_type(Path::new("game.exe")), Some(WallpaperType::App));
        assert_eq!(detect_type(Path::new("a.txt")), None);
    }

    #[test]
    fn url_detection_and_normalization() {
        assert!(is_url("https://example.com"));
        assert!(!is_url("C:\\a.mp4"));
        assert_eq!(normalize_source("C:/A/B.mp4"), normalize_source("c:\\a\\b.mp4"));
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\x\y.mp4")), PathBuf::from(r"C:\x\y.mp4"));
    }

    fn test_paths(root: &Path) -> WallpaperPaths {
        let p = WallpaperPaths {
            library: root.join("lib"),
            properties: root.join("props"),
            thumbs: root.join("thumbs"),
            webview: root.join("web"),
            backup_file: root.join("backup.toml"),
        };
        p.ensure();
        p
    }

    fn zip_with(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opt = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            z.start_file(*name, opt).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    const INFO: &[u8] = br#"{"Title":"T","Type":1,"FileName":"index.html"}"#;

    #[test]
    fn detect_type_cases() {
        assert_eq!(detect_type(Path::new("x.mkv")), Some(WallpaperType::Video));
        assert_eq!(detect_type(Path::new("x.WEBP")), Some(WallpaperType::Picture));
        assert_eq!(detect_type(Path::new("x.htm")), Some(WallpaperType::Web));
        assert_eq!(detect_type(Path::new("noext")), None);
        assert!(is_url(" HTTPS://a.b ") && !is_url("ftp://a"));
        assert_eq!(WallpaperType::from_name("videostream"), Some(WallpaperType::VideoStream));
        assert_eq!(WallpaperType::Url.as_str(), "url");
    }

    #[test]
    fn import_zip_ok_and_wrapped_folder() {
        let d = tempfile::tempdir().unwrap();
        let paths = test_paths(d.path());
        let z = d.path().join("a.zip");
        zip_with(&z, &[("LivelyInfo.json", INFO), ("index.html", b"<html></html>")]);
        let w = import_zip(&paths, &z).unwrap();
        assert_eq!(w.title(), "T");
        assert!(w.dir.join(INFO_FILE).exists());
        let z2 = d.path().join("b.zip");
        zip_with(&z2, &[("Wrapped/LivelyInfo.json", INFO), ("Wrapped/index.html", b"x")]);
        assert!(import_zip(&paths, &z2).is_ok());
    }

    #[test]
    fn import_zip_slip_rejected() {
        let d = tempfile::tempdir().unwrap();
        let paths = test_paths(d.path());
        let z = d.path().join("evil.zip");
        zip_with(&z, &[("LivelyInfo.json", INFO), ("../evil.txt", b"pwn")]);
        let err = import_zip(&paths, &z).unwrap_err();
        assert!(err.contains("không an toàn"), "{err}");
        assert!(!d.path().join("evil.txt").exists());
        assert!(!paths.library.join("evil.txt").exists());
        assert_eq!(std::fs::read_dir(&paths.library).unwrap().count(), 0, "thư mục dở dang phải bị xoá");
        let z = d.path().join("abs.zip");
        zip_with(&z, &[("C:/evil.txt", b"x")]);
        assert!(import_zip(&paths, &z).is_err());
        let z = d.path().join("bs.zip");
        zip_with(&z, &[("..\\evil2.txt", b"x")]);
        assert!(import_zip(&paths, &z).is_err());
        assert!(!d.path().join("evil2.txt").exists());
    }

    #[test]
    fn import_zip_limits() {
        let d = tempfile::tempdir().unwrap();
        let paths = test_paths(d.path());
        let z = d.path().join("big.zip");
        zip_with(&z, &[("LivelyInfo.json", INFO), ("big.bin", &vec![7u8; 5000])]);
        let tiny = sg_core::archive::Limits { max_entries: 10, max_total_bytes: 1000 };
        assert!(import_zip_with(&paths, &z, &tiny).is_err());
        assert_eq!(std::fs::read_dir(&paths.library).unwrap().count(), 0);
        let few = sg_core::archive::Limits { max_entries: 1, max_total_bytes: 1 << 20 };
        assert!(import_zip_with(&paths, &z, &few).is_err());
        assert!(import_zip_with(&paths, &z, &default_limits()).is_ok());
    }

    #[test]
    fn missing_source_reports_error() {
        let d = tempfile::tempdir().unwrap();
        let paths = test_paths(d.path());
        let f = d.path().join("clip.mp4");
        std::fs::write(&f, b"x").unwrap();
        let w = add(&paths, f.to_str().unwrap(), None).unwrap();
        assert_eq!(w.kind(), WallpaperType::Video);
        assert!(check_source(&w).is_ok());
        std::fs::remove_file(&f).unwrap();
        let w = find(&paths, &w.id).unwrap();
        assert!(!w.source_exists());
        assert!(check_source(&w).unwrap_err().starts_with("source_missing"));
        // A URL is always valid as far as files go.
        let u = add(&paths, "https://example.com/a", None).unwrap();
        assert!(check_source(&u).is_ok());
    }

    #[test]
    fn delete_and_find_validate_ids() {
        let d = tempfile::tempdir().unwrap();
        let paths = test_paths(d.path());
        assert!(find(&paths, "../x").is_none());
        assert!(find(&paths, "..").is_none());
        assert!(delete(&paths, "../lib").is_err());
        let u = add(&paths, "https://example.com/b", None).unwrap();
        assert!(valid_id(&u.id));
        rename(&paths, &u.id, "Tên mới").unwrap();
        assert_eq!(find(&paths, &u.id).unwrap().title(), "Tên mới");
        delete(&paths, &u.id).unwrap();
        assert!(find(&paths, &u.id).is_none());
    }
}
