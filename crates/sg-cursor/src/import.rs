//! Imports a cursor scheme from a `.zip`, a folder, or `install.inf`. It runs on files the user supplies, so
//! everything is treated as untrusted: paths inside the zip are filtered, and bytes are counted as they are actually read.

use crate::guess::guess_role;
use crate::inf::{decode_text, parse_inf};
use crate::library::{Library, SchemeFile, io_err};
use crate::{Role, parse_cursor_file};
use sg_core::config::write_atomic;
use sg_core::text::I18nText;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Import limits (defaults follow the plan; tests use smaller limits).
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_entries: usize,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_entries: 2000, max_total_bytes: 200 * 1024 * 1024, max_file_bytes: 16 * 1024 * 1024 }
    }
}

const INF_MAX_BYTES: u64 = 1024 * 1024;
const MAX_DEPTH: usize = 8;
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn too_large() -> I18nText {
    I18nText::new("cursor.err.import_too_large")
}

fn is_cursor_ext(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["ani", "cur", "ico"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Deletes the directory when it goes out of scope (even if an error occurs midway).
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn import(lib: &Library, src: &Path) -> Result<Vec<String>, I18nText> {
    import_with(lib, src, &Limits::default())
}

pub fn import_with(lib: &Library, src: &Path, limits: &Limits) -> Result<Vec<String>, I18nText> {
    std::fs::create_dir_all(lib.root()).map_err(io_err)?;
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let stem_of = |p: &Path| p.file_stem().and_then(|s| s.to_str()).unwrap_or("").trim().to_string();
    let (tree, only_inf, source_name, _guard);
    if src.is_dir() {
        tree = src.to_path_buf();
        only_inf = None;
        source_name = src.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        _guard = None;
    } else if ext == "zip" {
        let tmp = lib.root().join(format!(".import-{}", unique_token()));
        _guard = Some(RemoveOnDrop(tmp.clone()));
        extract_zip(src, &tmp, limits)?;
        tree = tmp;
        only_inf = None;
        source_name = stem_of(src);
    } else if ext == "inf" {
        tree = src.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        only_inf = Some(src.to_path_buf());
        source_name = tree.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        _guard = None;
    } else {
        return Err(I18nText::new("cursor.err.unsupported_source"));
    }
    let source_label = src.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let source_name = if source_name.is_empty() { "Cursor".to_string() } else { source_name };

    let files = walk(&tree, limits)?;
    let candidates = find_schemes(&files, only_inf.as_deref(), &source_name, limits);
    if candidates.is_empty() {
        return Err(I18nText::new("cursor.err.empty_scheme"));
    }
    let mut created: Vec<String> = Vec::new();
    for cand in &candidates {
        match install(lib, cand, &source_label, limits) {
            Ok(id) => created.push(id),
            Err(e) => {
                for id in &created {
                    let _ = lib.delete(id);
                }
                return Err(e);
            }
        }
    }
    Ok(created)
}

fn unique_token() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    // FNV-1a mixing the time, a counter and the pid.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in t.to_le_bytes().iter().chain(n.to_le_bytes().iter()).chain(std::process::id().to_le_bytes().iter()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:06x}", h & 0xFF_FFFF)
}

// ---------------------------------------------------------------------------------------------
// Extracting the zip

/// Whether a path component inside the zip is safe (no `..`, no characters forbidden on Windows).
fn safe_component(c: &str) -> bool {
    !c.is_empty()
        && !c.chars().all(|ch| ch == '.' || ch == ' ')
        && !c.chars().any(|ch| ch.is_control() || "<>:\"|?*".contains(ch))
}

/// `\` → `/`; rejects absolute paths, drive letters and `..`.
fn safe_zip_path(raw: &str) -> Option<Vec<String>> {
    let norm = raw.replace('\\', "/");
    if norm.starts_with('/') {
        return None;
    }
    let mut out = Vec::new();
    for c in norm.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if !safe_component(c) {
            return None;
        }
        out.push(c.to_string());
    }
    (!out.is_empty()).then_some(out)
}

