//! File logging with daily rotation. Each module logs with `target: "sg::<module>"`
//! so every line can be traced back to its module.

use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

/// Number of log files kept (one per day).
pub const KEEP_FILES: usize = 14;

/// Enables logging to `<dir>/<prefix>.<date>.log`. Keep the returned `WorkerGuard` alive until exit so logs get flushed.
/// The default level is `info`; override it with the `SG_LOG` environment variable (e.g. `SG_LOG=debug`).
pub fn init(dir: &Path, prefix: &str) -> Option<WorkerGuard> {
    std::fs::create_dir_all(dir).ok()?;
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("SG_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = fmt::layer().with_writer(writer).with_ansi(false).with_target(true);
    // Debug builds also print to the console, handy when running `tauri dev`.
    let console = cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr).with_target(true));
    tracing_subscriber::registry().with(filter).with(file_layer).with(console).try_init().ok()?;
    install_panic_hook();
    Some(guard)
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        tracing::error!(target: "sg::panic", "panic in thread {thread}: {info}");
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_writes_daily_file() {
        let d = tempfile::tempdir().unwrap();
        let guard = init(d.path(), "core").expect("init log");
        tracing::info!(target: "sg::test", "xin chào log");
        drop(guard);
        let files: Vec<String> = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].starts_with("core.") && files[0].ends_with(".log"), "{files:?}");
        let text = std::fs::read_to_string(d.path().join(&files[0])).unwrap();
        assert!(text.contains("xin chào log") && text.contains("sg::test"), "{text}");
    }
}
