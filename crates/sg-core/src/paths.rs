//! Data paths of the app.
//!
//! - roaming (`%APPDATA%\ShunguangDecor`): config, profiles, backups.
//! - local (`%LOCALAPPDATA%\ShunguangDecor`): logs, state, temp files, cursors, wallpapers.
//! - With a `portable.txt` next to the exe → both are `<exe folder>\data`.

use crate::ids::ModuleId;
use std::path::{Path, PathBuf};

pub const APP_DIR_NAME: &str = "ShunguangDecor";
pub const PORTABLE_MARKER: &str = "portable.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub roaming: PathBuf,
    pub local: PathBuf,
    pub portable: bool,
}

impl Paths {
    /// Computes paths from known inputs (a pure function, for tests).
    /// `roaming_base`/`local_base` are the OS base folders (without the app name);
    /// when missing, the exe folder is used.
    pub fn resolve(
        exe_dir: &Path,
        portable_marker: bool,
        roaming_base: Option<PathBuf>,
        local_base: Option<PathBuf>,
    ) -> Paths {
        if portable_marker {
            let data = exe_dir.join("data");
            return Paths { roaming: data.clone(), local: data, portable: true };
        }
        let fallback = exe_dir.join("data");
        let roaming = roaming_base.map(|b| b.join(APP_DIR_NAME)).unwrap_or_else(|| fallback.clone());
        let local = local_base.map(|b| b.join(APP_DIR_NAME)).unwrap_or(fallback);
        Paths { roaming, local, portable: false }
    }

    /// Finds the real paths on this machine. Also returns a list of warnings to log.
    /// If portable mode is requested but the `data` folder is not writable (e.g. the exe is in Program Files),
    /// it falls back to the normal paths instead of preventing the app from starting.
    pub fn detect() -> (Paths, Vec<String>) {
        let mut warnings = Vec::new();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let marker = exe_dir.join(PORTABLE_MARKER).is_file();
        let roaming = dirs::config_dir();
        let local = dirs::data_local_dir();
        if marker {
            let p = Paths::resolve(&exe_dir, true, None, None);
            if is_writable_dir(&p.roaming) {
                return (p, warnings);
            }
            warnings.push(format!(
                "portable.txt found but {} is not writable; using the normal data folders",
                p.roaming.display()
            ));
        }
        (Paths::resolve(&exe_dir, false, roaming, local), warnings)
    }

    pub fn config_dir(&self) -> PathBuf {
        self.roaming.join("config")
    }
    pub fn app_config_file(&self) -> PathBuf {
        self.config_dir().join("app.toml")
    }
    pub fn module_config_file(&self, id: ModuleId) -> PathBuf {
        self.config_dir().join(format!("{}.toml", id.as_str()))
    }
    pub fn profiles_root(&self) -> PathBuf {
        self.roaming.join("profiles")
    }
    pub fn profiles_dir(&self, id: ModuleId) -> PathBuf {
        self.profiles_root().join(id.as_str())
    }
    pub fn backup_dir(&self) -> PathBuf {
        self.roaming.join("backup")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.local.join("logs")
    }
    pub fn state_dir(&self) -> PathBuf {
        self.local.join("state")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.local.join("tmp")
    }
    pub fn guard_journal_file(&self) -> PathBuf {
        self.state_dir().join("guard.toml")
    }
}

/// Tries to create the folder and write a small file to find out whether it is writable.
fn is_writable_dir(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".sg-write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_portable_uses_exe_data() {
        let p = Paths::resolve(
            Path::new("C:/x"),
            true,
            Some(PathBuf::from("C:/r")),
            Some(PathBuf::from("C:/l")),
        );
        assert_eq!(p.roaming, PathBuf::from("C:/x/data"));
        assert_eq!(p.local, PathBuf::from("C:/x/data"));
        assert!(p.portable);
    }

    #[test]
    fn paths_normal_appends_app_name() {
        let p = Paths::resolve(
            Path::new("C:/x"),
            false,
            Some(PathBuf::from("C:/R")),
            Some(PathBuf::from("C:/L")),
        );
        assert_eq!(p.roaming, PathBuf::from("C:/R").join("ShunguangDecor"));
        assert_eq!(p.local, PathBuf::from("C:/L").join("ShunguangDecor"));
        assert!(!p.portable);
        assert!(p.module_config_file(ModuleId::Cursor).ends_with("config/cursor.toml"));
        assert!(p.logs_dir().starts_with(&p.local));
        assert!(p.profiles_dir(ModuleId::Taskbar).ends_with("profiles/taskbar"));
    }
}
