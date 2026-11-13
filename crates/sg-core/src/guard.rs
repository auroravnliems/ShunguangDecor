//! Guard process (`ShunguangDecor.exe --guard`).
//!
//! The main app holds the write end of a stdin pipe connected to the guard process. When the
//! main app exits (cleanly or by crashing) the pipe closes → the guard reads EOF → it performs
//! any restore actions still recorded in the journal `state/guard.toml` → exits. On a clean
//! exit the main app has already restored everything and cleared the journal, so the guard
//! only sees an empty journal.

use crate::config::write_atomic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Work the guard process must do if the main app dies abnormally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreAction {
    /// Show the native Windows taskbar again (the YASB taskbar part hides it).
    ShowNativeTaskbar,
    /// Restore the original desktop icon label color (desktop labels, `LVM_SETTEXTCOLOR` fallback) from `backup/labels-original.toml`.
    RestoreDesktopLabels,
    /// Turn off the native taskbar's auto-hide that the YASB taskbar part enabled to give the work area back (only recorded when the user had not enabled it already).
    DisableTaskbarAutohide,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GuardOutcome {
    Clean,
    Restored(usize),
}

#[derive(Serialize)]
struct JournalFile<'a> {
    actions: &'a [RestoreAction],
}

pub struct GuardJournal {
    path: PathBuf,
    actions: BTreeSet<RestoreAction>,
}

