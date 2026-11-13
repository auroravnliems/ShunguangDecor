//! The ShunguangDecor app shell: system tray, windows created/destroyed on demand, commands, events, `sg://`.

mod commands;
mod guard_proc;
mod lifecycle;
mod modules;
mod protocol;
mod state;
mod tray;
mod window;

use sg_core::config;
use sg_core::guard::GuardJournal;
use sg_core::host::{HostEvent, ModuleHost};
use sg_core::i18n::{os_locale, pick_language};
use sg_core::module::{Availability, ModuleCtx};
use sg_core::paths::Paths;
use sg_core::profiles::ProfileStore;
use state::{AppState, lock};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::ManagerExt as _;

/// Windows launches the app at sign-in with this argument: it runs hidden in the tray and opens no window.
pub const TRAY_ARG: &str = "--tray";

/// Whether to open a window (not when launched with `--tray`).
pub fn wants_window(args: &[String]) -> bool {
    !args.iter().any(|a| a == TRAY_ARG)
}

/// `ShunguangDecor.exe --guard …`
pub fn is_guard(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a == guard_proc::GUARD_ARG)
}

pub fn run_guard(args: &[String]) -> i32 {
    guard_proc::run(args)
}

/// `ShunguangDecor.exe --elevated-helper --pipe <name> --parent <pid>` (the admin-privileged helper process of the Hidden features part).
pub fn is_elevated_helper(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| a == sg_features::helper::HELPER_ARG)
}

/// Runs the helper loop then exits; does not start Tauri and opens no window.
pub fn run_elevated_helper(args: &[String]) -> i32 {
    sg_features::helper::run(args)
}

/// Installer commands (`--quit`, `--uninstall-cleanup`, `--features-pending`; see `lifecycle.rs`).
/// `Some(exit code)` = a command ran and the process must exit immediately; `None` = open the app as usual.
pub fn run_installer_command(args: &[String]) -> Option<i32> {
    if lifecycle::is_quit(args) {
        Some(lifecycle::run_quit())
    } else if lifecycle::is_uninstall_cleanup(args) {
        Some(lifecycle::run_uninstall_cleanup())
    } else if lifecycle::is_features_pending(args) {
        Some(lifecycle::run_features_pending())
    } else {
        None
    }
}

/// Whether the first run enables "Start with Windows" automatically.
/// Not for the portable build: the exe may be on a USB drive that is no longer plugged in at boot.
/// Not for debug builds: avoids writing a Run entry that points into `target\debug`.
fn should_enable_autostart_first_run(initialized: bool, portable: bool, debug: bool) -> bool {
    !initialized && !portable && !debug
}

fn on_host_event(app: &AppHandle, ev: HostEvent) {
    let Some(state) = app.try_state::<AppState>() else { return };
    match ev {
        HostEvent::Changed(info) => {
            // The configuration always follows the real state, even for a result that arrives after the call already reported a timeout.
            let saved = lock(&state.config).modules_enabled.get(&info.id).copied().unwrap_or(false);
            if saved != info.enabled {
                state.update_config(|c| c.modules_enabled.insert(info.id, info.enabled));
            }
            if let Some(tray) = state.tray() {
                tray.sync(&info, state.lang());
            }
            window::emit_main(app, "module-changed", info);
        }
        HostEvent::Faulted { id, message } => {
            tracing::error!(target: "sg::app", "{id} faulted and was turned off: {message}");
            state.update_config(|c| c.modules_enabled.insert(id, false));
            window::emit_main(app, "module-faulted", serde_json::json!({ "id": id, "message": message }));
        }
    }
}

