//! Backs up Windows' original cursors before the first apply, so turning the part off restores everything exactly.

use crate::Role;
use serde::{Deserialize, Serialize};
use sg_core::config::write_atomic;
use sg_core::paths::Paths;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// Prefix of the scheme name the app writes into the registry's default value.
pub const SCHEME_PREFIX: &str = "ShunguangDecor - ";

/// Every value the app touches in `HKCU\Control Panel\Cursors`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    /// The `(Default)` value = scheme name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme_source: Option<u32>,
    /// `CursorBaseSize`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_size: Option<u32>,
    /// Roles that currently have a value (including an empty string = default cursor), copied verbatim, without expanding `%variables%`.
    #[serde(default)]
    pub roles: BTreeMap<Role, String>,
}

impl RegistrySnapshot {
    /// Windows' default cursors: every role left empty, `Scheme Source` = 0.
    pub fn windows_default() -> RegistrySnapshot {
        RegistrySnapshot { scheme_name: None, scheme_source: Some(0), base_size: None, roles: BTreeMap::new() }
    }

    /// Does the registry point into the app's cursor folder (or carry the app's scheme name)?
    pub fn is_ours(&self, cursors_root: &Path) -> bool {
        let root = norm(&cursors_root.to_string_lossy());
        let under = |p: &str| {
            let p = norm(p);
            p.strip_prefix(&root).is_some_and(|rest| rest.starts_with('\\'))
        };
        self.roles.values().any(|v| under(v)) || self.scheme_name.as_deref().is_some_and(|n| n.starts_with(SCHEME_PREFIX))
    }
}

fn norm(s: &str) -> String {
    s.trim().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

pub fn backup_file(paths: &Paths) -> PathBuf {
    paths.backup_dir().join("cursor-original.toml")
}

/// If a usable backup already exists, do nothing. If the registry points at the app's own scheme (the backup was
/// deleted by hand, or the previous run crashed midway), do NOT take it as the original: save Windows' default cursors.
pub fn ensure_backup(
    paths: &Paths,
    cursors_root: &Path,
    read: impl Fn() -> io::Result<RegistrySnapshot>,
) -> io::Result<()> {
    if matches!(take_backup(paths), Ok(Some(_))) {
        return Ok(());
    }
    let current = read()?;
    // A `CursorBaseSize` written by the app cannot be told apart from the user's own, so drop it here.
    let snapshot = if current.is_ours(cursors_root) { RegistrySnapshot::windows_default() } else { current };
    let text = toml::to_string_pretty(&snapshot).map_err(io::Error::other)?;
    write_atomic(&backup_file(paths), text.as_bytes())
}

pub fn take_backup(paths: &Paths) -> io::Result<Option<RegistrySnapshot>> {
    let text = match std::fs::read_to_string(backup_file(paths)) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    toml::from_str(sg_core::config::strip_bom(&text))
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn clear(paths: &Paths) {
    let _ = std::fs::remove_file(backup_file(paths));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn paths() -> (tempfile::TempDir, Paths) {
        let tmp = tempfile::tempdir().unwrap();
        let p = Paths { roaming: tmp.path().join("r"), local: tmp.path().join("l"), portable: false };
        (tmp, p)
    }

    fn original() -> RegistrySnapshot {
        let mut roles = BTreeMap::new();
        roles.insert(Role::Arrow, r"%SystemRoot%\cursors\aero_arrow.cur".to_string());
        roles.insert(Role::Help, String::new());
        RegistrySnapshot { scheme_name: Some("Windows Default".into()), scheme_source: Some(2), base_size: Some(48), roles }
    }

    #[test]
    fn backup_written_once() {
        let (_t, p) = paths();
        let root = p.local.join("cursors");
        let calls = Cell::new(0);
        let read = || {
            calls.set(calls.get() + 1);
            Ok(original())
        };
        ensure_backup(&p, &root, read).unwrap();
        ensure_backup(&p, &root, || Ok(RegistrySnapshot::windows_default())).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(take_backup(&p).unwrap().unwrap(), original());
        clear(&p);
        assert_eq!(take_backup(&p).unwrap(), None);
    }

    #[test]
    fn backup_ignores_our_own_files() {
        let (_t, p) = paths();
        let root = p.local.join("cursors");
        let mut ours = original();
        ours.roles.insert(Role::Hand, root.join("abc-123456").join("sized").join("32-sharp").join("Hand.cur").to_string_lossy().into_owned());
        ensure_backup(&p, &root, || Ok(ours.clone())).unwrap();
        assert_eq!(take_backup(&p).unwrap().unwrap(), RegistrySnapshot::windows_default());
        // Having only the app's scheme name also counts as the app's.
        clear(&p);
        let named = RegistrySnapshot { scheme_name: Some("ShunguangDecor - X".into()), ..original() };
        ensure_backup(&p, &root, || Ok(named.clone())).unwrap();
        assert_eq!(take_backup(&p).unwrap().unwrap(), RegistrySnapshot::windows_default());
    }

    #[test]
    fn is_ours_only_matches_whole_folder() {
        let (_t, p) = paths();
        let root = p.local.join("cursors");
        let mut s = original();
        s.roles.insert(Role::Hand, format!("{}-other\\x.cur", root.to_string_lossy()));
        assert!(!s.is_ours(&root));
    }

    #[test]
    fn corrupt_backup_is_replaced() {
        let (_t, p) = paths();
        let root = p.local.join("cursors");
        std::fs::create_dir_all(p.backup_dir()).unwrap();
        std::fs::write(backup_file(&p), b"\xff\xfe not toml").unwrap();
        ensure_backup(&p, &root, || Ok(original())).unwrap();
        assert_eq!(take_backup(&p).unwrap().unwrap(), original());
    }
}