impl GuardJournal {
    /// A missing or corrupt file means an empty journal. Unknown actions (from a newer app version) are ignored.
    pub fn open(path: &Path) -> GuardJournal {
        let mut actions = BTreeSet::new();
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(t) = crate::config::strip_bom(&text).parse::<toml::Table>()
            && let Some(arr) = t.get("actions").and_then(toml::Value::as_array)
        {
            for v in arr {
                if let Ok(a) = v.clone().try_into::<RestoreAction>() {
                    actions.insert(a);
                }
            }
        }
        GuardJournal { path: path.to_path_buf(), actions }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn actions(&self) -> Vec<RestoreAction> {
        self.actions.iter().copied().collect()
    }

    pub fn add(&mut self, a: RestoreAction) -> io::Result<()> {
        if self.actions.insert(a) { self.write() } else { Ok(()) }
    }

    pub fn remove(&mut self, a: RestoreAction) -> io::Result<()> {
        if self.actions.remove(&a) { self.write() } else { Ok(()) }
    }

    pub fn contains(&self, a: RestoreAction) -> bool {
        self.actions.contains(&a)
    }

    pub fn clear(&mut self) -> io::Result<()> {
        self.actions.clear();
        self.write()
    }

    fn write(&self) -> io::Result<()> {
        let list = self.actions();
        let text = toml::to_string(&JournalFile { actions: &list }).map_err(io::Error::other)?;
        write_atomic(&self.path, text.as_bytes())
    }
}

/// Perform one restore action on the real operating system.
pub fn perform(a: RestoreAction) -> io::Result<()> {
    match a {
        RestoreAction::ShowNativeTaskbar => crate::platform::show_native_taskbar(),
        // The guard process has no app `Paths`: recompute them the way the app does (same exe, same `portable.txt`).
        RestoreAction::RestoreDesktopLabels => {
            crate::labels_backup::restore(&crate::labels_backup::path(&crate::paths::Paths::detect().0)).map(|_| ())
        }
        RestoreAction::DisableTaskbarAutohide => crate::platform::disable_taskbar_autohide(),
    }
}

/// Perform every action currently in the journal. A SUCCESSFUL action is removed from the journal; a failed one (including "taskbar
/// not found") is KEPT to retry later (next startup, or the guard process when the app exits) rather than being dropped while
/// Windows is still in the state the app changed. One failing action does not block the others. Returns the failed actions.
pub fn replay(journal: &mut GuardJournal, perform: &mut dyn FnMut(RestoreAction) -> io::Result<()>) -> Vec<(RestoreAction, io::Error)> {
    let mut failed = Vec::new();
    for a in journal.actions() {
        match perform(a) {
            Ok(()) => {
                tracing::info!(target: "sg::guard", "restored {a:?}");
                if let Err(e) = journal.remove(a) {
                    tracing::warn!(target: "sg::guard", "restored {a:?} but cannot update the journal: {e}");
                }
            }
            Err(e) => {
                tracing::warn!(target: "sg::guard", "restore {a:?} failed, keeping it in the journal: {e}");
                failed.push((a, e));
            }
        }
    }
    failed
}

/// Clean exit (every part has already put Windows back as it was): finish whatever is STILL in the journal (leftovers from a
/// previous run, or a restore of another part that failed during the session) using `replay`. Successful actions are removed from
/// the journal; failed ones are KEPT so the guard process (which sees EOF right after) and the next startup carry on. The whole
/// journal is not cleared and rewritten, so no entry is lost and there is no gap between clearing and writing. Returns the
/// actions that remain.
pub fn finish_clean_exit(journal: &mut GuardJournal, perform: &mut dyn FnMut(RestoreAction) -> io::Result<()>) -> Vec<RestoreAction> {
    replay(journal, perform).into_iter().map(|(a, _)| a).collect()
}

/// Main loop of the guard process: wait for EOF on `input`, then finish the journal's actions (see `replay`:
/// failed actions are kept in the journal).
pub fn run_guard(
    mut input: impl Read,
    journal_path: &Path,
    perform: &mut dyn FnMut(RestoreAction) -> io::Result<()>,
) -> GuardOutcome {
    let mut buf = [0u8; 256];
    loop {
        match input.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let mut journal = GuardJournal::open(journal_path);
    let actions = journal.actions();
    if actions.is_empty() {
        // We cannot tell whether the app exited cleanly or was killed; we only know nothing is left to restore.
        tracing::info!(target: "sg::guard", "main process exited; journal empty, nothing to restore");
        return GuardOutcome::Clean;
    }
    let failed = replay(&mut journal, perform);
    tracing::info!(target: "sg::guard", "main process died; restored {} of {}", actions.len() - failed.len(), actions.len());
    GuardOutcome::Restored(actions.len() - failed.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn journal_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state").join("guard.toml");
        let mut j = GuardJournal::open(&p);
        j.add(RestoreAction::ShowNativeTaskbar).unwrap();
        j.add(RestoreAction::ShowNativeTaskbar).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.matches("show-native-taskbar").count(), 1, "{text}");
        let j2 = GuardJournal::open(&p);
        assert_eq!(j2.actions(), vec![RestoreAction::ShowNativeTaskbar]);
        let mut j2 = j2;
        j2.remove(RestoreAction::ShowNativeTaskbar).unwrap();
        assert!(GuardJournal::open(&p).actions().is_empty());
    }

    #[test]
    fn restore_labels_action_serializes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        let mut j = GuardJournal::open(&p);
        j.add(RestoreAction::RestoreDesktopLabels).unwrap();
        j.add(RestoreAction::ShowNativeTaskbar).unwrap();
        j.add(RestoreAction::DisableTaskbarAutohide).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("restore-desktop-labels"), "{text}");
        assert!(text.contains("disable-taskbar-autohide"), "{text}");
        assert_eq!(GuardJournal::open(&p).actions().len(), 3);
    }

    #[test]
    fn journal_corrupt_is_empty() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        std::fs::write(&p, b"\x00\x01 rac [[").unwrap();
        assert!(GuardJournal::open(&p).actions().is_empty());
    }

    #[test]
    fn guard_eof_with_entries_restores() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        GuardJournal::open(&p).add(RestoreAction::ShowNativeTaskbar).unwrap();
        let mut done = Vec::new();
        let out = run_guard(&b""[..], &p, &mut |a| {
            done.push(a);
            Ok(())
        });
        assert_eq!(out, GuardOutcome::Restored(1));
        assert_eq!(done, vec![RestoreAction::ShowNativeTaskbar]);
        assert!(GuardJournal::open(&p).actions().is_empty());
    }

    #[test]
    fn guard_restores_taskbar_then_autohide() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        let mut j = GuardJournal::open(&p);
        j.add(RestoreAction::DisableTaskbarAutohide).unwrap();
        j.add(RestoreAction::ShowNativeTaskbar).unwrap();
        let mut done = Vec::new();
        let out = run_guard(&b""[..], &p, &mut |a| {
            done.push(a);
            Ok(())
        });
        assert_eq!(out, GuardOutcome::Restored(2));
        // show the taskbar first, then turn auto-hide off
        assert_eq!(done, vec![RestoreAction::ShowNativeTaskbar, RestoreAction::DisableTaskbarAutohide]);
    }

    #[test]
    fn guard_clean_exit() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        let mut called = 0;
        let out = run_guard(&b""[..], &p, &mut |_| {
            called += 1;
            Ok(())
        });
        assert_eq!(out, GuardOutcome::Clean);
        assert_eq!(called, 0);
    }

    #[test]
    fn guard_waits_for_eof() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        let input = Cursor::new(b"rac\nrac nua\n".to_vec());
        // The journal is written by the "main app" while the guard process is reading: here we write it first,
        // because Cursor returns EOF right after the data. What we check: garbage data does not make the guard exit early.
        GuardJournal::open(&p).add(RestoreAction::ShowNativeTaskbar).unwrap();
        let out = run_guard(input, &p, &mut |_| Ok(()));
        assert_eq!(out, GuardOutcome::Restored(1));
    }

    #[test]
    fn guard_perform_error_continues() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        GuardJournal::open(&p).add(RestoreAction::ShowNativeTaskbar).unwrap();
        let out = run_guard(&b""[..], &p, &mut |_| Err(std::io::Error::other("hỏng")));
        assert_eq!(out, GuardOutcome::Restored(0), "không việc nào thành công");
        // The failed action must stay in the journal to be retried.
        assert_eq!(GuardJournal::open(&p).actions(), vec![RestoreAction::ShowNativeTaskbar]);
    }

    #[test]
    fn clean_exit_replays_what_is_in_the_journal_and_keeps_failures() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        let mut j = GuardJournal::open(&p);
        // One action succeeds, one still fails, and one was left by another part mid-session (restore error): no entry is lost.
        j.add(RestoreAction::ShowNativeTaskbar).unwrap();
        j.add(RestoreAction::DisableTaskbarAutohide).unwrap();
        j.add(RestoreAction::RestoreDesktopLabels).unwrap();
        let mut tried = Vec::new();
        let still = finish_clean_exit(&mut j, &mut |a| {
            tried.push(a);
            if a == RestoreAction::ShowNativeTaskbar { Ok(()) } else { Err(std::io::Error::other("vẫn chưa làm được")) }
        });
        assert_eq!(tried.len(), 3, "thử đúng những gì còn trong nhật ký");
        let both = [RestoreAction::DisableTaskbarAutohide, RestoreAction::RestoreDesktopLabels];
        assert!(still.len() == 2 && both.iter().all(|a| still.contains(a)), "{still:?}");
        let kept = GuardJournal::open(&p).actions();
        assert!(kept.len() == 2 && both.iter().all(|a| kept.contains(a)), "{kept:?}");
        // An entry resolved during the session (already removed from the journal) is NOT retried.
        j.remove(RestoreAction::DisableTaskbarAutohide).unwrap();
        let mut tried = Vec::new();
        let still = finish_clean_exit(&mut j, &mut |a| {
            tried.push(a);
            Ok(())
        });
        assert_eq!(tried, vec![RestoreAction::RestoreDesktopLabels]);
        assert!(still.is_empty() && GuardJournal::open(&p).actions().is_empty());
    }

    #[test]
    fn replay_keeps_failed_actions_and_removes_done_ones() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("guard.toml");
        let mut j = GuardJournal::open(&p);
        j.add(RestoreAction::ShowNativeTaskbar).unwrap();
        j.add(RestoreAction::DisableTaskbarAutohide).unwrap();
        j.add(RestoreAction::RestoreDesktopLabels).unwrap();
        // "Taskbar not found": the two taskbar actions fail, the remaining one is still performed.
        let failed = replay(&mut j, &mut |a| match a {
            RestoreAction::RestoreDesktopLabels => Ok(()),
            _ => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "native taskbar not found")),
        });
        let kinds: Vec<_> = failed.iter().map(|(a, _)| *a).collect();
        assert_eq!(kinds, vec![RestoreAction::ShowNativeTaskbar, RestoreAction::DisableTaskbarAutohide]);
        assert_eq!(GuardJournal::open(&p).actions(), kinds);
        // A later attempt (with the taskbar present) succeeds, so the journal ends up empty.
        assert!(replay(&mut j, &mut |_| Ok(())).is_empty());
        assert!(GuardJournal::open(&p).actions().is_empty());
    }
}
