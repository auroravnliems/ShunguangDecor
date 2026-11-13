//! Library of imported cursor schemes: `<root>/<id>/scheme.toml`, `files/`, `sized/<size>-<mode>/`.

use crate::{Animation, Role, ScaleMode, parse_cursor_file, scale, write_ani, write_cur};
use serde::{Deserialize, Serialize};
use sg_core::config::write_atomic;
use sg_core::text::I18nText;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const CACHE_MAX: usize = 64;
const MAX_NAME_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemeSummary {
    pub id: String,
    pub name: String,
    pub role_count: u8,
    pub imported_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleInfo {
    pub file: String,
    pub frames: u32,
    /// How many milliseconds each frame is shown (jiffy × 1000 / 60, minimum 16).
    pub delays_ms: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub hotspot: (u32, u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemeDetail {
    pub id: String,
    pub name: String,
    pub imported_unix: u64,
    pub roles: BTreeMap<Role, RoleInfo>,
    /// Every cursor file in the scheme (to pick a replacement for a role).
    pub files: Vec<String>,
}

/// Contents of `scheme.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SchemeFile {
    pub name: String,
    #[serde(default)]
    pub imported_unix: u64,
    #[serde(default)]
    pub source: String,
    /// Key = the role's registry name, value = file name inside `files/`.
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
}

impl SchemeFile {
    pub(crate) fn role_map(&self) -> BTreeMap<Role, String> {
        self.roles.iter().filter_map(|(k, v)| Role::parse(k).map(|r| (r, v.clone()))).collect()
    }

    pub(crate) fn set_roles(&mut self, roles: &BTreeMap<Role, String>) {
        self.roles = roles.iter().map(|(r, f)| (r.reg_name().to_string(), f.clone())).collect();
    }
}

pub struct Library {
    root: PathBuf,
    cache: Mutex<HashMap<(String, Role), Arc<Animation>>>,
}

pub(crate) fn io_err(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("cursor.err.io").arg("detail", e)
}

/// A valid id: lowercase a–z, digits and `-`, non-empty.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn clean_name(name: &str) -> Option<String> {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    let n = n.trim().to_string();
    (!n.is_empty()).then_some(n)
}

impl Library {
    pub fn new(root: PathBuf) -> Library {
        Library { root, cache: Mutex::new(HashMap::new()) }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn scheme_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn checked_dir(&self, id: &str) -> Result<PathBuf, I18nText> {
        if !valid_id(id) {
            return Err(I18nText::new("cursor.err.scheme_missing"));
        }
        let dir = self.scheme_dir(id);
        if dir.join("scheme.toml").is_file() { Ok(dir) } else { Err(I18nText::new("cursor.err.scheme_missing")) }
    }

    pub(crate) fn load_file(&self, id: &str) -> Result<SchemeFile, I18nText> {
        let dir = self.checked_dir(id)?;
        let text = std::fs::read_to_string(dir.join("scheme.toml")).map_err(io_err)?;
        toml::from_str(sg_core::config::strip_bom(&text)).map_err(io_err)
    }

    pub(crate) fn save_file(&self, id: &str, f: &SchemeFile) -> Result<(), I18nText> {
        let text = toml::to_string_pretty(f).map_err(io_err)?;
        write_atomic(&self.scheme_dir(id).join("scheme.toml"), text.as_bytes()).map_err(io_err)
    }

    pub fn load_name(&self, id: &str) -> Result<String, I18nText> {
        Ok(self.load_file(id)?.name)
    }

    pub fn exists(&self, id: &str) -> bool {
        self.checked_dir(id).is_ok()
    }

    pub fn list(&self) -> Vec<SchemeSummary> {
        let Ok(rd) = std::fs::read_dir(&self.root) else { return Vec::new() };
        let mut out: Vec<SchemeSummary> = rd
            .flatten()
            .filter_map(|e| {
                let id = e.file_name().to_str()?.to_string();
                let f = self.load_file(&id).ok()?;
                Some(SchemeSummary {
                    id,
                    name: f.name,
                    role_count: f.roles.len().min(17) as u8,
                    imported_unix: f.imported_unix,
                })
            })
            .collect();
        out.sort_by(|a, b| a.imported_unix.cmp(&b.imported_unix).then_with(|| a.id.cmp(&b.id)));
        out
    }

    pub fn get(&self, id: &str) -> Result<SchemeDetail, I18nText> {
        let f = self.load_file(id)?;
        let mut roles = BTreeMap::new();
        for (role, file) in f.role_map() {
            match self.read_frames(id, role) {
                Ok(a) => {
                    let first = a.frames.first();
                    roles.insert(
                        role,
                        RoleInfo {
                            file,
                            frames: a.frames.len() as u32,
                            delays_ms: a.delays_jiffies.iter().map(|j| (j * 1000 / 60).max(16)).collect(),
                            width: first.map_or(0, |i| i.width),
                            height: first.map_or(0, |i| i.height),
                            hotspot: first.map_or((0, 0), |i| i.hotspot),
                        },
                    );
                }
                Err(e) => tracing::warn!(target: "sg::cursor", "scheme {id}: role {role:?} unreadable: {e}"),
            }
        }
        Ok(SchemeDetail { id: id.to_string(), name: f.name, imported_unix: f.imported_unix, roles, files: self.file_names(id) })
    }

    fn file_names(&self, id: &str) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.scheme_dir(id).join("files"))
            .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).collect())
            .unwrap_or_default();
        v.sort_by_key(|s| s.to_ascii_lowercase());
        v
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<(), I18nText> {
        let mut f = self.load_file(id)?;
        f.name = clean_name(name).ok_or_else(|| I18nText::new("cursor.err.bad_name"))?;
        self.save_file(id, &f)
    }

    pub fn delete(&self, id: &str) -> Result<(), I18nText> {
        let dir = self.checked_dir(id)?;
        std::fs::remove_dir_all(dir).map_err(io_err)?;
        self.invalidate(id);
        Ok(())
    }

    /// Sets (or clears, with `None`) the file for a role. The file must exist in the scheme's `files/`.
    pub fn set_role(&self, id: &str, role: Role, file: Option<String>) -> Result<(), I18nText> {
        let mut f = self.load_file(id)?;
        let mut roles = f.role_map();
        match file {
            None => {
                roles.remove(&role);
            }
            Some(name) => {
                if !self.file_names(id).contains(&name) {
                    return Err(I18nText::new("cursor.err.role_file_missing"));
                }
                // Only accept it if it can be read, otherwise a broken role would break the whole scheme when applied.
                let bytes = std::fs::read(self.scheme_dir(id).join("files").join(&name)).map_err(io_err)?;
                parse_cursor_file(&bytes).map_err(|e| e.to_text())?;
                roles.insert(role, name);
            }
        }
        f.set_roles(&roles);
        self.save_file(id, &f)?;
        self.invalidate(id);
        Ok(())
    }

    pub(crate) fn invalidate(&self, id: &str) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).retain(|(i, _), _| i != id);
    }

    /// The frames of one role (cache holds at most 64 entries, cleared when the scheme changes).
    pub fn read_frames(&self, id: &str, role: Role) -> Result<Arc<Animation>, I18nText> {
        let key = (id.to_string(), role);
        if let Some(a) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(a.clone());
        }
        let f = self.load_file(id)?;
        let file = f.role_map().remove(&role).ok_or_else(|| I18nText::new("cursor.err.role_empty"))?;
        let bytes = std::fs::read(self.scheme_dir(id).join("files").join(&file)).map_err(io_err)?;
        let anim = Arc::new(parse_cursor_file(&bytes).map_err(|e| e.to_text())?);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, anim.clone());
        Ok(anim)
    }

    /// Generates `.cur`/`.ani` files with exactly one `side × side` image per frame (see [`crate::image_side`]) into
    /// `sized/<side>-<mode>/`. Skipped if `stamp` still matches. Afterwards the other size directories of this scheme are
    /// removed (dragging the size slider used to leave dozens of directories, each up to ~55 MB at size 256).
    pub fn build_sized(&self, id: &str, side: u32, mode: ScaleMode) -> Result<BTreeMap<Role, PathBuf>, I18nText> {
        let f = self.load_file(id)?;
        let roles = f.role_map();
        let sized_root = self.scheme_dir(id).join("sized");
        let dir = sized_root.join(format!("{side}-{}", mode.as_str()));
        // v2: one image per frame. v1 files (several sizes per frame) may fail to load, so they must be regenerated.
        let stamp = format!(
            "v2;{};{}",
            f.imported_unix,
            roles.iter().map(|(r, n)| format!("{}={n}", r.reg_name())).collect::<Vec<_>>().join(",")
        );
        let stamp_path = dir.join("stamp");
        let cached = std::fs::read_to_string(&stamp_path).is_ok_and(|s| s == stamp);
        let mut out = BTreeMap::new();
        if cached {
            // Only treat the cache as usable if all files are present.
            for role in roles.keys() {
                let found = ["ani", "cur"].iter().map(|e| dir.join(format!("{}.{e}", role.reg_name()))).find(|p| p.is_file());
                match found {
                    Some(p) => {
                        out.insert(*role, p);
                    }
                    None => {
                        out.clear();
                        break;
                    }
                }
            }
            if out.len() == roles.len() {
                remove_other_dirs(&sized_root, &dir);
                return Ok(out);
            }
            out.clear();
        }
        std::fs::create_dir_all(&dir).map_err(io_err)?;
        let _ = std::fs::remove_file(&stamp_path);
        for role in roles.keys() {
            let anim = self.read_frames(id, *role)?;
            let frames: Vec<Vec<_>> = anim.frames.iter().map(|img| vec![scale(img, side, mode)]).collect();
            let ext = if anim.frames.len() > 1 { "ani" } else { "cur" };
            let bytes = if ext == "ani" { write_ani(&frames, &anim.delays_jiffies) } else { write_cur(&frames[0]) };
            let path = dir.join(format!("{}.{ext}", role.reg_name()));
            write_atomic(&path, &bytes).map_err(io_err)?;
            // Remove the old file of the same role with the other extension.
            let other = dir.join(format!("{}.{}", role.reg_name(), if ext == "ani" { "cur" } else { "ani" }));
            let _ = std::fs::remove_file(other);
            out.insert(*role, path);
        }
        // Roles removed from the scheme: clean up leftover files.
        for role in Role::ALL.iter().filter(|r| !roles.contains_key(r)) {
            for e in ["ani", "cur"] {
                let _ = std::fs::remove_file(dir.join(format!("{}.{e}", role.reg_name())));
            }
        }
        write_atomic(&stamp_path, stamp.as_bytes()).map_err(io_err)?;
        remove_other_dirs(&sized_root, &dir);
        Ok(out)
    }
}

