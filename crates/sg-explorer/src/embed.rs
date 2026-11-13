//! The DLL embedded in the exe → a file in `paths.local/bin/`.
//!
//! The file name carries the version number + 8 hash characters of the content: a new app build always gets a new name, so an old
//! file still held by Explorer (a pinned DLL) does not block updating. Old files are cleaned up once they can be deleted (Explorer restarted).
//! Before every load, the file content is compared byte by byte with the embedded copy (the file name is not trusted).

use std::io;
use std::path::{Path, PathBuf};

pub const DLL_PREFIX: &str = "shunguang_explorer-";

/// FNV-1a 32-bit → 8 hex characters. Only for naming (the content is always compared in full).
pub fn short_hash(bytes: &[u8]) -> String {
    let mut h: u32 = 0x811C_9DC5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

pub fn dll_file_name(version: &str, bytes: &[u8]) -> String {
    format!("{DLL_PREFIX}{version}-{}.dll", short_hash(bytes))
}

/// Whether the file has exactly this content.
pub fn same_content(path: &Path, bytes: &[u8]) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() == bytes.len() as u64) && std::fs::read(path).is_ok_and(|b| b == bytes)
}

/// Writes the DLL into `dir` if the exact file is not there yet. Returns the file path.
pub fn extract(dir: &Path, version: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    if bytes.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no embedded DLL"));
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(dll_file_name(version, bytes));
    if same_content(&path, bytes) {
        return Ok(path);
    }
    // Write to a temp file then rename: never leave a half-written DLL under the real name.
    let tmp = dir.join(format!(".{}.tmp", dll_file_name(version, bytes)));
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        // If the target file is locked (loaded in Explorer) but its content is correct, it is still usable.
        if same_content(&path, bytes) {
            return Ok(path);
        }
        return Err(e);
    }
    Ok(path)
}

/// Deletes the app's old DLLs in `dir` (except `keep`). Files still held by Explorer are left alone. Returns the number of files deleted.
pub fn clean_old(dir: &Path, keep: Option<&Path>) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let ours = (name.starts_with(DLL_PREFIX) && name.ends_with(".dll")) || (name.starts_with(&format!(".{DLL_PREFIX}")) && name.ends_with(".tmp"));
        if !ours || keep.is_some_and(|k| k == p) {
            continue;
        }
        if std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dll_file_name_has_version_and_hash() {
        let a = dll_file_name("0.1.0", b"abc");
        assert!(a.starts_with("shunguang_explorer-0.1.0-") && a.ends_with(".dll"), "{a}");
        let hash = &a["shunguang_explorer-0.1.0-".len()..a.len() - 4];
        assert_eq!(hash.len(), 8);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, dll_file_name("0.1.0", b"abd"), "nội dung khác → tên khác");
        assert_ne!(a, dll_file_name("0.2.0", b"abc"));
        assert_eq!(short_hash(b""), "811c9dc5");
    }

    #[test]
    fn extract_skips_when_same_hash() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        let p = extract(&bin, "0.1.0", b"dll bytes").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"dll bytes");
        let t1 = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let p2 = extract(&bin, "0.1.0", b"dll bytes").unwrap();
        assert_eq!(p, p2);
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), t1, "đã đúng file thì không ghi lại");
        // File tampered with (same name) → rewritten with the correct content.
        std::fs::write(&p, b"tampered!").unwrap();
        extract(&bin, "0.1.0", b"dll bytes").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"dll bytes");
        assert!(extract(&bin, "0.1.0", b"").is_err(), "không có DLL nhúng");
    }

    #[test]
    fn old_dll_files_cleaned_when_unlocked() {
        let d = tempfile::tempdir().unwrap();
        let keep = extract(d.path(), "0.2.0", b"new").unwrap();
        let old = d.path().join("shunguang_explorer-0.1.0-00000000.dll");
        std::fs::write(&old, b"old").unwrap();
        let other = d.path().join("khac.dll");
        std::fs::write(&other, b"x").unwrap();
        let locked_path = d.path().join("shunguang_explorer-0.0.9-11111111.dll");
        let _lock = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::write(&locked_path, b"in explorer").unwrap();
            // Do not share delete access: just like a DLL loaded in Explorer.
            std::fs::OpenOptions::new().read(true).share_mode(0).open(&locked_path).unwrap()
        };
        let n = clean_old(d.path(), Some(&keep));
        assert_eq!(n, 1);
        assert!(!old.exists());
        assert!(keep.exists() && other.exists(), "không đụng file đang dùng và file không phải của ta");
        assert!(locked_path.exists(), "file đang bị giữ thì để lại, không lỗi");
    }
}
