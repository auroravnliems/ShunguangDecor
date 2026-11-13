//! Commands for the NSIS installer. They run BEFORE Tauri like `--guard`: no window, no single-instance.
//!
//! - `--quit`: tells this user's running app to exit cleanly (exactly like clicking "Quit" in the tray: disable the parts, restore
//!   Windows to how it was), then waits for every other `ShunguangDecor.exe` process in the same sign-in session (app, guard process,
//!   admin helper process) to exit, for at most [`QUIT_WAIT`]. The installer calls it before overwriting the exe.
//! - `--uninstall-cleanup`: `--quit`, then finish whatever the app may have left behind in Windows (uninstalling the app also restores
//!   the cursors…). A failing step is logged and the next step still runs; always exits with code 0 so the uninstaller never gets stuck.
//!   Hidden features are NOT undone automatically (persistent settings, the user decides) — the uninstaller shows a separate reminder.
//!
//! The running app listens on an event named `Local\ShunguangDecor.Quit.<user SID>` ([`listen_for_quit`]).

use sg_core::paths::Paths;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

pub const QUIT_ARG: &str = "--quit";
pub const CLEANUP_ARG: &str = "--uninstall-cleanup";
pub const FEATURES_PENDING_ARG: &str = "--features-pending";
/// How long to wait at most for the other processes to exit. The app disables the parts in at most 3 s (`shutdown`), plus the guard process.
pub const QUIT_WAIT: Duration = Duration::from_secs(10);
/// Name of the value under `HKCU\…\Run`. tauri-plugin-autostart sets it from `package_info().name` = `productName`.
const RUN_VALUE: &str = "ShunguangDecor";

pub fn is_quit(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a == QUIT_ARG)
}

pub fn is_uninstall_cleanup(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a == CLEANUP_ARG)
}

pub fn is_features_pending(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a == FEATURES_PENDING_ARG)
}

/// Whether the Hidden features undo journal still has entries (corrupt = treated as non-empty: changes may remain, better to remind).
pub fn features_pending(journal: &Path) -> bool {
    match sg_features::undo::UndoJournal::open(journal) {
        Ok(j) => !j.entries().is_empty(),
        Err(_) => true,
    }
}

/// `ShunguangDecor.exe --features-pending`: exit code 1 = Hidden features changes remain that have not been undone (the uninstaller reminds the user).
pub fn run_features_pending() -> i32 {
    let (paths, _) = Paths::detect();
    i32::from(features_pending(&sg_features::undo::journal_path(&paths)))
}

/// A named cleanup step (for logging).
pub struct Step<'a> {
    name: &'static str,
    run: Box<dyn FnMut() -> Result<(), String> + 'a>,
}

impl<'a> Step<'a> {
    pub fn new(name: &'static str, run: impl FnMut() -> Result<(), String> + 'a) -> Step<'a> {
        Step { name, run: Box::new(run) }
    }
}

/// Runs EVERY step in turn; a step that errors or panics does not block the next one. Returns the failed steps with their reasons.
pub fn run_steps(steps: Vec<Step<'_>>) -> Vec<(&'static str, String)> {
    let mut failed = Vec::new();
    for mut s in steps {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (s.run)()));
        let err = match outcome {
            Ok(Ok(())) => {
                tracing::info!(target: "sg::installer", "cleanup step {}: ok", s.name);
                continue;
            }
            Ok(Err(e)) => e,
            Err(p) => {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                format!("panicked: {msg}")
            }
        };
        tracing::warn!(target: "sg::installer", "cleanup step {} failed: {err}", s.name);
        failed.push((s.name, err));
    }
    failed
}

/// The Run entry was just removed on uninstall → the next install is treated as a first run and re-enables "Start with Windows" (the
/// default). If the user had turned it off there is no Run entry → this function is not called → a reinstall stays off.
/// No `app.toml`, or the flag was already off → writes nothing. File unreadable → reports an error, does not overwrite.
pub fn forget_autostart_initialized(app_toml: &Path) -> io::Result<()> {
    if !app_toml.exists() {
        return Ok(());
    }
    let report = sg_core::config::load(app_toml);
    if report.must_not_overwrite {
        return Err(io::Error::other("app.toml cannot be read; not touching it"));
    }
    if !report.config.autostart_initialized {
        return Ok(());
    }
    let mut cfg = report.config;
    cfg.autostart_initialized = false;
    sg_core::config::save(app_toml, &cfg)
}

