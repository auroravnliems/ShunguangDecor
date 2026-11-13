//! The two real files the engine reads: `taskbar-raw/config.yaml` + `taskbar-raw/styles.css`, plus `.generated-hash`
//! (the hash of the app's most recent write). The user edits by hand → different hash → `EditedByUser`, and the app NO longer overwrites
//! until the user chooses ("Use the app's controls again" or "Keep my files").

use crate::generate::Generated;
use serde::Serialize;
use sg_core::config::write_atomic;
use sg_core::text::I18nText;
use std::io;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "config.yaml";
pub const STYLES_FILE: &str = "styles.css";
pub const HASH_FILE: &str = ".generated-hash";
/// An imported YASB theme file larger than this is abnormal (real themes are only a few dozen KB).
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RawState {
    /// Content exactly as the app wrote it (or no files yet): the app may rewrite when the configuration changes.
    #[serde(rename = "generated")]
    Generated,
    /// The user edited by hand or imported a theme: the engine uses the user's files.
    #[serde(rename = "edited")]
    EditedByUser,
}

/// `<roaming>/taskbar-raw`.
pub fn dir(roaming: &Path) -> PathBuf {
    roaming.join("taskbar-raw")
}

/// FNV-1a 64-bit (stable across Rust versions, unlike `DefaultHasher`) over the BYTES of the yaml, one 0 byte, then the css.
/// Hashing bytes (not strings) so that even a non-UTF-8 file is detected as edited.
pub fn hash_of(yaml: impl AsRef<[u8]>, css: impl AsRef<[u8]>) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in yaml.as_ref().iter().chain(std::iter::once(&0u8)).chain(css.as_ref().iter()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Decodes the user's text file: UTF-8 (BOM stripped) or UTF-16 with a BOM (old Notepad). Anything else → `None`.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, u16::from_be_bytes);
    }
    let rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(rest.to_vec()).ok()
}

fn decode_utf16(rest: &[u8], unit: fn([u8; 2]) -> u16) -> Option<String> {
    if !rest.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| unit(*c)).collect();
    String::from_utf16(&units).ok()
}

/// Compares the current BYTE hash with the recorded hash. A recorded hash with a file that is missing / unreadable / different → `EditedByUser`
/// (never let the app overwrite). Only "no hash and no files at all" counts as fresh → `Generated`; files present but no
/// hash (a theme was imported) → `EditedByUser`.
pub fn state(dir: &Path) -> RawState {
    let (cfg_p, css_p) = (dir.join(CONFIG_FILE), dir.join(STYLES_FILE));
    let stored = std::fs::read_to_string(dir.join(HASH_FILE)).ok();
    let Some(stored) = stored else {
        return if cfg_p.exists() || css_p.exists() { RawState::EditedByUser } else { RawState::Generated };
    };
    match (std::fs::read(&cfg_p), std::fs::read(&css_p)) {
        (Ok(yaml), Ok(css)) if stored.trim() == hash_of(&yaml, &css) => RawState::Generated,
        _ => RawState::EditedByUser,
    }
}

/// Writes the two files + the hash (the hash is written last: if we die midway, the next run sees "edited" instead of overwriting by mistake).
pub fn write_generated(dir: &Path, g: &Generated) -> io::Result<()> {
    write_atomic(&dir.join(CONFIG_FILE), g.yaml.as_bytes())?;
    write_atomic(&dir.join(STYLES_FILE), g.css.as_bytes())?;
    write_atomic(&dir.join(HASH_FILE), hash_of(&g.yaml, &g.css).as_bytes())
}

fn read_one(dir: &Path, name: &str) -> Result<String, I18nText> {
    let bytes = std::fs::read(dir.join(name)).map_err(|e| I18nText::new("taskbar.err.raw_io").arg("detail", e))?;
    decode_text(&bytes).ok_or_else(|| I18nText::new("taskbar.err.raw_decode").arg("file", name))
}

/// Content of the two files on disk (UTF-8 or UTF-16 with a BOM). If it cannot be decoded → a clear error, and the file is NOT overwritten.
pub fn read(dir: &Path) -> Result<Generated, I18nText> {
    Ok(Generated { yaml: read_one(dir, CONFIG_FILE)?, css: read_one(dir, STYLES_FILE)? })
}

/// Syncs with disk before running: `Generated` → write `g` over it; `EditedByUser` → touch no files.
/// Returns the final state.
pub fn sync(dir: &Path, g: &Generated) -> io::Result<RawState> {
    match state(dir) {
        RawState::Generated => {
            write_generated(dir, g)?;
            Ok(RawState::Generated)
        }
        RawState::EditedByUser => Ok(RawState::EditedByUser),
    }
}

/// The user's choice. `keep_user = false`: overwrite with `g` (the app takes control back). `true`: keep the user's
/// files as they are (the state stays `EditedByUser`, and the engine keeps using those files).
pub fn choose(dir: &Path, keep_user: bool, g: &Generated) -> io::Result<()> {
    if keep_user { Ok(()) } else { write_generated(dir, g) }
}

