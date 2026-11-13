//! "Export all / Import all" in Settings: every configuration + profile in a single `.sgdecor` file.

use crate::archive::{ArchiveKind, ArchiveWriter, FORMAT_VERSION, Limits, Manifest, read_archive};
use crate::config::{self, AppConfig};
use crate::i18n::Lang;
use crate::ids::ModuleId;
use crate::paths::Paths;
use crate::profiles::ProfileStore;
use crate::text::I18nText;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// The part of `app.toml` that travels with the file (excludes machine-specific state such as autostart).
#[derive(Serialize)]
struct PortableSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<Lang>,
    reduce_motion: bool,
    follow_os_motion: bool,
    modules_enabled: BTreeMap<ModuleId, bool>,
}

#[derive(Debug)]
pub struct ImportOutcome {
    /// Number of configuration files imported.
    pub files: u32,
    /// General settings in the file (read fault-tolerantly like `app.toml`); the caller merges them into the running config.
    pub settings: Option<AppConfig>,
    /// Where the old configuration was backed up.
    pub backup: PathBuf,
}

fn io_err(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("err.io").arg("detail", e)
}

pub fn export_all(paths: &Paths, cfg: &AppConfig, dest: &Path, app_version: &str) -> io::Result<()> {
    export_all_with(paths, cfg, dest, app_version, |_| Ok(()))
}

/// Like `export_all`, but lets a part add assets (wallpaper images, ...) to the file before it is closed.
pub fn export_all_with(
    paths: &Paths,
    cfg: &AppConfig,
    dest: &Path,
    app_version: &str,
    extra: impl FnOnce(&mut ArchiveWriter) -> io::Result<()>,
) -> io::Result<()> {
    let portable = PortableSettings {
        language: cfg.language,
        reduce_motion: cfg.reduce_motion,
        follow_os_motion: cfg.follow_os_motion,
        modules_enabled: cfg.modules_enabled.clone(),
    };
    let store = ProfileStore::new(paths.clone());
    let mut w = ArchiveWriter::create(dest)?;
    w.add_bytes("app/app.toml", toml::to_string_pretty(&portable).map_err(io::Error::other)?.as_bytes())?;
    for id in ModuleId::ALL {
        let cfg_file = paths.module_config_file(id);
        if cfg_file.is_file() {
            w.add_file(&format!("modules/{id}/config.toml"), &cfg_file)?;
        }
        for p in store.list(id) {
            let file = store.dir(id).join(format!("{}.toml", p.name));
            w.add_file(&format!("modules/{id}/profiles/{}.toml", p.name), &file)?;
        }
    }
    extra(&mut w)?;
    w.finish(&Manifest {
        format: FORMAT_VERSION,
        app_version: app_version.to_string(),
        kind: ArchiveKind::All,
        module: None,
        profile_name: None,
        created_unix: now_unix(),
    })
}

/// A suffix unique within a process (seconds + counter + pid): two imports in the same second,
/// or a double click, never share a temp or backup directory.
fn unique_suffix(now: u64) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{now}-{}-{n}", std::process::id())
}

/// A dedicated temp directory for one extraction of an import file.
pub fn staging_dir(paths: &Paths) -> std::path::PathBuf {
    paths.tmp_dir().join(format!("import-{}", unique_suffix(now_unix())))
}

/// Removes leftover temp directories (the app was killed mid-import). Call at startup, before any import.
pub fn purge_tmp(paths: &Paths) {
    let dir = paths.tmp_dir();
    if dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&dir)
    {
        tracing::warn!(target: "sg::core", "cannot purge {}: {e}", dir.display());
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}

/// Imports an "Export all" file: validate the file (safe, correct type) → back up the current `config/` and `profiles/`
/// into `backup/before-import-<now>/` → copy the new configuration in. A bad file changes nothing.
pub fn import_all(paths: &Paths, src: &Path, now: u64) -> Result<ImportOutcome, I18nText> {
    import_all_with(paths, src, now, |_, _| Ok(()))
}