fn setup(app: &mut tauri::App, paths: Paths, cfg: config::AppConfig, config_frozen: bool, open_window: bool) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();

    // Leftover temp directories from a previous run killed mid-import: delete them (single-instance, so no other import is affected).
    sg_core::transfer::purge_tmp(&paths);

    // If the app (and the guard process) died last time with a restore unfinished → do it now.
    let mut journal = GuardJournal::open(&paths.guard_journal_file());
    if !journal.actions().is_empty() {
        tracing::warn!(target: "sg::app", "leftover restore journal from last run: {:?}", journal.actions());
        // Tasks that succeed are removed from the journal; failed ones ("taskbar not found"…) are KEPT in the journal and retried
        // on a clean exit (and by the guard process), never dropped, so Windows is not left stuck in a state the app changed.
        for (a, e) in sg_core::guard::replay(&mut journal, &mut sg_core::guard::perform) {
            tracing::warn!(target: "sg::app", "leftover {a:?} could not be restored yet ({e}); kept in the journal to retry");
        }
    }

    let journal = Arc::new(Mutex::new(journal));
    let h = handle.clone();
    let emit = Arc::new(move |event: &str, payload: serde_json::Value| window::emit_main(&h, event, payload));
    let ctx = ModuleCtx::new(paths.clone(), journal.clone(), emit);
    // Work the YASB taskbar asks the app to do. It runs on the engine's thread so it must not block: hand it off to another thread.
    let h = handle.clone();
    let turn_off: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let h = h.clone();
        // Exactly as if the user flipped the switch: `app.toml` + UI + tray all update together.
        tauri::async_runtime::spawn_blocking(move || {
            if let Some(state) = h.try_state::<AppState>()
                && let Err(e) = commands::toggle_module(&state, sg_core::ids::ModuleId::Taskbar, false)
            {
                tracing::warn!(target: "sg::app", "cannot turn the taskbar part off: {}", e.key);
            }
        });
    });
    let h = handle.clone();
    let open_app: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let h2 = h.clone();
        if let Err(e) = h.run_on_main_thread(move || window::open_main(&h2)) {
            tracing::warn!(target: "sg::app", "cannot open the main window from the bar: {e}");
        }
    });
    let lang_handle = handle.clone();
    let lang: Arc<dyn Fn() -> sg_core::i18n::Lang + Send + Sync> =
        Arc::new(move || lang_handle.try_state::<AppState>().map(|s| s.lang()).unwrap_or(sg_core::i18n::Lang::En));
    let hooks = sg_taskbar::Hooks { turn_off, open_app, watch_explorer: Arc::new(|| {}) };
    let (modules, services) = modules::build(&ctx, lang, hooks);
    let h = handle.clone();
    let on_event = Arc::new(move |ev: HostEvent| on_host_event(&h, ev));
    let host = ModuleHost::new(modules, on_event);

    let state = AppState {
        profiles: ProfileStore::new(paths.clone()),
        journal,
        services,
        config: Mutex::new(cfg.clone()),
        config_frozen,
        host,
        pending_intent: Mutex::new(None),
        tray: Mutex::new(None),
        thumb: Mutex::new(None),
        quitting: AtomicBool::new(false),
        shut_down: AtomicBool::new(false),
        tray_notice_asked: AtomicBool::new(false),
        guard_child: Mutex::new(guard_proc::spawn(&paths.guard_journal_file())),
        paths,
    };
    app.manage(state);
    let state = app.state::<AppState>();

    // Re-enable the parts the user had enabled last time (any part that cannot be enabled is recorded as off).
    for id in cfg.modules_enabled.iter().filter(|(_, on)| **on).map(|(id, _)| id) {
        if matches!(state.host.info(*id).availability, Availability::Unsupported { .. }) {
            continue;
        }
        if let Err(e) = commands::toggle_module(&state, *id, true) {
            tracing::warn!(target: "sg::app", "cannot re-enable {id}: {e}");
        }
    }

    let tray = tray::build(&handle, state.lang(), &state.host.infos())?;
    *lock(&state.tray) = Some(tray);

    // "Start with Windows" is on by default at first run. Debug and portable builds leave it alone.
    if should_enable_autostart_first_run(cfg.autostart_initialized, state.paths.portable, cfg!(debug_assertions)) {
        match handle.autolaunch().enable() {
            Ok(()) => tracing::info!(target: "sg::app", "autostart enabled (first run)"),
            Err(e) => tracing::warn!(target: "sg::app", "cannot enable autostart: {e}"),
        }
        state.update_config(|c| c.autostart_initialized = true);
    }

    // The installer (`ShunguangDecor.exe --quit`) requests exit → exit cleanly exactly as if "Quit" was clicked in the tray.
    let h = handle.clone();
    let listening = lifecycle::listen_for_quit(move || {
        let h2 = h.clone();
        if let Err(e) = h.run_on_main_thread(move || quit(&h2)) {
            tracing::warn!(target: "sg::app", "cannot quit for the installer: {e}");
        }
    });
    if let Err(e) = listening {
        tracing::warn!(target: "sg::app", "cannot listen for the installer's quit request: {e}");
    }

    if open_window {
        window::open_main(&handle);
    }
    Ok(())
}

