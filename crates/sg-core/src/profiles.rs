//! Saved profiles for each module: `profiles/<module>/<name>.toml`.

use crate::config::write_atomic;
use crate::ids::ModuleId;
use crate::paths::Paths;
use crate::text::I18nText;
use serde::Serialize;
use std::io;
use std::path::PathBuf;

pub const MAX_NAME_CHARS: usize = 64;
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileMeta {
    pub name: String,
    pub modified_unix: u64,
}

/// A profile name is used as a file name, so it must be valid on Windows.
/// Unicode text (Vietnamese, CJK, emoji) is fine. Returns the name with surrounding whitespace trimmed.
pub fn validate_name(raw: &str) -> Result<String, I18nText> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(I18nText::new("err.profile.empty"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(I18nText::new("err.profile.too_long").arg("max", MAX_NAME_CHARS));
    }
    if name.chars().any(|c| c.is_control() || FORBIDDEN.contains(&c)) {
        return Err(I18nText::new("err.profile.bad_char").arg("chars", "< > : \" / \\ | ? *"));
    }
    if name.ends_with('.') {
        return Err(I18nText::new("err.profile.bad_end"));
    }
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(I18nText::new("err.profile.reserved").arg("name", name));
    }
    Ok(name.to_string())
}

fn io_err(e: io::Error) -> I18nText {
    I18nText::new("err.profile.io").arg("detail", e)
}

pub struct ProfileStore {
    paths: Paths,
}

impl ProfileStore {
    pub fn new(paths: Paths) -> ProfileStore {
        ProfileStore { paths }
    }

    pub fn dir(&self, id: ModuleId) -> PathBuf {
        self.paths.profiles_dir(id)
    }

    fn file(&self, id: ModuleId, name: &str) -> Result<PathBuf, I18nText> {
        let n = validate_name(name)?;
        Ok(self.dir(id).join(format!("{n}.toml")))
    }

