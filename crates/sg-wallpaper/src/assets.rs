//! Config bundles that carry their assets: export includes the library folders of the wallpapers named in `[layout]`
//! (`modules/wallpaper/assets/<id>/…`); import copies them into the library if the id isn't there yet.

use crate::config::WallpaperPaths;
use crate::library::{self, INFO_FILE, Wallpaper, WallpaperType};
use sg_core::archive::{Limits, normalize_entry_name};
use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetSource {
    File(PathBuf),
    Bytes(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct ExportAssets {
    /// (path inside the `.sgdecor` file, source).
    pub files: Vec<(String, AssetSource)>,
    /// Wallpapers that were skipped and why (log only; the config is still exported).
    pub warnings: Vec<String>,
}

fn list_files(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            list_files(&p, base, out);
        } else if ft.is_file() {
            let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            out.push((rel, p, e.metadata().map(|m| m.len()).unwrap_or(0)));
        }
    }
}

/// All assets that must be carried for the config `config_text`. URL/stream wallpapers are skipped;
/// exceeding `limits` (entry count, total bytes) drops that whole wallpaper (with a warning).
pub fn collect_export_assets(paths: &WallpaperPaths, config_text: &str, limits: &Limits) -> ExportAssets {
    let mut out = ExportAssets::default();
    let cfg: toml::Table = config_text.parse().unwrap_or_default();
    let ids: BTreeSet<String> = cfg
        .get("layout")
        .and_then(toml::Value::as_table)
        .map(|l| l.values().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    let mut total: u64 = 0;
    // Leave room for config.toml + manifest.
    let mut entries = 2usize;
    for id in ids {
        let Some(w) = library::find(paths, &id) else {
            out.warnings.push(format!("{id}: not in the library"));
            continue;
        };
        if matches!(w.kind(), WallpaperType::Url | WallpaperType::VideoStream) {
            continue;
        }
        match item_files(&w, &id) {
            Ok(files) => {
                let size: u64 = files.iter().map(|(_, s, _)| s).sum();
                if entries + files.len() > limits.max_entries || total.saturating_add(size) > limits.max_total_bytes {
                    out.warnings.push(format!("{id}: too large to bundle, skipped"));
                    continue;
                }
                entries += files.len();
                total += size;
                out.files.extend(files.into_iter().map(|(name, _, src)| (name, src)));
            }
            Err(e) => out.warnings.push(format!("{id}: {e}")),
        }
    }
    out
}

/// The files of one wallpaper: (path inside the file, size, source).
fn item_files(w: &Wallpaper, id: &str) -> Result<Vec<(String, u64, AssetSource)>, String> {
    let mut listed = Vec::new();
    list_files(&w.dir, &w.dir, &mut listed);
    let mut files: Vec<(String, u64, AssetSource)> = Vec::new();
    for (rel, path, size) in listed {
        if rel == INFO_FILE {
            continue; // rewritten below
        }
        files.push((format!("modules/wallpaper/assets/{id}/{rel}"), size, AssetSource::File(path)));
    }
    let mut info = w.info.clone();
    if info.is_absolute_path {
        // The source file lives outside the library: carry it along and switch to a relative path.
        let src = PathBuf::from(w.source());
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("source has no file name")?;
        let meta = std::fs::metadata(&src).map_err(|_| "source file is missing".to_string())?;
        if !meta.is_file() {
            return Err("source is not a file".into());
        }
        files.push((format!("modules/wallpaper/assets/{id}/{name}"), meta.len(), AssetSource::File(src)));
        info.file_name = Some(name);
        info.is_absolute_path = false;
    }
    let bytes = serde_json::to_vec_pretty(&info).map_err(|e| e.to_string())?;
    files.push((format!("modules/wallpaper/assets/{id}/{INFO_FILE}"), bytes.len() as u64, AssetSource::Bytes(bytes)));
    // A single invalid name (e.g. `CON`, a `:`) would break the whole wallpaper on import: drop the whole wallpaper.
    for (name, _, _) in &files {
        normalize_entry_name(name).map_err(|_| format!("file name not allowed in a bundle: {name}"))?;
    }
    let mut seen = BTreeSet::new();
    for (name, _, _) in &files {
        if !seen.insert(name.to_lowercase()) {
            return Err("duplicate file names".into());
        }
    }
    Ok(files)
}

/// Write the collected assets into the `.sgdecor` file.
pub fn write_export_assets(w: &mut sg_core::archive::ArchiveWriter, assets: &ExportAssets) -> io::Result<()> {
    for (name, src) in &assets.files {
        match src {
            AssetSource::File(p) => w.add_file(name, p)?,
            AssetSource::Bytes(b) => w.add_bytes(name, b)?,
        }
    }
    Ok(())
}

/// Import assets: copy `modules/wallpaper/assets/<id>/…` into the library if the id isn't there yet (never overwrites).
pub struct AssetImporter<'a> {
    paths: &'a WallpaperPaths,
    /// id → whether to copy (decided at the id's first file).
    decided: HashMap<String, bool>,
    pub copied: BTreeSet<String>,
}