/// Main app: listens for the installer's quit event. `on_quit` runs on a background thread (the caller hops back to the main thread itself).
pub fn listen_for_quit(on_quit: impl FnOnce() + Send + 'static) -> io::Result<()> {
    sys::listen_for_quit(Box::new(on_quit))
}

/// Tells the running app to quit, then waits for the other processes to exit. Error = processes still remain after [`QUIT_WAIT`].
fn quit_running_and_wait() -> Result<(), String> {
    match sys::signal_quit() {
        Ok(true) => tracing::info!(target: "sg::installer", "asked the running app to quit"),
        Ok(false) => tracing::info!(target: "sg::installer", "no running app is listening"),
        Err(e) => tracing::warn!(target: "sg::installer", "cannot signal the running app: {e}"),
    }
    let deadline = Instant::now() + QUIT_WAIT;
    loop {
        let left = sys::other_instances();
        if left.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("still running after {} s: pid {left:?}", QUIT_WAIT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `ShunguangDecor.exe --quit`
pub fn run_quit() -> i32 {
    let (paths, _) = Paths::detect();
    let _log = sg_core::logging::init(&paths.logs_dir(), "installer");
    tracing::info!(target: "sg::installer", "--quit");
    if let Err(e) = quit_running_and_wait() {
        tracing::warn!(target: "sg::installer", "{e}");
    }
    0
}

/// `ShunguangDecor.exe --uninstall-cleanup`
pub fn run_uninstall_cleanup() -> i32 {
    let (paths, _) = Paths::detect();
    let _log = sg_core::logging::init(&paths.logs_dir(), "installer");
    tracing::info!(target: "sg::installer", "--uninstall-cleanup ({})", if paths.portable { "portable" } else { "installed" });
    let steps = vec![
        Step::new("quit", quit_running_and_wait),
        // Work the app (or the guard process) has not finished restoring. Runs AFTER every other process has exited, so there is no contention.
        Step::new("guard-journal", || {
            let mut journal = sg_core::guard::GuardJournal::open(&paths.guard_journal_file());
            let failed = sg_core::guard::replay(&mut journal, &mut sg_core::guard::perform);
            if failed.is_empty() { Ok(()) } else { Err(format!("{failed:?}")) }
        }),
        // Quitting the app keeps the cursors, but uninstalling the app restores the originals.
        Step::new("cursor", || restore_cursor(&paths)),
        Step::new("wallpaper", || {
            let file = sg_wallpaper::WallpaperPaths::from_paths(&paths).backup_file;
            sys::with_com(|| sg_wallpaper::backup::restore_and_clear(&file)).map(|_| ())
        }),
        Step::new("desktop-labels", || {
            sg_core::labels_backup::restore(&sg_core::labels_backup::path(&paths)).map(|_| ()).map_err(|e| e.to_string())
        }),
        // No journal needed: showing a taskbar that is already showing does nothing.
        Step::new("native-taskbar", || sg_core::platform::show_native_taskbar().map_err(|e| e.to_string())),
        Step::new("autostart", || {
            if sys::delete_run_value(RUN_VALUE).map_err(|e| e.to_string())? {
                forget_autostart_initialized(&paths.app_config_file()).map_err(|e| e.to_string())?;
            }
            Ok(())
        }),
        // Windows will not delete a DLL loaded into Explorer until Explorer restarts: this step failing is normal.
        Step::new("dll-folder", || match std::fs::remove_dir_all(sg_explorer::bin_dir(&paths)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }),
    ];
    let failed = run_steps(steps);
    tracing::info!(target: "sg::installer", "uninstall cleanup done, {} step(s) failed", failed.len());
    0
}

/// Writes the original cursor registry values back from `backup/cursor-original.toml`, then deletes the backup. No backup → nothing to do.
/// Corrupt backup → reports an error and KEEPS the file (no guessing).
fn restore_cursor(paths: &Paths) -> Result<(), String> {
    let Some(original) = sg_cursor::backup::take_backup(paths).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    sg_cursor::platform::write_registry(&original).map_err(|e| e.to_string())?;
    sg_cursor::platform::reload_cursors(original.base_size).map_err(|e| e.to_string())?;
    sg_cursor::backup::clear(paths);
    Ok(())
}

mod sys {
    use std::ffi::c_void;
    use std::io;
    use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{
        CreateEventW, EVENT_MODIFY_STATE, GetCurrentProcess, GetCurrentProcessId, INFINITE, OpenEventW,
        OpenProcessToken, SetEvent, WaitForSingleObject,
    };
    use windows::core::{HSTRING, PWSTR};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by us and is closed exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn user_sid_string() -> io::Result<String> {
        // SAFETY: calls the Win32 API with a buffer we provide; the token is closed when `Owned` drops.
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
            let token = Owned(token);
            let mut len = 0u32;
            let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut len);
            if len == 0 {
                return Err(io::Error::other("GetTokenInformation returned no size"));
            }
            // u64 to guarantee alignment for TOKEN_USER.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            GetTokenInformation(token.0, TokenUser, Some(buf.as_mut_ptr().cast::<c_void>()), len, &mut len)
                .map_err(io::Error::other)?;
            let tu = &*(buf.as_ptr().cast::<TOKEN_USER>());
            let mut s = PWSTR::null();
            ConvertSidToStringSidW(tu.User.Sid, &mut s).map_err(io::Error::other)?;
            let out = s.to_string().map_err(io::Error::other);
            let _ = LocalFree(Some(HLOCAL(s.0.cast())));
            out
        }
    }

    /// `Local\` = per sign-in session; SID = per user. The token's default DACL: other users cannot open it.
    fn event_name() -> io::Result<HSTRING> {
        Ok(HSTRING::from(format!(r"Local\ShunguangDecor.Quit.{}", user_sid_string()?)))
    }

    pub fn listen_for_quit(on_quit: Box<dyn FnOnce() + Send>) -> io::Result<()> {
        let name = event_name()?;
        // SAFETY: valid name; auto-reset, initially unsignaled.
        let h = unsafe { CreateEventW(None, false, false, &name) }.map_err(io::Error::other)?;
        // HANDLE is not Send: pass it as an integer. The handle lives until the process ends (intentionally never closed).
        let raw = h.0 as isize;
        std::thread::Builder::new().name("sg-quit-listener".into()).spawn(move || {
            // SAFETY: the event handle stays open for the whole process lifetime.
            unsafe { WaitForSingleObject(HANDLE(raw as *mut c_void), INFINITE) };
            tracing::info!(target: "sg::app", "installer asked the app to quit");
            on_quit();
        })?;
        Ok(())
    }

    /// `Ok(false)` = no app is listening (nobody has created the event).
    pub fn signal_quit() -> io::Result<bool> {
        let name = event_name()?;
        // SAFETY: only requests the right to signal the event.
        match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) } {
            Ok(h) => {
                let h = Owned(h);
                // SAFETY: a valid handle that was just opened.
                unsafe { SetEvent(h.0) }.map_err(io::Error::other)?;
                Ok(true)
            }
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => Ok(false),
            Err(e) => Err(io::Error::other(e)),
        }
    }

    /// PIDs of the processes with the same exe name as this process, in the same sign-in session, excluding itself.
    pub fn other_instances() -> Vec<u32> {
        let Some(me) = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        else {
            return Vec::new();
        };
        // SAFETY: simple query functions, with structures provided by us.
        unsafe {
            let my_pid = GetCurrentProcessId();
            let mut my_session = 0u32;
            let _ = ProcessIdToSessionId(my_pid, &mut my_session);
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return Vec::new() };
            let snap = Owned(snap);
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut out = Vec::new();
            let mut ok = Process32FirstW(snap.0, &mut e).is_ok();
            while ok {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
                let mut session = u32::MAX;
                if name == me
                    && e.th32ProcessID != my_pid
                    && ProcessIdToSessionId(e.th32ProcessID, &mut session).is_ok()
                    && session == my_session
                {
                    out.push(e.th32ProcessID);
                }
                ok = Process32NextW(snap.0, &mut e).is_ok();
            }
            out
        }
    }

    /// Deletes the autostart value (including the Task Manager enable/disable entry). `Ok(true)` = the Run entry existed.
    pub fn delete_run_value(name: &str) -> io::Result<bool> {
        let value = HSTRING::from(name);
        // SAFETY: the HKCU root key is always open; the strings are valid.
        let run = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &value) };
        // SAFETY: as above. The Task Manager entry may not exist: ignore any result.
        let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(APPROVED_KEY), &value) };
        if run.is_ok() {
            Ok(true)
        } else if run == ERROR_FILE_NOT_FOUND {
            Ok(false)
        } else {
            Err(io::Error::from_raw_os_error(run.0 as i32))
        }
    }

    /// Runs `f` with COM initialized on this thread (changing the wallpaper uses `IDesktopWallpaper`).
    pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
        // SAFETY: initializes COM for the current thread; uninitialized exactly once if initialization succeeded.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let out = f();
        if hr.is_ok() {
            // SAFETY: pairs with the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        out
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flag_detection_only_first_argument() {
        assert!(is_quit(&args(&["exe", "--quit"])));
        assert!(!is_quit(&args(&["exe", "--tray", "--quit"])));
        assert!(is_uninstall_cleanup(&args(&["exe", "--uninstall-cleanup"])));
        assert!(!is_uninstall_cleanup(&args(&["exe"])));
        assert!(!is_uninstall_cleanup(&args(&["exe", "--quit"])));
    }

    #[test]
    fn cleanup_runs_every_step_even_if_one_fails() {
        let ran = RefCell::new(Vec::new());
        let steps = vec![
            Step::new("a", || {
                ran.borrow_mut().push("a");
                Ok(())
            }),
            Step::new("b", || {
                ran.borrow_mut().push("b");
                Err("boom".to_string())
            }),
            Step::new("c", || {
                ran.borrow_mut().push("c");
                panic!("panic in c")
            }),
            Step::new("d", || {
                ran.borrow_mut().push("d");
                Ok(())
            }),
        ];
        let failed = run_steps(steps);
        assert_eq!(*ran.borrow(), ["a", "b", "c", "d"]);
        let names: Vec<_> = failed.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["b", "c"]);
        assert_eq!(failed[0].1, "boom");
        assert!(failed[1].1.contains("panic in c"), "{}", failed[1].1);
    }

    #[test]
    fn forgetting_autostart_resets_first_run_flag_and_keeps_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config").join("app.toml");
        let mut cfg = sg_core::config::AppConfig { autostart_initialized: true, ..Default::default() };
        cfg.modules_enabled.insert(sg_core::ids::ModuleId::Cursor, true);
        sg_core::config::save(&p, &cfg).unwrap();

        forget_autostart_initialized(&p).unwrap();

        let back = sg_core::config::load(&p).config;
        assert!(!back.autostart_initialized);
        assert_eq!(back.modules_enabled.get(&sg_core::ids::ModuleId::Cursor), Some(&true));
    }

    #[test]
    fn features_pending_follows_the_undo_journal() {
        use sg_features::model::{Priority, Store};
        use sg_features::undo::{UndoEntry, UndoJournal};
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("backup").join("features-undo.toml");
        assert!(!features_pending(&p), "no journal");
        UndoJournal::open(&p)
            .unwrap()
            .record(UndoEntry { unix: 0, store: Store::Runtime, original: None, id: 7, priority: Priority::User })
            .unwrap();
        assert!(features_pending(&p), "one entry");
        UndoJournal::open(&p).unwrap().clear().unwrap();
        assert!(!features_pending(&p), "cleared");
        // Corrupt: changes may remain → still remind the user.
        std::fs::write(&p, "[[entry]]\nnot toml at all").unwrap();
        assert!(features_pending(&p), "corrupt");
    }

    #[test]
    fn features_pending_flag_detection() {
        assert!(is_features_pending(&args(&["exe", "--features-pending"])));
        assert!(!is_features_pending(&args(&["exe", "--quit"])));
    }

    #[test]
    fn forgetting_autostart_without_app_toml_does_nothing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config").join("app.toml");
        forget_autostart_initialized(&p).unwrap();
        assert!(!p.exists());
    }
}