/// Full quit (tray menu): disable the parts, delete the restore journal, then exit.
pub fn quit(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.quitting.store(true, Ordering::SeqCst);
    }
    shutdown(app);
    app.exit(0);
}

fn shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    if state.shut_down.swap(true, Ordering::SeqCst) {
        return;
    }
    tracing::info!(target: "sg::app", "shutting down");
    let unfinished = state.host.shutdown(Duration::from_secs(3));
    if unfinished.is_empty() {
        // Every part has already restored Windows itself: finish whatever remains in the journal, and keep anything that still fails.
        let mut journal = lock(&state.journal);
        let still = sg_core::guard::finish_clean_exit(&mut journal, &mut sg_core::guard::perform);
        if !still.is_empty() {
            tracing::warn!(target: "sg::app", "restore journal keeps unfinished actions: {still:?}");
        }
    } else {
        // Some part has not restored Windows yet: keep the journal so the guard process finishes the job when it sees EOF.
        tracing::warn!(target: "sg::app", "keeping restore journal for the guard; unfinished: {unfinished:?}");
    }
    // Close the stdin pipe right away: the guard process sees EOF with an empty journal → exits by itself, without waiting for the app to fully quit.
    if let Some(mut child) = lock(&state.guard_child).take() {
        drop(child.stdin.take());
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let (paths, path_warnings) = Paths::detect();
    let _log = sg_core::logging::init(&paths.logs_dir(), "core");
    tracing::info!(
        target: "sg::app",
        "ShunguangDecor {} starting on {} ({})",
        commands::VERSION,
        sg_core::platform::os_description(),
        if paths.portable { "portable" } else { "installed" }
    );
    for w in path_warnings {
        tracing::warn!(target: "sg::app", "{w}");
    }
    let report = config::load(&paths.app_config_file());
    for w in &report.warnings {
        tracing::warn!(target: "sg::app", "app.toml: {w}");
    }
    let config_frozen = report.must_not_overwrite;
    let mut cfg = report.config;
    if cfg.language.is_none() {
        let os = os_locale();
        let lang = pick_language(&os);
        tracing::info!(target: "sg::app", "first run: OS locale {os:?} → language {}", lang.code());
        cfg.language = Some(lang);
        if config_frozen {
            tracing::warn!(target: "sg::app", "app.toml exists but could not be read or backed up; not overwriting it this session");
        } else if let Err(e) = config::save(&paths.app_config_file(), &cfg) {
            tracing::warn!(target: "sg::app", "cannot save app.toml: {e}");
        }
    }
    let open_window = wants_window(&args);

    let app = tauri::Builder::default()
        // single-instance must come first: launching the exe again makes the running instance open its window.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if wants_window(&args) {
                window::open_main(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![TRAY_ARG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("sg", protocol::handle)
        .setup(move |app| setup(app, paths, cfg, config_frozen, open_window))
        .on_window_event(window::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_settings,
            commands::set_language,
            commands::set_autostart,
            commands::set_reduce_motion,
            commands::set_follow_os_motion,
            commands::list_modules,
            commands::set_module_enabled,
            commands::module_get_config,
            commands::module_set_config,
            commands::get_runtime_stats,
            commands::get_current_wallpaper,
            commands::profile_list,
            commands::profile_save,
            commands::profile_load,
            commands::profile_delete,
            commands::profile_export,
            commands::profile_import,
            commands::export_all,
            commands::import_all,
            commands::restore_all_os_defaults,
            commands::open_data_dir,
            commands::open_link,
            commands::export_bug_report,
            commands::take_pending_intent,
            commands::ack_tray_notice,
            commands::log_frontend,
            modules::capture::list_monitors,
            modules::capture::capture_start,
            modules::capture::capture_stop,
            modules::capture::capture_live_source,
            modules::wallpaper::wallpaper_library,
            modules::wallpaper::wallpaper_add,
            modules::wallpaper::wallpaper_import,
            modules::wallpaper::wallpaper_delete,
            modules::wallpaper::wallpaper_rename,
            modules::wallpaper::wallpaper_state,
            modules::wallpaper::wallpaper_toggle_pause,
            modules::wallpaper::wallpaper_properties,
            modules::wallpaper::wallpaper_set_property,
            modules::wallpaper::wallpaper_reset_properties,
            modules::wallpaper::wallpaper_find_mpv,
            modules::wallpaper::wallpaper_debug_taskbar_created,
            modules::labels::labels_status,
            modules::labels::explorer_restart,
            modules::labels::labels_debug_taskbar_created,
            modules::features::features_search,
            modules::features::features_list,
            modules::features::features_web_search,
            modules::features::features_get,
            modules::features::features_suggestions,
            modules::features::features_subscriptions,
            modules::features::features_undo_list,
            modules::features::features_export,
            modules::features::features_elevate,
            modules::features::features_set,
            modules::features::features_reset,
            modules::features::features_import,
            modules::features::features_undo_all,
            modules::features::features_fix_priority,
            modules::features::features_notify_usage,
            modules::features::features_release,
            modules::taskbar::taskbar_widget_catalog,
            modules::taskbar::taskbar_status,
            modules::taskbar::taskbar_raw_state,
            modules::taskbar::taskbar_open_raw,
            modules::taskbar::taskbar_raw_choose,
            modules::taskbar::taskbar_import_yasb,
            modules::taskbar::taskbar_list_fonts,
            modules::taskbar::taskbar_debug_taskbar_created,
            modules::cursor::cursor_list,
            modules::cursor::cursor_detail,
            modules::cursor::cursor_import,
            modules::cursor::cursor_rename,
            modules::cursor::cursor_delete,
            modules::cursor::cursor_set_role,
        ])
        .build(tauri::generate_context!());

    let app = match app {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(target: "sg::app", "cannot start: {e}");
            return;
        }
    };
    app.run(|app, ev| match ev {
        // Closing the last window does not quit the app: it keeps running in the tray.
        RunEvent::ExitRequested { code: None, api, .. } => {
            let quitting = app.try_state::<AppState>().is_some_and(|s| s.quitting.load(Ordering::SeqCst));
            if !quitting {
                api.prevent_exit();
            }
        }
        RunEvent::Exit => shutdown(app),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wants_window_cases() {
        assert!(wants_window(&args(&[])));
        assert!(wants_window(&args(&["C:/x/ShunguangDecor.exe"])));
        assert!(!wants_window(&args(&["--tray"])));
        assert!(!wants_window(&args(&["x", "--tray"])));
    }

    #[test]
    fn portable_does_not_autostart() {
        // The USB drive may not be plugged in at boot → the portable build does not write the Run entry itself.
        assert!(!should_enable_autostart_first_run(false, true, false));
        assert!(should_enable_autostart_first_run(false, false, false));
        // Already initialized (even if the user turned it off) → leave it alone.
        assert!(!should_enable_autostart_first_run(true, false, false));
        // Debug builds never write it.
        assert!(!should_enable_autostart_first_run(false, false, true));
    }

    #[test]
    fn normal_launch_is_not_an_installer_command() {
        // Do not run the real commands here (they touch Windows); only check that a normal launch is not swallowed.
        assert_eq!(run_installer_command(&args(&["exe"])), None);
        assert_eq!(run_installer_command(&args(&["exe", "--tray"])), None);
        assert_eq!(run_installer_command(&args(&["exe", "--tray", "--quit"])), None);
    }

    #[test]
    fn guard_detection() {
        assert!(is_guard(&args(&["exe", "--guard", "--journal", "j"])));
        assert!(!is_guard(&args(&["exe"])));
        assert!(!is_guard(&args(&["exe", "--tray", "--guard"])));
    }

    #[test]
    fn elevated_helper_detection() {
        assert!(is_elevated_helper(&args(&["exe", "--elevated-helper", "--pipe", "p", "--parent", "1"])));
        assert!(!is_elevated_helper(&args(&["exe"])));
        assert!(!is_elevated_helper(&args(&["exe", "--tray", "--elevated-helper"])));
        assert!(!is_elevated_helper(&args(&["exe", "--guard"])));
        // Bad arguments: still enters helper mode but exits with code 1, without opening the app.
        assert_eq!(run_elevated_helper(&args(&["exe", "--elevated-helper"])), 1);
    }
}