impl<'a> AssetImporter<'a> {
    pub fn new(paths: &'a WallpaperPaths) -> AssetImporter<'a> {
        AssetImporter { paths, decided: HashMap::new(), copied: BTreeSet::new() }
    }

    /// `rel` is the path inside the file (`modules/wallpaper/assets/<id>/<file>`); other kinds are ignored.
    pub fn accept(&mut self, rel: &str, from: &Path) -> io::Result<()> {
        let Some(rest) = rel.strip_prefix("modules/wallpaper/assets/") else { return Ok(()) };
        let Some((id, file)) = rest.split_once('/') else { return Ok(()) };
        if !library::valid_id(id) || file.is_empty() {
            return Ok(());
        }
        let dir = self.paths.library.join(id);
        let want = *self.decided.entry(id.to_string()).or_insert_with(|| !dir.exists());
        if !want {
            return Ok(());
        }
        let to = file.split('/').fold(dir, |p, c| p.join(c));
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, &to)?;
        self.copied.insert(id.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::add;

    fn paths(root: &Path) -> WallpaperPaths {
        let p = WallpaperPaths {
            library: root.join("lib"),
            properties: root.join("props"),
            thumbs: root.join("thumbs"),
            webview: root.join("web"),
            backup_file: root.join("b.toml"),
        };
        p.ensure();
        p
    }

    #[test]
    fn export_assets_skips_urls_and_caps_size() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let clip = d.path().join("clip.mp4");
        std::fs::write(&clip, vec![1u8; 4000]).unwrap();
        let big = d.path().join("big.mp4");
        std::fs::write(&big, vec![2u8; 9000]).unwrap();
        let w_clip = add(&p, clip.to_str().unwrap(), None).unwrap();
        let w_big = add(&p, big.to_str().unwrap(), None).unwrap();
        let w_url = add(&p, "https://example.com/live", None).unwrap();
        let cfg = format!("[layout]\n\"1\" = \"{}\"\n\"2\" = \"{}\"\n\"3\" = \"{}\"\n\"4\" = \"khong-co\"\n", w_clip.id, w_big.id, w_url.id);
        let limits = Limits { max_entries: 100, max_total_bytes: 6000 };
        let a = collect_export_assets(&p, &cfg, &limits);
        let names: Vec<&str> = a.files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&format!("modules/wallpaper/assets/{}/clip.mp4", w_clip.id).as_str()), "{names:?}");
        assert!(names.iter().any(|n| n.ends_with("/TachyonInfo.json") && n.contains(&w_clip.id)));
        assert!(!names.iter().any(|n| n.contains(&w_big.id)), "quá giới hạn → bỏ cả hình nền");
        assert!(!names.iter().any(|n| n.contains(&w_url.id)), "URL không có tệp để mang");
        assert!(a.warnings.iter().any(|w| w.contains("too large")) && a.warnings.iter().any(|w| w.contains("khong-co")));
        // The absolute path was converted to a relative one in the carried info.
        let info = a.files.iter().find_map(|(n, s)| (n.ends_with("TachyonInfo.json") && n.contains(&w_clip.id)).then_some(s)).unwrap();
        let AssetSource::Bytes(b) = info else { panic!() };
        let v: serde_json::Value = serde_json::from_slice(b).unwrap();
        assert_eq!(v["FileName"], "clip.mp4");
        assert_eq!(v["IsAbsolutePath"], false);
        // Every name is accepted by `.sgdecor`.
        assert!(a.files.iter().all(|(n, _)| normalize_entry_name(n).is_ok()));
    }

    #[test]
    fn importer_copies_only_new_ids_and_validates() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        std::fs::create_dir_all(p.library.join("old")).unwrap();
        let src = d.path().join("src.txt");
        std::fs::write(&src, b"x").unwrap();
        let mut imp = AssetImporter::new(&p);
        imp.accept("modules/wallpaper/assets/new1/a.mp4", &src).unwrap();
        imp.accept("modules/wallpaper/assets/new1/sub/b.txt", &src).unwrap();
        imp.accept("modules/wallpaper/assets/old/a.mp4", &src).unwrap();
        imp.accept("modules/wallpaper/assets/../evil/a.mp4", &src).unwrap();
        imp.accept("modules/cursor/assets/x/a.cur", &src).unwrap();
        assert!(p.library.join("new1").join("a.mp4").is_file() && p.library.join("new1").join("sub").join("b.txt").is_file());
        assert!(!p.library.join("old").join("a.mp4").exists(), "id đã có thì không ghi đè");
        assert!(!p.library.join("evil").exists());
        assert_eq!(imp.copied.iter().collect::<Vec<_>>(), ["new1"]);
    }
}
