//! `.sgdecor` file: a zip containing `manifest.toml` + configuration (+ assets in later stages).
//!
//! Imported files may come from strangers, so every entry name is validated before extraction
//! and files are only extracted into a temporary directory.

use crate::ids::ModuleId;
use crate::profiles::validate_name;
use crate::text::I18nText;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const FORMAT_VERSION: u32 = 1;
pub const MANIFEST: &str = "manifest.toml";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveKind {
    /// A configuration set for one part.
    Module,
    /// "Export all" in Settings.
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub app_version: String,
    pub kind: ArchiveKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<ModuleId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(default)]
    pub created_unix: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_entries: usize,
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_entries: 2000, max_total_bytes: 1 << 30 }
    }
}

#[derive(Debug)]
pub struct StagedArchive {
    pub manifest: Manifest,
    /// Temporary directory holding the extracted files (paths use `/`, as in `files`).
    pub root: PathBuf,
    /// The files (excluding the manifest), in zip order.
    pub files: Vec<String>,
}

fn unsafe_path(name: &str) -> I18nText {
    I18nText::new("err.archive.unsafe_path").arg("name", name)
}

/// Normalizes a zip entry name and checks it fits the `.sgdecor` layout:
/// `manifest.toml`, `app/app.toml`, `modules/<part>/config.toml`,
/// `modules/<part>/profiles/<name>.toml`, `modules/<part>/assets/<…>`.
pub fn normalize_entry_name(raw: &str) -> Result<String, I18nText> {
    let s = raw.replace('\\', "/");
    if s.starts_with('/') || s.contains(':') || s.chars().any(char::is_control) {
        return Err(unsafe_path(raw));
    }
    let parts: Vec<&str> = s.split('/').collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
        return Err(unsafe_path(raw));
    }
    let ok = match parts.as_slice() {
        [MANIFEST] => true,
        ["app", "app.toml"] => true,
        ["modules", id, rest @ ..] if ModuleId::parse(id).is_some() => match rest {
            ["config.toml"] => true,
            ["profiles", file] => file
                .strip_suffix(".toml")
                .is_some_and(|stem| validate_name(stem).is_ok_and(|n| n == *stem)),
            // Every asset path segment must be a valid Windows file name (no CON, nul.txt, trailing dot…).
            ["assets", more @ ..] => !more.is_empty() && more.iter().all(|c| validate_name(c).is_ok_and(|n| n == *c)),
            _ => false,
        },
        _ => false,
    };
    if ok { Ok(s) } else { Err(unsafe_path(raw)) }
}

/// Writes the zip to `<dest>.tmp` and only renames it to `dest` once `finish` completes — never leaves a half-written file.
pub struct ArchiveWriter {
    dest: PathBuf,
    tmp: PathBuf,
    zip: Option<zip::ZipWriter<File>>,
}

impl ArchiveWriter {
    pub fn create(dest: &Path) -> io::Result<ArchiveWriter> {
        let mut tmp = dest.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = File::create(&tmp)?;
        Ok(ArchiveWriter { dest: dest.to_path_buf(), tmp, zip: Some(zip::ZipWriter::new(f)) })
    }

    fn options() -> zip::write::SimpleFileOptions {
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
    }

    fn start(&mut self, path: &str) -> io::Result<&mut zip::ZipWriter<File>> {
        let name = normalize_entry_name(path).map_err(|e| io::Error::other(e.to_string()))?;
        if name == MANIFEST {
            return Err(io::Error::other("manifest.toml is written by finish()"));
        }
        let z = self.zip.as_mut().ok_or_else(|| io::Error::other("archive already finished"))?;
        z.start_file(name, Self::options()).map_err(io::Error::other)?;
        Ok(z)
    }

    pub fn add_bytes(&mut self, path: &str, data: &[u8]) -> io::Result<()> {
        self.start(path)?.write_all(data)
    }

    pub fn add_file(&mut self, path: &str, src: &Path) -> io::Result<()> {
        let mut f = File::open(src)?;
        let z = self.start(path)?;
        io::copy(&mut f, z)?;
        Ok(())
    }

    pub fn finish(mut self, manifest: &Manifest) -> io::Result<()> {
        let text = toml::to_string_pretty(manifest).map_err(io::Error::other)?;
        let mut z = self.zip.take().ok_or_else(|| io::Error::other("archive already finished"))?;
        z.start_file(MANIFEST, Self::options()).map_err(io::Error::other)?;
        z.write_all(text.as_bytes())?;
        let f = z.finish().map_err(io::Error::other)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&self.tmp, &self.dest)
    }
}

