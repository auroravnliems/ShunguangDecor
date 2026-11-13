//! The DLL's log (in English): `paths.local/logs/explorer-hook.log`, rotated at 1 MiB.
//!
//! - Anyone can `push` (including the desktop thread): it only appends a line to an in-memory queue, no I/O.
//! - Only the pipe thread `flush`es to the file. The path is sent by the app in `Hello`; until then lines wait
//!   in the queue (at most [`MAX_QUEUE`] lines, dropping the oldest).

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_QUEUE: usize = 256;
pub const MAX_LOG_BYTES: u64 = 1 << 20;

pub struct Logger {
    queue: Mutex<VecDeque<String>>,
    path: Mutex<Option<PathBuf>>,
    max_bytes: u64,
}

impl Logger {
    pub const fn new(max_bytes: u64) -> Logger {
        Logger { queue: Mutex::new(VecDeque::new()), path: Mutex::new(None), max_bytes }
    }

    /// Adds a line. Never blocks for long: if the lock is busy (rare) the line is dropped.
    pub fn push(&self, line: String) {
        let line = format!("{} {line}", timestamp());
        if let Ok(mut q) = self.queue.try_lock() {
            if q.len() >= MAX_QUEUE {
                q.pop_front();
            }
            q.push_back(line);
        }
    }

    pub fn set_path(&self, p: Option<PathBuf>) {
        *self.path.lock().unwrap_or_else(|e| e.into_inner()) = p;
    }

    pub fn pending(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// Writes the pending lines to the file (if the path is known). On a write error they are dropped, not retried forever.
    pub fn flush(&self) {
        let Some(path) = self.path.lock().ok().and_then(|p| p.clone()) else { return };
        let lines: Vec<String> = match self.queue.lock() {
            Ok(mut q) if !q.is_empty() => q.drain(..).collect(),
            _ => return,
        };
        let _ = append(&path, &lines, self.max_bytes);
    }
}

fn append(path: &Path, lines: &[String], max_bytes: u64) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).map(|m| m.len() >= max_bytes).unwrap_or(false) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(path, &old)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let mut text = String::new();
    for l in lines {
        text.push_str(l);
        text.push_str("\r\n");
    }
    f.write_all(text.as_bytes())
}

/// UTC time formatted like `2026-11-12T03:04:05.678Z` (no date/time library needed).
pub fn timestamp() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format_utc(d.as_millis() as u64)
}

pub fn format_utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // The civil_from_days algorithm (Howard Hinnant).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        ms % 1000
    )
}

static LOGGER: Logger = Logger::new(MAX_LOG_BYTES);

pub fn push(line: impl Into<String>) {
    LOGGER.push(line.into());
}

pub fn set_path(p: Option<PathBuf>) {
    LOGGER.set_path(p);
}

pub fn flush() {
    LOGGER.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_format() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00.000Z");
        // 2026-11-12T03:04:05.678Z
        assert_eq!(format_utc(1_794_452_645_678), "2026-11-12T03:04:05.678Z");
        assert_eq!(format_utc(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn log_waits_for_path_then_rotates() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("logs").join("explorer-hook.log");
        let lg = Logger::new(64);
        lg.push("before path".into());
        lg.flush();
        assert!(!p.exists(), "chưa có đường dẫn thì không ghi gì");
        assert_eq!(lg.pending(), 1);
        lg.set_path(Some(p.clone()));
        lg.flush();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("before path"), "{t}");
        for i in 0..4 {
            lg.push(format!("line {i} {}", "x".repeat(30)));
            lg.flush();
        }
        let mut old = p.as_os_str().to_owned();
        old.push(".1");
        assert!(std::path::Path::new(&old).exists(), "quá giới hạn thì đổi tên sang .1");
        assert!(std::fs::metadata(&p).unwrap().len() < 200);
    }

    #[test]
    fn queue_is_bounded() {
        let lg = Logger::new(1 << 20);
        for i in 0..(MAX_QUEUE + 10) {
            lg.push(format!("{i}"));
        }
        assert_eq!(lg.pending(), MAX_QUEUE);
    }
}