/// Removes every subdirectory of `sized_root` except `keep`. Errors are ignored (garbage collection, no effect on the applied cursor):
/// Windows has already loaded the cursor into memory, so deleting the old size's files does not break the cursor being shown.
fn remove_other_dirs(sized_root: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(sized_root) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p != keep
            && e.file_type().is_ok_and(|t| t.is_dir())
            && let Err(err) = std::fs::remove_dir_all(&p)
        {
            tracing::debug!(target: "sg::cursor", "không xoá được {}: {err}", p.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::flat_cur;

    /// Builds a fake scheme directly in the library (bypassing import).
    pub(crate) fn make_scheme(lib: &Library, id: &str, files: &[(&str, Vec<u8>)], roles: &[(Role, &str)]) {
        let dir = lib.scheme_dir(id);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        for (n, b) in files {
            std::fs::write(dir.join("files").join(n), b).unwrap();
        }
        let mut f = SchemeFile { name: id.to_string(), imported_unix: 100, source: "test".into(), roles: BTreeMap::new() };
        f.set_roles(&roles.iter().map(|(r, n)| (*r, n.to_string())).collect());
        lib.save_file(id, &f).unwrap();
    }

    #[test]
    fn list_get_rename_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path().to_path_buf());
        make_scheme(&lib, "abc-123456", &[("Normal.cur", flat_cur(32, 32, [1, 2, 3, 255]))], &[(Role::Arrow, "Normal.cur")]);
        let l = lib.list();
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].id.as_str(), l[0].role_count), ("abc-123456", 1));
        let d = lib.get("abc-123456").unwrap();
        assert_eq!(d.roles[&Role::Arrow].width, 32);
        assert_eq!(d.files, vec!["Normal.cur".to_string()]);
        lib.rename("abc-123456", "  Tên mới  ").unwrap();
        assert_eq!(lib.get("abc-123456").unwrap().name, "Tên mới");
        assert!(lib.rename("abc-123456", "   ").is_err());
        lib.delete("abc-123456").unwrap();
        assert!(lib.list().is_empty());
        assert!(lib.get("abc-123456").is_err());
        assert!(lib.get("../x").is_err());
        assert!(lib.delete("../x").is_err());
    }

    #[test]
    fn set_role_then_get() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path().to_path_buf());
        make_scheme(
            &lib,
            "s-000001",
            &[("A.cur", flat_cur(32, 32, [1, 1, 1, 255])), ("B.cur", flat_cur(48, 48, [2, 2, 2, 255])), ("bad.cur", vec![1, 2, 3])],
            &[(Role::Arrow, "A.cur")],
        );
        assert_eq!(lib.get("s-000001").unwrap().roles[&Role::Arrow].width, 32);
        lib.set_role("s-000001", Role::Arrow, Some("B.cur".into())).unwrap();
        assert_eq!(lib.get("s-000001").unwrap().roles[&Role::Arrow].width, 48);
        lib.set_role("s-000001", Role::Hand, Some("A.cur".into())).unwrap();
        assert_eq!(lib.get("s-000001").unwrap().roles.len(), 2);
        lib.set_role("s-000001", Role::Hand, None).unwrap();
        assert_eq!(lib.get("s-000001").unwrap().roles.len(), 1);
        // Unknown or corrupt files are rejected and nothing changes.
        assert!(lib.set_role("s-000001", Role::Hand, Some("nope.cur".into())).is_err());
        assert!(lib.set_role("s-000001", Role::Hand, Some("../A.cur".into())).is_err());
        assert!(lib.set_role("s-000001", Role::Hand, Some("bad.cur".into())).is_err());
        assert_eq!(lib.get("s-000001").unwrap().roles.len(), 1);
    }

    #[test]
    fn build_sized_is_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path().to_path_buf());
        make_scheme(&lib, "s-000002", &[("A.cur", flat_cur(32, 32, [1, 1, 1, 255]))], &[(Role::Arrow, "A.cur"), (Role::Hand, "A.cur")]);
        let first = lib.build_sized("s-000002", 48, ScaleMode::Sharp).unwrap();
        assert_eq!(first.len(), 2);
        let path = first[&Role::Arrow].clone();
        assert!(path.starts_with(tmp.path().join("s-000002").join("sized").join("48-sharp")));
        let m1 = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(60));
        let second = lib.build_sized("s-000002", 48, ScaleMode::Sharp).unwrap();
        assert_eq!(second, first);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), m1);
        // Exactly one 48×48 image (a multi-size frame can make Windows reject the whole file).
        let cur = crate::parse_ico_cur(&std::fs::read(&path).unwrap()).unwrap();
        let w: Vec<u32> = cur.iter().map(|i| i.width).collect();
        assert_eq!(w, vec![48]);
        // Changing a role → rebuild.
        lib.set_role("s-000002", Role::Hand, None).unwrap();
        let third = lib.build_sized("s-000002", 48, ScaleMode::Sharp).unwrap();
        assert_eq!(third.len(), 1);
        assert!(!tmp.path().join("s-000002/sized/48-sharp/Hand.cur").exists());
    }

    #[test]
    fn build_sized_ani_keeps_frames() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path().to_path_buf());
        let ani = crate::test_util::make_ani(
            &[flat_cur(32, 32, [1, 0, 0, 255]), flat_cur(32, 32, [2, 0, 0, 255]), flat_cur(32, 32, [3, 0, 0, 255])],
            None,
            Some(&[6, 12, 18]),
            6,
        );
        make_scheme(&lib, "s-000003", &[("W.ani", ani)], &[(Role::Wait, "W.ani")]);
        let d = lib.get("s-000003").unwrap();
        assert_eq!(d.roles[&Role::Wait].frames, 3);
        assert_eq!(d.roles[&Role::Wait].delays_ms, vec![100, 200, 300]);
        let built = lib.build_sized("s-000003", 64, ScaleMode::Smooth).unwrap();
        let path = &built[&Role::Wait];
        assert_eq!(path.extension().unwrap(), "ani");
        let a = parse_cursor_file(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(a.frames.len(), 3);
        assert_eq!(a.delays_jiffies, vec![6, 12, 18]);
        assert_eq!(a.frames[0].width, 64);
        // Only one image per frame.
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.windows(4).filter(|w| w == b"icon").count(), 3);
    }

    #[test]
    fn build_sized_removes_other_sizes_and_v1_cache_is_rebuilt() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path().to_path_buf());
        make_scheme(&lib, "s-000004", &[("A.cur", flat_cur(32, 32, [1, 1, 1, 255]))], &[(Role::Arrow, "A.cur")]);
        let sized = tmp.path().join("s-000004").join("sized");
        // A v1 cache (several sizes per frame) in the same directory: it must be regenerated, not reused.
        let dir = sized.join("40-sharp");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Arrow.cur"), b"old").unwrap();
        let roles = "Arrow=A.cur";
        let imported = lib.load_file("s-000004").unwrap().imported_unix;
        std::fs::write(dir.join("stamp"), format!("v1;{imported};{roles}")).unwrap();
        std::fs::create_dir_all(sized.join("96-smooth")).unwrap();
        std::fs::write(sized.join("96-smooth").join("Arrow.cur"), b"x").unwrap();

        let out = lib.build_sized("s-000004", 40, ScaleMode::Sharp).unwrap();
        assert_ne!(std::fs::read(&out[&Role::Arrow]).unwrap(), b"old");
        assert!(!sized.join("96-smooth").exists());
        // The next run uses the v2 cache and still cleans up stray directories.
        std::fs::create_dir_all(sized.join("64-sharp")).unwrap();
        lib.build_sized("s-000004", 40, ScaleMode::Sharp).unwrap();
        assert!(!sized.join("64-sharp").exists());
        assert!(dir.join("Arrow.cur").is_file());
    }
}