impl Drop for ArchiveWriter {
    fn drop(&mut self) {
        if self.zip.is_some() || self.tmp.exists() {
            // Not finished (or the rename failed): delete the temp file.
            self.zip = None;
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

fn read_failed(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("err.archive.read_failed").arg("detail", e)
}

/// Reads a `.sgdecor` file and extracts it into `staging` (a new directory). All names and sizes
/// are checked before any file is written; on an error midway, `staging` is deleted.
pub fn read_archive(src: &Path, staging: &Path, limits: Limits) -> Result<StagedArchive, I18nText> {
    let file = File::open(src).map_err(read_failed)?;
    let mut z = zip::ZipArchive::new(file).map_err(|_| I18nText::new("err.archive.not_zip"))?;
    if z.len() > limits.max_entries {
        return Err(I18nText::new("err.archive.too_big"));
    }

    // Pass 1: read the index only.
    let mut entries: Vec<(usize, String, u64)> = Vec::new();
    let mut seen = HashSet::new();
    let mut total: u64 = 0;
    let mut manifest_idx = None;
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(|_| I18nText::new("err.archive.not_zip"))?;
        if f.is_dir() {
            continue;
        }
        let raw = f.name().map_err(|_| I18nText::new("err.archive.not_zip"))?;
        let name = normalize_entry_name(&raw)?;
        if !seen.insert(name.to_lowercase()) {
            return Err(unsafe_path(&name));
        }
        total = total.saturating_add(f.size());
        if total > limits.max_total_bytes {
            return Err(I18nText::new("err.archive.too_big"));
        }
        if name == MANIFEST {
            manifest_idx = Some(i);
        } else {
            entries.push((i, name, f.size()));
        }
    }
    let manifest_idx = manifest_idx.ok_or_else(|| I18nText::new("err.archive.no_manifest"))?;

    let manifest = {
        let f = z.by_index(manifest_idx).map_err(read_failed)?;
        let mut text = String::new();
        f.take(MAX_MANIFEST_BYTES).read_to_string(&mut text).map_err(|_| I18nText::new("err.archive.bad_manifest"))?;
        let table: toml::Table =
            crate::config::strip_bom(&text).parse().map_err(|_| I18nText::new("err.archive.bad_manifest"))?;
        let format = table.get("format").and_then(toml::Value::as_integer).unwrap_or(0);
        if format > FORMAT_VERSION as i64 {
            return Err(I18nText::new("err.archive.newer_format"));
        }
        toml::Value::Table(table).try_into::<Manifest>().map_err(|_| I18nText::new("err.archive.bad_manifest"))?
    };

    // Pass 2: extract into the temporary directory.
    let extract = |z: &mut zip::ZipArchive<File>| -> Result<Vec<String>, I18nText> {
        std::fs::create_dir_all(staging).map_err(read_failed)?;
        let mut files = Vec::with_capacity(entries.len());
        for (i, name, size) in &entries {
            let out = staging.join(name);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(read_failed)?;
            }
            let f = z.by_index(*i).map_err(read_failed)?;
            let mut w = File::create(&out).map_err(read_failed)?;
            let copied = io::copy(&mut f.take(*size), &mut w).map_err(read_failed)?;
            if copied != *size {
                return Err(read_failed(format!("{name}: size mismatch")));
            }
            files.push(name.clone());
        }
        Ok(files)
    };
    match extract(&mut z) {
        Ok(files) => Ok(StagedArchive { manifest, root: staging.to_path_buf(), files }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(staging);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn manifest(kind: ArchiveKind) -> Manifest {
        Manifest {
            format: FORMAT_VERSION,
            app_version: "0.1.0".into(),
            kind,
            module: None,
            profile_name: None,
            created_unix: 1,
        }
    }

    /// Builds a "dirty" zip by hand (bypassing ArchiveWriter) to test malicious files.
    fn raw_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            z.start_file(*name, opt).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    fn manifest_bytes(format: u32) -> Vec<u8> {
        format!("format = {format}\napp_version = \"0.1.0\"\nkind = \"all\"\ncreated_unix = 1\n").into_bytes()
    }

    #[test]
    fn archive_roundtrip_all() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("all.sgdecor");
        let mut w = ArchiveWriter::create(&dest).unwrap();
        w.add_bytes("app/app.toml", b"reduce_motion = true\n").unwrap();
        w.add_bytes("modules/cursor/config.toml", b"size = 48\n").unwrap();
        w.finish(&manifest(ArchiveKind::All)).unwrap();
        assert!(dest.is_file());
        assert!(!d.path().join("all.sgdecor.tmp").exists());

        let staged = read_archive(&dest, &d.path().join("stage"), Limits::default()).unwrap();
        assert_eq!(staged.manifest, manifest(ArchiveKind::All));
        let mut files = staged.files.clone();
        files.sort();
        assert_eq!(files, ["app/app.toml", "modules/cursor/config.toml"]);
        assert_eq!(std::fs::read(staged.root.join("modules/cursor/config.toml")).unwrap(), b"size = 48\n");
    }

    #[test]
    fn normalize_cases() {
        assert_eq!(normalize_entry_name("modules\\cursor\\config.toml").unwrap(), "modules/cursor/config.toml");
        assert_eq!(normalize_entry_name("manifest.toml").unwrap(), "manifest.toml");
        assert_eq!(
            normalize_entry_name("modules/cursor/profiles/Mặc định.toml").unwrap(),
            "modules/cursor/profiles/Mặc định.toml"
        );
        assert_eq!(normalize_entry_name("modules/wallpaper/assets/a/b.png").unwrap(), "modules/wallpaper/assets/a/b.png");
        for bad in [
            "../evil",
            "modules/../../x",
            "/etc/passwd",
            "C:/x",
            "c:x",
            "\\\\srv\\x",
            "modules//config.toml",
            "modules/abc/config.toml",
            "modules/cursor/other.toml",
            "modules/cursor/profiles/../config.toml",
            "other.txt",
            "app/other.toml",
        ] {
            assert!(normalize_entry_name(bad).is_err(), "phải từ chối {bad:?}");
        }
    }

    #[test]
    fn archive_zip_slip_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("evil.sgdecor");
        raw_zip(&src, &[("manifest.toml", &manifest_bytes(1)), ("../../evil.txt", b"x")]);
        let stage = d.path().join("a").join("b").join("stage");
        let e = read_archive(&src, &stage, Limits::default()).unwrap_err();
        assert_eq!(e.key, "err.archive.unsafe_path");
        assert!(!d.path().join("evil.txt").exists());
        assert!(!d.path().join("a").join("evil.txt").exists());
        assert!(!stage.exists(), "thư mục tạm phải bị xoá");
    }

    #[test]
    fn archive_backslash_names_are_read() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("bs.sgdecor");
        raw_zip(&src, &[("manifest.toml", &manifest_bytes(1)), ("modules\\cursor\\config.toml", b"a = 1\n")]);
        let staged = read_archive(&src, &d.path().join("stage"), Limits::default()).unwrap();
        assert_eq!(staged.files, ["modules/cursor/config.toml"]);
    }

