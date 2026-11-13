//! "Export bug report": a zip with system information, logs, and configuration.
//! No personal files are collected; the home directory path is replaced with `~`.

use crate::paths::Paths;
use crate::sysinfo::SysInfo;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Only take logs modified within the last 7 days.
pub const LOG_DAYS: u64 = 7;
/// Take at most the last 5 MB of each log file.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Replaces the home directory path with `~`: in `\`, `/`, and `\\` forms (escaped strings in JSON/TOML/debug
/// logs) and the Windows 8.3 short name (`C:\Users\NGUYEN~1`, common in `%TEMP%`).
/// Matching is case-insensitive (ASCII letters), as on Windows. It only replaces when the name ends right
/// after (so it does not cut another user's name in half).
pub fn redact(text: &str, home: &Path) -> String {
    let mut bases = vec![home.to_string_lossy().into_owned()];
    if let Some(short) = crate::platform::short_path(home) {
        bases.push(short.to_string_lossy().into_owned());
    }
    redact_bases(text, &bases)
}

fn redact_bases(text: &str, bases: &[String]) -> String {
    let mut variants = Vec::new();
    for b in bases {
        let h = b.trim_end_matches(['/', '\\']);
        if h.len() < 3 {
            continue;
        }
        let back = h.replace('/', "\\");
        // Replace the `\\` form first so the `\` form does not eat half of it.
        variants.push(back.replace('\\', "\\\\"));
        variants.push(back);
        variants.push(h.replace('\\', "/"));
    }
    let mut seen = std::collections::HashSet::new();
    variants.retain(|v| seen.insert(v.to_ascii_lowercase()));
    let mut out = text.to_string();
    for v in variants {
        out = replace_path(&out, &v, true);
    }
    out
}

fn replace_path(hay: &str, needle: &str, ignore_ascii_case: bool) -> String {
    let (h, n) = if ignore_ascii_case {
        (hay.to_ascii_lowercase(), needle.to_ascii_lowercase())
    } else {
        (hay.to_string(), needle.to_string())
    };
    let mut out = String::with_capacity(hay.len());
    let mut last = 0;
    for (i, _) in h.match_indices(&n) {
        if i < last {
            continue;
        }
        let end = i + needle.len();
        let next = hay[end..].chars().next();
        if next.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        out.push_str(&hay[last..i]);
        out.push('~');
        last = end;
    }
    out.push_str(&hay[last..]);
    out
}

/// Reads at most the last `max` bytes of the file; if truncated, drops the partial line at the start.
fn read_tail(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if len <= max {
        let mut v = Vec::with_capacity(len as usize);
        f.read_to_end(&mut v)?;
        return Ok(v);
    }
    f.seek(SeekFrom::Start(len - max))?;
    let mut v = Vec::with_capacity(max as usize);
    f.take(max).read_to_end(&mut v)?;
    match v.iter().position(|&b| b == b'\n') {
        Some(p) => Ok(v.split_off(p + 1)),
        None => Ok(v),
    }
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect();
    v.sort();
    v
}