fn extract_zip(src: &Path, dest: &Path, limits: &Limits) -> Result<(), I18nText> {
    let file = File::open(src).map_err(io_err)?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| crate::CursorError::Io(e.to_string()).to_text())?;
    if zip.len() > limits.max_entries {
        return Err(too_large());
    }
    std::fs::create_dir_all(dest).map_err(io_err)?;
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let Ok(mut entry) = zip.by_index(i) else { continue };
        if entry.is_dir() {
            continue;
        }
        let raw_name = entry.name().map(|c| c.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(entry.name_raw()).into_owned());
        let Some(parts) = safe_zip_path(&raw_name) else {
            tracing::warn!(target: "sg::cursor", "zip entry skipped (unsafe path): {raw_name:?}");
            continue;
        };
        let out_path: PathBuf = parts.iter().fold(dest.to_path_buf(), |p, c| p.join(c));
        let keep = out_path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["ani", "cur", "ico", "inf"].iter().any(|x| e.eq_ignore_ascii_case(x))
        });
        if !keep {
            continue;
        }
        if entry.size() > limits.max_file_bytes {
            return Err(too_large());
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        let mut out = File::create(&out_path).map_err(io_err)?;
        // Count the real bytes while reading; do not trust the size declared in the header.
        let mut buf = [0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = entry.read(&mut buf).map_err(io_err)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            total += n as u64;
            if written > limits.max_file_bytes || total > limits.max_total_bytes {
                return Err(too_large());
            }
            out.write_all(&buf[..n]).map_err(io_err)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Scanning the directory tree and finding schemes

/// Every `.ani/.cur/.ico/.inf` file under `root`, in a stable sorted order. Exceeding the entry limit → error.
fn walk(root: &Path, limits: &Limits) -> Result<Vec<PathBuf>, I18nText> {
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > limits.max_entries {
                return Err(too_large());
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let p = e.path();
            if ft.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((p, depth + 1));
                }
            } else if ft.is_file() {
                let is_inf = p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("inf"));
                if is_cursor_ext(&p) || is_inf {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

struct Candidate {
    name: String,
    roles: BTreeMap<Role, PathBuf>,
    /// Other cursor files in the same scheme (so the user can reassign roles later).
    extras: Vec<PathBuf>,
}

fn find_schemes(files: &[PathBuf], only_inf: Option<&Path>, source_name: &str, limits: &Limits) -> Vec<Candidate> {
    let mut valid: HashMap<PathBuf, bool> = HashMap::new();
    let mut is_valid = |p: &Path| -> bool {
        *valid.entry(p.to_path_buf()).or_insert_with(|| {
            let ok_size = std::fs::metadata(p).is_ok_and(|m| m.len() <= limits.max_file_bytes);
            ok_size
                && match std::fs::read(p) {
                    Ok(b) => match parse_cursor_file(&b) {
                        Ok(_) => true,
                        Err(e) => {
                            tracing::warn!(target: "sg::cursor", "skip {}: {e}", p.display());
                            false
                        }
                    },
                    Err(_) => false,
                }
        })
    };
    let cursor_files: Vec<&PathBuf> = files.iter().filter(|p| is_cursor_ext(p)).collect();
    let infs: Vec<&PathBuf> = if only_inf.is_some() {
        Vec::new()
    } else {
        files
            .iter()
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case("install.inf")))
            .collect()
    };
    let mut out = Vec::new();
    if infs.is_empty() && only_inf.is_none() {
        let mut roles: BTreeMap<Role, PathBuf> = BTreeMap::new();
        let mut sorted = cursor_files.clone();
        sorted.sort_by_key(|p| (p.components().count(), (*p).clone()));
        for p in sorted {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(role) = guess_role(name)
                && !roles.contains_key(&role)
                && is_valid(p)
            {
                roles.insert(role, p.clone());
            }
        }
        if !roles.is_empty() {
            let extras = cursor_files.iter().filter(|p| is_valid(p)).map(|p| (*p).clone()).collect();
            out.push(Candidate { name: source_name.to_string(), roles, extras });
        }
        return out;
    }
    let inf_paths: Vec<PathBuf> = match only_inf {
        Some(i) => vec![i.to_path_buf()],
        None => infs.into_iter().cloned().collect(),
    };
    for inf in inf_paths {
        let Some(dir) = inf.parent() else { continue };
        if std::fs::metadata(&inf).is_ok_and(|m| m.len() > INF_MAX_BYTES) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&inf) else { continue };
        let parsed = parse_inf(&decode_text(&bytes));
        let mut roles: BTreeMap<Role, PathBuf> = BTreeMap::new();
        for (role, fname) in &parsed.roles {
            let lower = fname.to_ascii_lowercase();
            let mut matches: Vec<&&PathBuf> = cursor_files
                .iter()
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_ascii_lowercase() == lower))
                .collect();
            // Prefer files in the same folder as the inf.
            matches.sort_by_key(|p| (p.parent() != Some(dir), p.components().count()));
            match matches.first() {
                Some(p) if is_valid(p) => {
                    roles.insert(*role, (***p).clone());
                }
                Some(_) => {}
                None => tracing::warn!(target: "sg::cursor", "inf role {role:?}: file {fname} not found"),
            }
        }
        if roles.is_empty() {
            continue;
        }
        let used: HashSet<&PathBuf> = roles.values().collect();
        let extras = cursor_files
            .iter()
            .filter(|p| p.parent() == Some(dir) && !used.contains(**p) && is_valid(p))
            .map(|p| (*p).clone())
            .collect();
        let name = parsed.name.unwrap_or_else(|| {
            let d = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if d.is_empty() { source_name.to_string() } else { d.to_string() }
        });
        out.push(Candidate { name, roles, extras });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Writing into the library

fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s: String = s.trim_matches('-').chars().take(40).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "scheme".to_string() } else { s }
}

fn sanitize_file_name(name: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, e.to_ascii_lowercase()),
        None => (name, String::new()),
    };
    let mut stem: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' })
        .take(80)
        .collect();
    stem = stem.trim_matches(|c| c == ' ' || c == '.').to_string();
    if stem.is_empty() {
        stem = "cursor".to_string();
    }
    const RESERVED: [&str; 8] = ["con", "prn", "aux", "nul", "com1", "com2", "lpt1", "lpt2"];
    if RESERVED.contains(&stem.to_ascii_lowercase().as_str()) {
        stem.insert(0, '_');
    }
    if ext.is_empty() { stem } else { format!("{stem}.{ext}") }
}

fn install(lib: &Library, cand: &Candidate, source: &str, limits: &Limits) -> Result<String, I18nText> {
    let mut id;
    loop {
        id = format!("{}-{}", slug(&cand.name), unique_token());
        if !lib.scheme_dir(&id).exists() {
            break;
        }
    }
    let tmp = lib.root().join(format!(".tmp-{id}"));
    let guard = RemoveOnDrop(tmp.clone());
    let files_dir = tmp.join("files");
    std::fs::create_dir_all(&files_dir).map_err(io_err)?;

    let mut used_names: HashSet<String> = HashSet::new();
    let mut stored: HashMap<PathBuf, String> = HashMap::new();
    let mut total: u64 = 0;
    let all: Vec<&PathBuf> = cand.roles.values().chain(cand.extras.iter()).collect();
    for src in all {
        if stored.contains_key(src) {
            continue;
        }
        let base = sanitize_file_name(src.file_name().and_then(|n| n.to_str()).unwrap_or("cursor.cur"));
        let mut name = base.clone();
        let mut n = 2;
        while !used_names.insert(name.to_ascii_lowercase()) {
            let (stem, ext) = base.rsplit_once('.').unwrap_or((&base, ""));
            name = format!("{stem}-{n}.{ext}");
            n += 1;
        }
        let len = std::fs::metadata(src).map_err(io_err)?.len();
        total += len;
        if len > limits.max_file_bytes || total > limits.max_total_bytes {
            return Err(too_large());
        }
        std::fs::copy(src, files_dir.join(&name)).map_err(io_err)?;
        stored.insert(src.clone(), name);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut file = SchemeFile {
        name: cand.name.trim().chars().take(60).collect(),
        imported_unix: now,
        source: source.to_string(),
        roles: BTreeMap::new(),
    };
    if file.name.is_empty() {
        file.name = "Cursor".to_string();
    }
    let roles: BTreeMap<Role, String> = cand.roles.iter().filter_map(|(r, p)| stored.get(p).map(|n| (*r, n.clone()))).collect();
    file.set_roles(&roles);
    let text = toml::to_string_pretty(&file).map_err(io_err)?;
    write_atomic(&tmp.join("scheme.toml"), text.as_bytes()).map_err(io_err)?;
    std::fs::rename(&tmp, lib.scheme_dir(&id)).map_err(io_err)?;
    // Rename succeeded: the guard has nothing left to delete, but it must not delete the new directory.
    std::mem::forget(guard);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{flat_cur, make_ani};

    fn zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn inf_for(name: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let mut s = String::from("[Scheme.Reg]\n");
        for (role, var) in files {
            s.push_str(&format!("HKCU,\"Control Panel\\Cursors\",{role},0x00020000,\"%10%\\%CUR_DIR%\\%{var}%\"\n"));
        }
        s.push_str("[Wreg]\nHKLM,\"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Runonce\",x,0,\"calc.exe\"\n[Strings]\n");
        s.push_str(&format!("CUR_DIR = \"Cursors\\X\"\nSCHEME_NAME = \"{name}\"\n"));
        for (_, var) in files {
            s.push_str(&format!("{var} = \"{var}.cur\"\n"));
        }
        s.into_bytes()
    }

    fn setup() -> (tempfile::TempDir, Library) {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path().join("lib"));
        (tmp, lib)
    }

    fn all_17_zip(prefix: &str, name: &str) -> Vec<(String, Vec<u8>)> {
        let vars: Vec<(&str, String)> = Role::ALL.iter().map(|r| (r.reg_name(), format!("f{}", r.reg_name()))).collect();
        let pairs: Vec<(&str, &str)> = vars.iter().map(|(r, v)| (*r, v.as_str())).collect();
        let mut v = vec![(format!("{prefix}install.inf"), inf_for(name, &pairs))];
        for (_, var) in &vars {
            v.push((format!("{prefix}{var}.cur"), flat_cur(32, 32, [9, 9, 9, 255])));
        }
        v
    }

    fn write_zip(path: &Path, entries: &[(String, Vec<u8>)]) {
        let refs: Vec<(&str, Vec<u8>)> = entries.iter().map(|(n, d)| (n.as_str(), d.clone())).collect();
        std::fs::write(path, zip_bytes(&refs)).unwrap();
    }

    #[test]
    fn import_zip_with_inf() {
        let (tmp, lib) = setup();
        let zp = tmp.path().join("Pack Một.zip");
        write_zip(&zp, &all_17_zip("Bo/", "Bộ Thử"));
        let ids = import(&lib, &zp).unwrap();
        assert_eq!(ids.len(), 1);
        assert!(valid_id_for_test(&ids[0]));
        let d = lib.get(&ids[0]).unwrap();
        assert_eq!(d.name, "Bộ Thử");
        assert_eq!(d.roles.len(), 17);
        assert_eq!(d.roles[&Role::Hand].width, 32);
        // No temp directory is left behind.
        let left: Vec<_> = std::fs::read_dir(lib.root()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1);
    }

    fn valid_id_for_test(id: &str) -> bool {
        crate::library::valid_id(id)
    }

    #[test]
    fn import_folder_guess() {
        let (tmp, lib) = setup();
        let dir = tmp.path().join("Pack Hai");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("Normal Select.cur"), flat_cur(32, 32, [1, 1, 1, 255])).unwrap();
        std::fs::write(dir.join("sub").join("Link Select.cur"), flat_cur(32, 32, [1, 1, 1, 255])).unwrap();
        std::fs::write(dir.join("readme.txt"), b"hi").unwrap();
        std::fs::write(dir.join("broken.cur"), b"nope").unwrap();
        let ids = import(&lib, &dir).unwrap();
        let d = lib.get(&ids[0]).unwrap();
        assert_eq!(d.name, "Pack Hai");
        assert_eq!(d.roles.keys().copied().collect::<Vec<_>>(), vec![Role::Arrow, Role::Hand]);
        assert!(!d.files.iter().any(|f| f.contains("broken")));
    }

    #[test]
    fn import_inf_file() {
        let (tmp, lib) = setup();
        let dir = tmp.path().join("src");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("install.inf"), inf_for("Từ INF", &[("Arrow", "a"), ("Help", "b")])).unwrap();
        std::fs::write(dir.join("a.cur"), flat_cur(32, 32, [1, 1, 1, 255])).unwrap();
        std::fs::write(dir.join("B.CUR"), flat_cur(32, 32, [2, 2, 2, 255])).unwrap();
        let ids = import(&lib, &dir.join("install.inf")).unwrap();
        let d = lib.get(&ids[0]).unwrap();
        assert_eq!(d.name, "Từ INF");
        assert_eq!(d.roles.len(), 2);
    }

    #[test]
    fn import_zip_two_schemes() {
        let (tmp, lib) = setup();
        let mut e = Vec::new();
        for (dir, name, role) in [("A/", "Bộ A", "Arrow"), ("B/", "Bộ B", "Hand")] {
            e.push((format!("{dir}install.inf"), inf_for(name, &[(role, "x")])));
            e.push((format!("{dir}x.cur"), flat_cur(32, 32, [3, 3, 3, 255])));
        }
        let zp = tmp.path().join("two.zip");
        write_zip(&zp, &e);
        let ids = import(&lib, &zp).unwrap();
        assert_eq!(ids.len(), 2);
        let mut names: Vec<String> = ids.iter().map(|i| lib.get(i).unwrap().name).collect();
        names.sort();
        assert_eq!(names, vec!["Bộ A", "Bộ B"]);
    }

    #[test]
    fn zip_backslash_paths() {
        let (tmp, lib) = setup();
        let mut e = all_17_zip("Hsin\\", "Hsin");
        for (n, _) in e.iter_mut() {
            *n = n.replace('/', "\\");
        }
        let zp = tmp.path().join("hsin.zip");
        write_zip(&zp, &e);
        let ids = import(&lib, &zp).unwrap();
        assert_eq!(lib.get(&ids[0]).unwrap().roles.len(), 17);
    }

    #[test]
    fn zip_slip_rejected() {
        let (tmp, lib) = setup();
        let mut e = all_17_zip("", "Slip");
        e.push(("../evil.cur".into(), flat_cur(8, 8, [0, 0, 0, 255])));
        e.push(("C:\\evil2.cur".into(), flat_cur(8, 8, [0, 0, 0, 255])));
        e.push(("/abs/evil3.cur".into(), flat_cur(8, 8, [0, 0, 0, 255])));
        e.push(("a/../../evil4.cur".into(), flat_cur(8, 8, [0, 0, 0, 255])));
        let zp = tmp.path().join("sub").join("slip.zip");
        std::fs::create_dir_all(zp.parent().unwrap()).unwrap();
        write_zip(&zp, &e);
        let ids = import(&lib, &zp).unwrap();
        assert_eq!(lib.get(&ids[0]).unwrap().roles.len(), 17);
        // No file escapes the library.
        for name in ["evil.cur", "evil2.cur", "evil3.cur", "evil4.cur"] {
            assert!(!tmp.path().join(name).exists());
            assert!(!tmp.path().join("sub").join(name).exists());
            assert!(!lib.root().join(name).exists());
        }
        assert!(!lib.get(&ids[0]).unwrap().files.iter().any(|f| f.starts_with("evil")));
    }

    #[test]
    fn zip_uncompressed_limit() {
        let (tmp, lib) = setup();
        let zp = tmp.path().join("bomb.zip");
        let e: Vec<(String, Vec<u8>)> = (0..6).map(|i| (format!("f{i}.cur"), vec![0u8; 100_000])).collect();
        write_zip(&zp, &e);
        let limits = Limits { max_entries: 100, max_total_bytes: 350_000, max_file_bytes: 1_000_000 };
        assert!(import_with(&lib, &zp, &limits).is_err());
        let limits = Limits { max_entries: 100, max_total_bytes: 10_000_000, max_file_bytes: 50_000 };
        assert!(import_with(&lib, &zp, &limits).is_err());
        let limits = Limits { max_entries: 3, max_total_bytes: 10_000_000, max_file_bytes: 1_000_000 };
        assert!(import_with(&lib, &zp, &limits).is_err());
        // Nothing is left in the library.
        assert!(lib.list().is_empty());
        assert!(std::fs::read_dir(lib.root()).map(|r| r.count() == 0).unwrap_or(true));
    }

    #[test]
    fn import_failure_leaves_no_dir() {
        let (tmp, lib) = setup();
        let dir = tmp.path().join("empty");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("junk.cur"), b"junk").unwrap();
        let err = import(&lib, &dir).unwrap_err();
        assert_eq!(err.key, "cursor.err.empty_scheme");
        assert!(std::fs::read_dir(lib.root()).map(|r| r.count() == 0).unwrap_or(true));
        assert!(import(&lib, &tmp.path().join("nope.rar")).is_err());
        assert!(import(&lib, &tmp.path().join("missing.zip")).is_err());
        assert!(std::fs::read_dir(lib.root()).map(|r| r.count() == 0).unwrap_or(true));
    }

    #[test]
    fn import_ani_and_two_files_same_name() {
        let (tmp, lib) = setup();
        let dir = tmp.path().join("p");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let ani = make_ani(&[flat_cur(16, 16, [1, 0, 0, 255]), flat_cur(16, 16, [2, 0, 0, 255])], None, None, 6);
        std::fs::write(dir.join("a").join("Busy.ani"), &ani).unwrap();
        std::fs::write(dir.join("b").join("Busy.ani"), &ani).unwrap();
        let ids = import(&lib, &dir).unwrap();
        let d = lib.get(&ids[0]).unwrap();
        assert_eq!(d.roles[&Role::Wait].frames, 2);
        assert_eq!(d.files.len(), 2);
    }

    #[test]
    fn file_name_sanitizing() {
        assert_eq!(sanitize_file_name("a:b*c.CUR"), "a_b_c.cur");
        assert_eq!(sanitize_file_name("NUL.cur"), "_NUL.cur");
        assert_eq!(slug("Bộ Thử  Một!"), "b-th-m-t");
        assert_eq!(slug("???"), "scheme");
    }
}
