//! `app.toml`: app-wide configuration.
//!
//! Loading is always fault-tolerant: missing key → default; wrong type → warning + default for that key only;
//! completely broken file → keep a copy as `app.toml.broken-<unix>` and run with defaults.
//! The app must never fail to start just because of its config file.

use crate::i18n::Lang;
use crate::ids::ModuleId;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AppConfig {
    /// `None` until the first launch (then chosen from the OS language).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<Lang>,
    /// Whether "Start with Windows" has been enabled on the first run (on by default).
    pub autostart_initialized: bool,
    pub reduce_motion: bool,
    /// Follow the OS "turn off animations" setting. Off by default: many "PC optimizer" apps switch off
    /// Windows animation effects without the user knowing, and the app would look frozen.
    pub follow_os_motion: bool,
    /// Whether the "still running in the system tray" notice was shown on the first window close.
    pub tray_notice_shown: bool,
    pub modules_enabled: BTreeMap<ModuleId, bool>,
    /// Currently selected profile of each part.
    pub active_profiles: BTreeMap<ModuleId, String>,
}

#[derive(Debug)]
pub struct LoadReport {
    pub config: AppConfig,
    /// Warning lines to write to the log (English).
    pub warnings: Vec<String>,
    /// A completely broken file is copied here before falling back to defaults.
    pub broken_backup: Option<PathBuf>,
    /// The file exists but cannot be read, or is broken and could not be copied aside: this session must NOT
    /// write `app.toml`, or the user's configuration would be overwritten with defaults.
    pub must_not_overwrite: bool,
}

/// Strips the UTF-8 BOM that Notepad likes to add.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

pub fn load(path: &Path) -> LoadReport {
    let mut warnings = Vec::new();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return LoadReport { config: AppConfig::default(), warnings, broken_backup: None, must_not_overwrite: false };
        }
        Err(e) => {
            warnings.push(format!("cannot read {}: {e}; using defaults and leaving the file untouched", path.display()));
            return LoadReport { config: AppConfig::default(), warnings, broken_backup: None, must_not_overwrite: true };
        }
    };
    let table = match std::str::from_utf8(&bytes)
        .map_err(|e| e.to_string())
        .and_then(|t| strip_bom(t).parse::<toml::Table>().map_err(|e| e.to_string()))
    {
        Ok(t) => t,
        Err(e) => {
            let backup = backup_broken(path, &bytes);
            warnings.push(format!(
                "{} is not valid TOML ({e}); using defaults, copy kept at {}",
                path.display(),
                backup.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<copy failed>".into())
            ));
            let must_not_overwrite = backup.is_none();
            return LoadReport { config: AppConfig::default(), warnings, broken_backup: backup, must_not_overwrite };
        }
    };

    let d = AppConfig::default();
    let language = match table.get("language") {
        None => None,
        Some(toml::Value::String(s)) => match Lang::from_code(s) {
            Some(l) => Some(l),
            None => {
                warnings.push(format!("language: unknown code {s:?}, ignored"));
                None
            }
        },
        Some(v) => {
            warnings.push(format!("language: expected a string, got {}", v.type_str()));
            None
        }
    };
    let config = AppConfig {
        language,
        autostart_initialized: field(&table, "autostart_initialized", d.autostart_initialized, &mut warnings),
        reduce_motion: field(&table, "reduce_motion", d.reduce_motion, &mut warnings),
        follow_os_motion: field(&table, "follow_os_motion", d.follow_os_motion, &mut warnings),
        tray_notice_shown: field(&table, "tray_notice_shown", d.tray_notice_shown, &mut warnings),
        modules_enabled: module_map(&table, "modules_enabled", &mut warnings),
        active_profiles: module_map(&table, "active_profiles", &mut warnings),
    };
    LoadReport { config, warnings, broken_backup: None, must_not_overwrite: false }
}

pub fn save(path: &Path, cfg: &AppConfig) -> io::Result<()> {
    let text = toml::to_string_pretty(cfg).map_err(io::Error::other)?;
    write_atomic(path, text.as_bytes())
}

/// Writes `<file>.<pid>-<n>.tmp` then renames it over the real file: never leaves a half-written file behind.
/// Each call gets its own temp name: Tauri commands run in parallel (dragging a slider sends a new write before the
/// previous one finishes), and sharing one `<file>.tmp` would let one write rename away the other's temp file → "file not found".
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{n}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Reads one key; on a wrong type, warns and uses the default for that key only.
fn field<T: DeserializeOwned>(t: &toml::Table, key: &str, default: T, warn: &mut Vec<String>) -> T {
    match t.get(key) {
        None => default,
        Some(v) => match v.clone().try_into::<T>() {
            Ok(x) => x,
            Err(_) => {
                warn.push(format!("{key}: unexpected type {}, using default", v.type_str()));
                default
            }
        },
    }
}