pub fn write_bug_report(dest: &Path, paths: &Paths, info: &SysInfo, home: Option<&Path>) -> io::Result<()> {
    let clean = |s: String| match home {
        Some(h) => redact(&s, h),
        None => s,
    };
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| -> io::Result<()> {
        let mut z = zip::ZipWriter::new(File::create(&tmp)?);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut add = |name: String, data: String| -> io::Result<()> {
            z.start_file(name, opt).map_err(io::Error::other)?;
            z.write_all(data.as_bytes())
        };
        add("system.txt".into(), clean(info.to_text()))?;

        let cutoff = SystemTime::now() - Duration::from_secs(LOG_DAYS * 24 * 3600);
        for p in files_in(&paths.logs_dir()) {
            let recent = p.metadata().and_then(|m| m.modified()).map(|t| t >= cutoff).unwrap_or(false);
            let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
            if !recent {
                continue;
            }
            if let Ok(bytes) = read_tail(&p, MAX_LOG_BYTES) {
                add(format!("logs/{name}"), clean(String::from_utf8_lossy(&bytes).into_owned()))?;
            }
        }
        for p in files_in(&paths.config_dir()) {
            if p.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
            if let Ok(text) = std::fs::read_to_string(&p) {
                add(format!("config/{name}"), clean(text))?;
            }
        }
        z.finish().map_err(io::Error::other)?.sync_all()
    })();
    match result.and_then(|_| std::fs::rename(&tmp, dest)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn names(zip_path: &Path) -> Vec<String> {
        let z = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
        z.file_names().map(|n| n.unwrap().to_string()).collect()
    }

    fn entry(zip_path: &Path, name: &str) -> Vec<u8> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
        let mut f = z.by_name(name).unwrap();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        v
    }

    fn info() -> SysInfo {
        SysInfo {
            app_version: "0.1.0".into(),
            os: "Test OS".into(),
            arch: "x86_64".into(),
            gpus: vec![],
            webview: None,
            locale: "vi-VN".into(),
            portable: false,
        }
    }

    #[test]
    fn redact_home_both_slashes() {
        let home = Path::new(r"C:\Users\An");
        let out = redact(r"C:\Users\An\x.log và C:/Users/an/y", home);
        assert_eq!(out, r"~\x.log và ~/y");
    }

    #[test]
    fn redact_escaped_and_short_forms() {
        let bases = [r"C:\Users\Nguyen Van An".to_string(), r"C:\Users\NGUYEN~1".to_string()];
        let text = r#"{"p":"C:\\Users\\Nguyen Van An\\a.txt"} tmp=C:\Users\NGUYEN~1\AppData\Local\Temp"#;
        let out = redact_bases(text, &bases);
        assert!(!out.contains("Nguyen Van An"), "{out}");
        assert!(!out.contains("NGUYEN~1"), "{out}");
        assert!(out.contains(r#""~\\a.txt""#), "{out}");
        assert!(out.contains(r"tmp=~\AppData"), "{out}");
    }

    #[test]
    fn redact_unix_home() {
        assert_eq!(redact("/home/an/.config/x", Path::new("/home/an")), "~/.config/x");
    }

    #[test]
    fn bug_report_contents() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(d.path(), true, None, None);
        std::fs::create_dir_all(paths.logs_dir()).unwrap();
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::create_dir_all(paths.profiles_dir(crate::ids::ModuleId::Cursor)).unwrap();
        let home = d.path().join("home");
        let home_s = home.to_string_lossy().to_string();
        std::fs::write(paths.logs_dir().join("core.2026-11-11.log"), format!("opened {home_s}/x\n")).unwrap();
        std::fs::write(paths.app_config_file(), "reduce_motion = true\n").unwrap();
        std::fs::write(paths.profiles_dir(crate::ids::ModuleId::Cursor).join("a.toml"), "x").unwrap();
        let dest = d.path().join("report.zip");
        write_bug_report(&dest, &paths, &info(), Some(&home)).unwrap();
        let n = names(&dest);
        assert!(n.contains(&"system.txt".to_string()), "{n:?}");
        assert!(n.contains(&"logs/core.2026-11-11.log".to_string()), "{n:?}");
        assert!(n.contains(&"config/app.toml".to_string()), "{n:?}");
        assert!(!n.iter().any(|x| x.starts_with("profiles/")), "{n:?}");
        let log = String::from_utf8(entry(&dest, "logs/core.2026-11-11.log")).unwrap();
        assert!(!log.contains(&home_s), "{log}");
        assert!(log.contains("~"));
    }

    #[test]
    fn bug_report_caps_log_size() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(d.path(), true, None, None);
        std::fs::create_dir_all(paths.logs_dir()).unwrap();
        let mut big = vec![b'a'; 6 * 1024 * 1024];
        big.extend_from_slice(b"\nDONG CUOI\n");
        std::fs::write(paths.logs_dir().join("core.log"), &big).unwrap();
        let dest = d.path().join("r.zip");
        write_bug_report(&dest, &paths, &info(), None).unwrap();
        let log = entry(&dest, "logs/core.log");
        assert!(log.len() <= 5 * 1024 * 1024);
        assert!(log.ends_with(b"DONG CUOI\n"));
    }
}