    #[test]
    fn archive_limits() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("many.sgdecor");
        let names: Vec<String> = (0..5).map(|i| format!("modules/cursor/profiles/p{i}.toml")).collect();
        let mut entries: Vec<(&str, &[u8])> = vec![("manifest.toml", b"")];
        let mb = manifest_bytes(1);
        entries[0].1 = &mb;
        for n in &names {
            entries.push((n.as_str(), b"x"));
        }
        raw_zip(&src, &entries);
        let few = Limits { max_entries: 3, max_total_bytes: 1 << 20 };
        assert_eq!(read_archive(&src, &d.path().join("s1"), few).unwrap_err().key, "err.archive.too_big");

        let big = d.path().join("big.sgdecor");
        let blob = vec![b'a'; 4096];
        raw_zip(&big, &[("manifest.toml", &mb), ("modules/cursor/config.toml", &blob)]);
        let small = Limits { max_entries: 100, max_total_bytes: 1000 };
        assert_eq!(read_archive(&big, &d.path().join("s2"), small).unwrap_err().key, "err.archive.too_big");
    }

    #[test]
    fn archive_newer_format_and_missing_manifest() {
        let d = tempfile::tempdir().unwrap();
        let newer = d.path().join("n.sgdecor");
        raw_zip(&newer, &[("manifest.toml", &manifest_bytes(2))]);
        assert_eq!(read_archive(&newer, &d.path().join("s1"), Limits::default()).unwrap_err().key, "err.archive.newer_format");

        let nomani = d.path().join("m.sgdecor");
        raw_zip(&nomani, &[("app/app.toml", b"x")]);
        assert_eq!(read_archive(&nomani, &d.path().join("s2"), Limits::default()).unwrap_err().key, "err.archive.no_manifest");

        let notzip = d.path().join("z.sgdecor");
        std::fs::write(&notzip, b"day khong phai zip").unwrap();
        assert_eq!(read_archive(&notzip, &d.path().join("s3"), Limits::default()).unwrap_err().key, "err.archive.not_zip");
    }

    #[test]
    fn archive_writer_atomic() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("x.sgdecor");
        {
            let mut w = ArchiveWriter::create(&dest).unwrap();
            w.add_bytes("app/app.toml", b"x").unwrap();
            // dropped without finish
        }
        assert!(!dest.exists());
    }

    #[test]
    fn archive_writer_rejects_bad_names() {
        let d = tempfile::tempdir().unwrap();
        let mut w = ArchiveWriter::create(&d.path().join("x.sgdecor")).unwrap();
        assert!(w.add_bytes("../x", b"x").is_err());
    }
}