/// A `ModuleId → T` table: unknown keys or wrongly typed values are dropped pair by pair, the rest is kept.
fn module_map<T: DeserializeOwned>(t: &toml::Table, key: &str, warn: &mut Vec<String>) -> BTreeMap<ModuleId, T> {
    let mut out = BTreeMap::new();
    let Some(v) = t.get(key) else { return out };
    let Some(sub) = v.as_table() else {
        warn.push(format!("{key}: expected a table, got {}", v.type_str()));
        return out;
    };
    for (k, val) in sub {
        let Some(id) = ModuleId::parse(k) else {
            warn.push(format!("{key}.{k}: unknown part, ignored"));
            continue;
        };
        match val.clone().try_into::<T>() {
            Ok(x) => {
                out.insert(id, x);
            }
            Err(_) => warn.push(format!("{key}.{k}: unexpected type {}, ignored", val.type_str())),
        }
    }
    out
}

fn backup_broken(path: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = format!("{}.broken-{secs}", path.file_name()?.to_string_lossy());
    let backup = path.with_file_name(name);
    std::fs::write(&backup, bytes).ok()?;
    Some(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;
    use crate::ids::ModuleId;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn config_missing_file_gives_defaults() {
        let d = tmp();
        let r = load(&d.path().join("app.toml"));
        assert_eq!(r.config, AppConfig::default());
        assert!(r.warnings.is_empty());
        assert_eq!(r.broken_backup, None);
    }

    #[test]
    fn config_roundtrip() {
        let d = tmp();
        let path = d.path().join("config").join("app.toml");
        let c = AppConfig {
            language: Some(Lang::Vi),
            reduce_motion: true,
            follow_os_motion: true,
            modules_enabled: [(ModuleId::Cursor, true)].into(),
            active_profiles: [(ModuleId::Cursor, "Mặc định của tôi".to_string())].into(),
            ..AppConfig::default()
        };
        save(&path, &c).unwrap();
        let r = load(&path);
        assert_eq!(r.config, c);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn config_strips_utf8_bom() {
        let d = tmp();
        let path = d.path().join("app.toml");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"reduce_motion = true\n");
        std::fs::write(&path, bytes).unwrap();
        let r = load(&path);
        assert!(r.config.reduce_motion);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn config_wrong_type_keeps_other_keys() {
        let d = tmp();
        let path = d.path().join("app.toml");
        std::fs::write(&path, "language = 5\nreduce_motion = true\n").unwrap();
        let r = load(&path);
        assert_eq!(r.config.language, None);
        assert!(r.config.reduce_motion);
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].contains("language"));
    }

    #[test]
    fn config_unknown_language_code_is_none() {
        let d = tmp();
        let path = d.path().join("app.toml");
        std::fs::write(&path, "language = \"xx\"\n").unwrap();
        let r = load(&path);
        assert_eq!(r.config.language, None);
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn config_unknown_module_key_ignored() {
        let d = tmp();
        let path = d.path().join("app.toml");
        std::fs::write(&path, "[modules_enabled]\ncursor = true\nabc = true\n").unwrap();
        let r = load(&path);
        assert_eq!(r.config.modules_enabled.len(), 1);
        assert_eq!(r.config.modules_enabled.get(&ModuleId::Cursor), Some(&true));
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    }

    #[test]
    fn config_garbage_file_backed_up() {
        let d = tmp();
        let path = d.path().join("app.toml");
        std::fs::write(&path, b"[[[\x00").unwrap();
        let r = load(&path);
        assert_eq!(r.config, AppConfig::default());
        let backup = r.broken_backup.expect("phải có bản sao file hỏng");
        assert!(backup.file_name().unwrap().to_string_lossy().starts_with("app.toml.broken-"));
        assert_eq!(std::fs::read(&backup).unwrap(), b"[[[\x00");
        // The original file stays intact until the next save.
        assert_eq!(std::fs::read(&path).unwrap(), b"[[[\x00");
        assert!(!r.warnings.is_empty());
        // A copy exists, so overwriting is allowed.
        assert!(!r.must_not_overwrite);
    }

    #[test]
    fn config_unreadable_file_must_not_be_overwritten() {
        // The path is a directory → read fails but not with NotFound (like a locked / permission-denied file).
        let d = tmp();
        let path = d.path().join("app.toml");
        std::fs::create_dir(&path).unwrap();
        let r = load(&path);
        assert_eq!(r.config, AppConfig::default());
        assert!(r.must_not_overwrite);
        assert!(!r.warnings.is_empty());
    }

    #[test]
    fn config_missing_file_may_be_written() {
        let d = tmp();
        assert!(!load(&d.path().join("app.toml")).must_not_overwrite);
    }

    #[test]
    fn config_empty_file_is_defaults() {
        let d = tmp();
        let path = d.path().join("app.toml");
        std::fs::write(&path, "").unwrap();
        let r = load(&path);
        assert_eq!(r.config, AppConfig::default());
        assert!(r.warnings.is_empty());
        assert_eq!(r.broken_backup, None);
    }

    #[test]
    fn write_atomic_replaces_existing() {
        let d = tmp();
        let path = d.path().join("sub").join("f.toml");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn write_atomic_concurrent_same_file() {
        let d = tmp();
        let path = std::sync::Arc::new(d.path().join("c.toml"));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for j in 0..50 {
                        write_atomic(&path, format!("{i}-{j}").as_bytes()).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let leftovers = std::fs::read_dir(d.path()).unwrap().filter_map(|e| e.ok()).count();
        assert_eq!(leftovers, 1);
    }
}