fn invalid(file: &str, why: impl ToString) -> I18nText {
    I18nText::new("taskbar.err.import_invalid").arg("file", file).arg("detail", why)
}

fn read_limited(src: &Path, name: &str) -> Result<String, I18nText> {
    let p = src.join(name);
    let meta = std::fs::metadata(&p).map_err(|_| I18nText::new("taskbar.err.import_missing").arg("file", name))?;
    if !meta.is_file() {
        return Err(I18nText::new("taskbar.err.import_missing").arg("file", name));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(I18nText::new("taskbar.err.import_big").arg("file", name));
    }
    let bytes = std::fs::read(&p).map_err(|e| invalid(name, e))?;
    decode_text(&bytes).ok_or_else(|| I18nText::new("taskbar.err.import_utf8").arg("file", name))
}

/// Imports a YASB theme: `src` must contain `config.yaml` + `styles.css` (≤ 1 MB, UTF-8) and the engine must be able to parse them.
/// Only when valid is it written into `dir` (old files are untouched on rejection) and the state set to `EditedByUser`.
pub fn import(src: &Path, dir: &Path, validate: &dyn Fn(&Generated) -> Result<(), String>) -> Result<Generated, I18nText> {
    let g = Generated { yaml: read_limited(src, CONFIG_FILE)?, css: read_limited(src, STYLES_FILE)? };
    validate(&g).map_err(|e| invalid(CONFIG_FILE, e))?;
    // Remove the hash first: if we die midway, the files (old or new) are treated as the user's and are not overwritten by the app.
    match std::fs::remove_file(dir.join(HASH_FILE)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(invalid(HASH_FILE, e)),
    }
    write_atomic(&dir.join(CONFIG_FILE), g.yaml.as_bytes()).map_err(|e| invalid(CONFIG_FILE, e))?;
    write_atomic(&dir.join(STYLES_FILE), g.css.as_bytes()).map_err(|e| invalid(STYLES_FILE, e))?;
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen_a() -> Generated {
        Generated { yaml: "bars: {}\n".into(), css: ".bar {}\n".into() }
    }

    #[test]
    fn raw_edit_detected_by_hash() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("taskbar-raw");
        // Nothing yet: the app may write.
        assert_eq!(state(&d), RawState::Generated);
        assert_eq!(sync(&d, &gen_a()).unwrap(), RawState::Generated);
        assert_eq!(state(&d), RawState::Generated);
        assert_eq!(read(&d).unwrap(), gen_a());
        // Configuration changed, files not edited: overwrite as usual.
        let g2 = Generated { yaml: "bars: {a: 1}\n".into(), css: gen_a().css };
        assert_eq!(sync(&d, &g2).unwrap(), RawState::Generated);
        assert_eq!(read(&d).unwrap(), g2);
        // The user changes one CSS character: it is detected, and sync does not overwrite.
        std::fs::write(d.join(STYLES_FILE), ".bar { color: red }\n").unwrap();
        assert_eq!(state(&d), RawState::EditedByUser);
        let g3 = Generated { yaml: "bars: {b: 2}\n".into(), css: "x\n".into() };
        assert_eq!(sync(&d, &g3).unwrap(), RawState::EditedByUser);
        assert_eq!(std::fs::read_to_string(d.join(STYLES_FILE)).unwrap(), ".bar { color: red }\n");
        assert_eq!(std::fs::read_to_string(d.join(CONFIG_FILE)).unwrap(), g2.yaml);
        // Keep my files: nothing changes, still "edited".
        choose(&d, true, &g3).unwrap();
        assert_eq!(state(&d), RawState::EditedByUser);
        assert_eq!(std::fs::read_to_string(d.join(STYLES_FILE)).unwrap(), ".bar { color: red }\n");
        // Use the app's controls again: overwrite, state goes back to Generated.
        choose(&d, false, &g3).unwrap();
        assert_eq!(state(&d), RawState::Generated);
        assert_eq!(read(&d).unwrap(), g3);
    }

    fn utf16_le(s: &str) -> Vec<u8> {
        let mut v = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    fn utf16_user_file_is_edited_never_overwritten_and_decodes() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("taskbar-raw");
        sync(&d, &gen_a()).unwrap();
        std::fs::write(d.join(STYLES_FILE), utf16_le(".bar { color: red }")).unwrap();
        assert_eq!(state(&d), RawState::EditedByUser);
        let g2 = Generated { yaml: "bars: {b: 2}
".into(), css: "x
".into() };
        assert_eq!(sync(&d, &g2).unwrap(), RawState::EditedByUser);
        assert_eq!(std::fs::read(d.join(STYLES_FILE)).unwrap(), utf16_le(".bar { color: red }"));
        assert_eq!(read(&d).unwrap().css, ".bar { color: red }");
    }

    #[test]
    fn undecodable_user_file_is_edited_and_read_errors_without_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("taskbar-raw");
        sync(&d, &gen_a()).unwrap();
        std::fs::write(d.join(CONFIG_FILE), [0x80, 0x81, 0x82]).unwrap();
        assert_eq!(state(&d), RawState::EditedByUser);
        assert_eq!(read(&d).unwrap_err().key, "taskbar.err.raw_decode");
        assert_eq!(sync(&d, &gen_a()).unwrap(), RawState::EditedByUser);
        assert_eq!(std::fs::read(d.join(CONFIG_FILE)).unwrap(), [0x80, 0x81, 0x82]);
    }

    #[test]
    fn missing_file_with_stored_hash_is_edited_and_not_recreated() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("taskbar-raw");
        sync(&d, &gen_a()).unwrap();
        std::fs::remove_file(d.join(STYLES_FILE)).unwrap();
        assert_eq!(state(&d), RawState::EditedByUser);
        assert_eq!(sync(&d, &gen_a()).unwrap(), RawState::EditedByUser);
        assert!(!d.join(STYLES_FILE).exists());
        // The user chooses to use the app's controls again: the files are regenerated.
        choose(&d, false, &gen_a()).unwrap();
        assert_eq!(state(&d), RawState::Generated);
        assert!(d.join(STYLES_FILE).exists());
    }

    #[test]
    fn only_one_file_without_hash_is_edited() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(CONFIG_FILE), "a").unwrap();
        assert_eq!(state(t.path()), RawState::EditedByUser);
        assert_eq!(state(&t.path().join("nothing")), RawState::Generated);
    }

    #[test]
    fn missing_hash_with_files_is_edited() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(CONFIG_FILE), "a").unwrap();
        std::fs::write(t.path().join(STYLES_FILE), "b").unwrap();
        assert_eq!(state(t.path()), RawState::EditedByUser);
    }

    #[test]
    fn hash_is_stable_and_separates_the_two_files() {
        assert_eq!(hash_of("a", "b"), hash_of("a", "b"));
        assert_ne!(hash_of("ab", ""), hash_of("a", "b"));
        assert_eq!(hash_of("", "").len(), 16);
    }

    fn yasb_dir(yaml: &str, css: &str) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(CONFIG_FILE), yaml).unwrap();
        std::fs::write(t.path().join(STYLES_FILE), css).unwrap();
        t
    }

    #[test]
    fn import_accepts_valid_files_and_marks_edited() {
        let src = yasb_dir("bars: {}\n", ".bar {}\n");
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("taskbar-raw");
        sync(&d, &gen_a()).unwrap();
        let g = import(src.path(), &d, &|_| Ok(())).unwrap();
        assert_eq!(g.yaml, "bars: {}\n");
        assert_eq!(state(&d), RawState::EditedByUser);
        assert_eq!(read(&d).unwrap(), g);
    }

    #[test]
    fn import_rejects_and_leaves_old_files_untouched() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("taskbar-raw");
        sync(&d, &gen_a()).unwrap();
        let before = read(&d).unwrap();

        // Missing file.
        let empty = tempfile::tempdir().unwrap();
        let e = import(empty.path(), &d, &|_| Ok(())).unwrap_err();
        assert_eq!(e.key, "taskbar.err.import_missing");
        assert_eq!(e.args["file"], CONFIG_FILE);
        let only_yaml = tempfile::tempdir().unwrap();
        std::fs::write(only_yaml.path().join(CONFIG_FILE), "a").unwrap();
        assert_eq!(import(only_yaml.path(), &d, &|_| Ok(())).unwrap_err().args["file"], STYLES_FILE);

        // Too large.
        let big = yasb_dir("a", &"x".repeat(MAX_FILE_BYTES as usize + 1));
        let e = import(big.path(), &d, &|_| Ok(())).unwrap_err();
        assert_eq!(e.key, "taskbar.err.import_big");
        assert_eq!(e.args["file"], STYLES_FILE);

        // Not UTF-8.
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join(CONFIG_FILE), [0xff, 0xfe, 0x00]).unwrap();
        std::fs::write(bin.path().join(STYLES_FILE), "a").unwrap();
        assert_eq!(import(bin.path(), &d, &|_| Ok(())).unwrap_err().key, "taskbar.err.import_utf8");

        // The engine cannot parse it.
        let bad = yasb_dir("not: [valid", ".bar {}");
        let e = import(bad.path(), &d, &|_| Err("lỗi cú pháp".into())).unwrap_err();
        assert_eq!(e.key, "taskbar.err.import_invalid");
        assert_eq!(e.args["detail"], "lỗi cú pháp");

        // None of the attempts touched the old files, the state is still Generated.
        assert_eq!(read(&d).unwrap(), before);
        assert_eq!(state(&d), RawState::Generated);
    }

    #[test]
    fn raw_state_serializes_as_plain_strings() {
        assert_eq!(serde_json_str(RawState::Generated), "\"generated\"");
        assert_eq!(serde_json_str(RawState::EditedByUser), "\"edited\"");
    }

    fn serde_json_str(s: RawState) -> String {
        // sg-taskbar does not depend on serde_json: check with toml (same serde renaming rules).
        #[derive(Serialize)]
        struct W {
            v: RawState,
        }
        toml::to_string(&W { v: s }).unwrap().trim().trim_start_matches("v = ").to_string()
    }
}