/// Like `import_all`; `asset(path in the file, extracted file)` is called for each `modules/<part>/assets/…` file.
pub fn import_all_with(
    paths: &Paths,
    src: &Path,
    now: u64,
    mut asset: impl FnMut(&str, &Path) -> io::Result<()>,
) -> Result<ImportOutcome, I18nText> {
    let staging = paths.tmp_dir().join(format!("import-{}", unique_suffix(now)));
    let staged = read_archive(src, &staging, Limits::default())?;
    let result = (|| {
        if staged.manifest.kind != ArchiveKind::All {
            return Err(I18nText::new("err.archive.wrong_kind"));
        }
        let backup = paths.backup_dir().join(format!("before-import-{}", unique_suffix(now)));
        copy_dir(&paths.config_dir(), &backup.join("config")).map_err(io_err)?;
        copy_dir(&paths.profiles_root(), &backup.join("profiles")).map_err(io_err)?;

        let mut files = 0u32;
        let mut settings = None;
        for rel in &staged.files {
            let from = staged.root.join(rel);
            let parts: Vec<&str> = rel.split('/').collect();
            let to = match parts.as_slice() {
                ["app", "app.toml"] => {
                    let r = config::load(&from);
                    for w in &r.warnings {
                        tracing::warn!(target: "sg::app", "import app.toml: {w}");
                    }
                    settings = Some(r.config);
                    files += 1;
                    continue;
                }
                ["modules", m, "config.toml"] => ModuleId::parse(m).map(|id| paths.module_config_file(id)),
                ["modules", m, "profiles", file] => ModuleId::parse(m).map(|id| paths.profiles_dir(id).join(file)),
                // Assets (wallpaper images, ...): the part that owns them copies them itself.
                ["modules", _, "assets", ..] => {
                    asset(rel, &from).map_err(io_err)?;
                    continue;
                }
                _ => None,
            };
            let Some(to) = to else { continue };
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(io_err)?;
            }
            std::fs::copy(&from, &to).map_err(io_err)?;
            files += 1;
        }
        Ok(ImportOutcome { files, settings, backup })
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{ArchiveKind, ArchiveWriter, FORMAT_VERSION, Manifest};
    use crate::config::AppConfig;
    use crate::i18n::Lang;
    use crate::ids::ModuleId;
    use crate::paths::Paths;
    use std::path::Path;

    fn paths(dir: &Path) -> Paths {
        Paths::resolve(dir, true, None, None)
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn sample_config() -> AppConfig {
        AppConfig {
            language: Some(Lang::Vi),
            reduce_motion: true,
            autostart_initialized: true,
            tray_notice_shown: true,
            modules_enabled: [(ModuleId::Cursor, true)].into(),
            ..AppConfig::default()
        }
    }

    #[test]
    fn transfer_roundtrip() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let pa = paths(a.path());
        let pb = paths(b.path());
        write(&pa.module_config_file(ModuleId::Cursor), "size = 48\n");
        write(&pa.profiles_dir(ModuleId::Cursor).join("Mặc định.toml"), "size = 64\n");
        let dest = a.path().join("all.sgdecor");
        export_all(&pa, &sample_config(), &dest, "0.1.0").unwrap();

        let out = import_all(&pb, &dest, 100).unwrap();
        assert_eq!(out.files, 3);
        assert_eq!(std::fs::read_to_string(pb.module_config_file(ModuleId::Cursor)).unwrap(), "size = 48\n");
        assert_eq!(std::fs::read_to_string(pb.profiles_dir(ModuleId::Cursor).join("Mặc định.toml")).unwrap(), "size = 64\n");
        let s = out.settings.expect("có app/app.toml");
        assert_eq!(s.language, Some(Lang::Vi));
        assert!(s.reduce_motion);
        assert_eq!(s.modules_enabled.get(&ModuleId::Cursor), Some(&true));
        // Machine-specific state does not travel with the file.
        assert!(!s.autostart_initialized && !s.tray_notice_shown);
        // The temp directory has been cleaned up.
        let leftovers = std::fs::read_dir(pb.tmp_dir()).map(|r| r.count()).unwrap_or(0);
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn import_backs_up_existing() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (pa, pb) = (paths(a.path()), paths(b.path()));
        write(&pa.module_config_file(ModuleId::Cursor), "new = 1\n");
        write(&pb.module_config_file(ModuleId::Cursor), "old = 1\n");
        write(&pb.profiles_dir(ModuleId::Taskbar).join("cũ.toml"), "x = 1\n");
        let dest = a.path().join("all.sgdecor");
        export_all(&pa, &AppConfig::default(), &dest, "0.1.0").unwrap();
        let out = import_all(&pb, &dest, 777).unwrap();
        assert_eq!(out.backup.parent(), Some(pb.backup_dir().as_path()));
        assert!(out.backup.file_name().unwrap().to_string_lossy().starts_with("before-import-777-"));
        // Importing again within the same second → a different backup directory, the previous backup is not overwritten.
        let again = import_all(&pb, &dest, 777).unwrap();
        assert_ne!(again.backup, out.backup);
        assert_eq!(std::fs::read_to_string(out.backup.join("config").join("cursor.toml")).unwrap(), "old = 1\n");
        assert!(out.backup.join("profiles").join("taskbar").join("cũ.toml").is_file());
        assert_eq!(std::fs::read_to_string(pb.module_config_file(ModuleId::Cursor)).unwrap(), "new = 1\n");
    }

    #[test]
    fn import_rejects_single_profile_file() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let pb = paths(b.path());
        write(&pb.module_config_file(ModuleId::Cursor), "old = 1\n");
        let dest = a.path().join("one.sgdecor");
        let mut w = ArchiveWriter::create(&dest).unwrap();
        w.add_bytes("modules/cursor/config.toml", b"new = 1\n").unwrap();
        w.finish(&Manifest {
            format: FORMAT_VERSION,
            app_version: "0.1.0".into(),
            kind: ArchiveKind::Module,
            module: Some(ModuleId::Cursor),
            profile_name: Some("A".into()),
            created_unix: 1,
        })
        .unwrap();
        let e = import_all(&pb, &dest, 1).unwrap_err();
        assert_eq!(e.key, "err.archive.wrong_kind");
        assert_eq!(std::fs::read_to_string(pb.module_config_file(ModuleId::Cursor)).unwrap(), "old = 1\n");
        assert!(!pb.backup_dir().exists());
    }
}
