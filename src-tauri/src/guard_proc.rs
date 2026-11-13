//! Launches the guard process and its run loop (`ShunguangDecor.exe --guard --journal <file>`).

use sg_core::paths::Paths;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const GUARD_ARG: &str = "--guard";
const JOURNAL_ARG: &str = "--journal";

/// Runs this same exe in guard mode. The stdin pipe is left empty: when the main app dies the pipe closes, and the guard process notices.
pub fn spawn(journal: &Path) -> Option<Child> {
    let exe = std::env::current_exe().ok()?;
    let mut cmd = Command::new(exe);
    cmd.arg(GUARD_ARG)
        .arg(JOURNAL_ARG)
        .arg(journal)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
    match cmd.spawn() {
        Ok(c) => {
            tracing::info!(target: "sg::guard", "guard started (pid {})", c.id());
            Some(c)
        }
        Err(e) => {
            tracing::warn!(target: "sg::guard", "cannot start guard: {e}");
            None
        }
    }
}

/// Gets the journal path from the arguments; if missing, uses the default path.
pub fn journal_from_args(args: &[String], paths: &Paths) -> PathBuf {
    args.windows(2)
        .find(|w| w[0] == JOURNAL_ARG)
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| paths.guard_journal_file())
}

/// Body of the guard process. Does not start Tauri and opens no window.
pub fn run(args: &[String]) -> i32 {
    let (paths, _) = Paths::detect();
    let _log = sg_core::logging::init(&paths.logs_dir(), "guard");
    let journal = journal_from_args(args, &paths);
    tracing::info!(target: "sg::guard", "guard waiting (journal {})", journal.display());
    let stdin = std::io::stdin();
    let outcome = sg_core::guard::run_guard(stdin.lock(), &journal, &mut sg_core::guard::perform);
    tracing::info!(target: "sg::guard", "guard done: {outcome:?}");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_arg_parsed() {
        let paths = Paths::resolve(Path::new("C:/x"), true, None, None);
        let args: Vec<String> = ["exe", "--guard", "--journal", "D:/j.toml"].iter().map(|s| s.to_string()).collect();
        assert_eq!(journal_from_args(&args, &paths), PathBuf::from("D:/j.toml"));
        let none: Vec<String> = ["exe", "--guard"].iter().map(|s| s.to_string()).collect();
        assert_eq!(journal_from_args(&none, &paths), paths.guard_journal_file());
    }
}