    pub fn list(&self, id: ModuleId) -> Vec<ProfileMeta> {
        let Ok(rd) = std::fs::read_dir(self.dir(id)) else { return Vec::new() };
        let mut out: Vec<ProfileMeta> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                if !p.is_file() || p.extension().and_then(|x| x.to_str()) != Some("toml") {
                    return None;
                }
                let name = p.file_stem()?.to_str()?.to_string();
                validate_name(&name).ok()?;
                let modified_unix = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Some(ProfileMeta { name, modified_unix })
            })
            .collect();
        out.sort_by_key(|m| m.name.to_lowercase());
        out
    }

    pub fn save(&self, id: ModuleId, name: &str, toml_text: &str, overwrite: bool) -> Result<(), I18nText> {
        let path = self.file(id, name)?;
        if path.exists() && !overwrite {
            return Err(I18nText::new("err.profile.exists").arg("name", name.trim()));
        }
        write_atomic(&path, toml_text.as_bytes()).map_err(io_err)
    }

    pub fn load(&self, id: ModuleId, name: &str) -> Result<String, I18nText> {
        let path = self.file(id, name)?;
        match std::fs::read(&path) {
            Ok(b) => Ok(crate::config::strip_bom(&String::from_utf8_lossy(&b)).to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(I18nText::new("err.profile.not_found").arg("name", name.trim()))
            }
            Err(e) => Err(io_err(e)),
        }
    }

    pub fn delete(&self, id: ModuleId, name: &str) -> Result<(), I18nText> {
        let path = self.file(id, name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(I18nText::new("err.profile.not_found").arg("name", name.trim()))
            }
            Err(e) => Err(io_err(e)),
        }
    }

    /// An unused name: "A" → "A (2)" → "A (3)"... An invalid name (e.g. taken from an imported file) becomes "Imported".
    pub fn unique_name(&self, id: ModuleId, wanted: &str) -> String {
        let base = validate_name(wanted).unwrap_or_else(|_| "Imported".to_string());
        let exists = |n: &str| self.file(id, n).map(|p| p.exists()).unwrap_or(true);
        if !exists(&base) {
            return base;
        }
        for i in 2.. {
            let suffix = format!(" ({i})");
            let keep = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
            let head: String = base.chars().take(keep).collect();
            let candidate = format!("{}{suffix}", head.trim_end());
            if !exists(&candidate) {
                return candidate;
            }
        }
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn store(dir: &Path) -> ProfileStore {
        ProfileStore::new(Paths::resolve(dir, true, None, None))
    }

    #[test]
    fn name_vietnamese_ok() {
        assert_eq!(validate_name("  Mặc định của tôi ").unwrap(), "Mặc định của tôi");
        assert_eq!(validate_name("夜 光 ✨").unwrap(), "夜 光 ✨");
        assert!(validate_name(&"ư".repeat(64)).is_ok());
    }

    #[test]
    fn name_rejects_bad() {
        let long = "x".repeat(65);
        for bad in [
            "", "   ", "../x", "a\\b", "a/b", "CON", "nul.txt", "Com3", "lpt9.cfg", "tên.", "a:b", "a*b", "a?b",
            "a<b", "a|b", "a\"b", ".", "..", long.as_str(), "a\u{0007}", "a\nb",
        ] {
            assert!(validate_name(bad).is_err(), "phải từ chối {bad:?}");
        }
    }

    #[test]
    fn name_error_has_key() {
        let e = validate_name("CON").unwrap_err();
        assert!(e.key.starts_with("err.profile."), "{}", e.key);
    }

    #[test]
    fn profile_save_list_load_delete() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path());
        s.save(ModuleId::Cursor, "Mặc định của tôi", "size = 48\n", false).unwrap();
        let l = s.list(ModuleId::Cursor);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, "Mặc định của tôi");
        assert_eq!(s.load(ModuleId::Cursor, "Mặc định của tôi").unwrap(), "size = 48\n");
        let e = s.save(ModuleId::Cursor, "Mặc định của tôi", "x", false).unwrap_err();
        assert_eq!(e.key, "err.profile.exists");
        s.save(ModuleId::Cursor, "Mặc định của tôi", "size = 64\n", true).unwrap();
        assert_eq!(s.load(ModuleId::Cursor, "Mặc định của tôi").unwrap(), "size = 64\n");
        s.delete(ModuleId::Cursor, "Mặc định của tôi").unwrap();
        assert!(s.list(ModuleId::Cursor).is_empty());
        // Other modules do not see this module's profiles.
        assert!(s.list(ModuleId::Taskbar).is_empty());
    }

    #[test]
    fn profile_list_sorted_and_ignores_other_files() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path());
        s.save(ModuleId::Cursor, "b", "", false).unwrap();
        s.save(ModuleId::Cursor, "a", "", false).unwrap();
        std::fs::write(s.dir(ModuleId::Cursor).join("ghi chú.txt"), "x").unwrap();
        let names: Vec<String> = s.list(ModuleId::Cursor).into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn profile_bad_name_never_writes_outside() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path());
        assert!(s.save(ModuleId::Cursor, "../../thoat", "x", false).is_err());
        assert!(!d.path().join("thoat.toml").exists());
        assert!(!d.path().join("data").join("thoat.toml").exists());
    }

    #[test]
    fn profile_load_missing() {
        let d = tempfile::tempdir().unwrap();
        let e = store(d.path()).load(ModuleId::Cursor, "không có").unwrap_err();
        assert_eq!(e.key, "err.profile.not_found");
    }

    #[test]
    fn profile_unique_name() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path());
        assert_eq!(s.unique_name(ModuleId::Cursor, "A"), "A");
        s.save(ModuleId::Cursor, "A", "", false).unwrap();
        s.save(ModuleId::Cursor, "A (2)", "", false).unwrap();
        assert_eq!(s.unique_name(ModuleId::Cursor, "A"), "A (3)");
    }
}
