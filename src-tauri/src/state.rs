//! The app's shared state (Tauri `manage`).

use crate::tray::TrayHandles;
use sg_core::config::{self, AppConfig};
use sg_core::guard::GuardJournal;
use sg_core::host::ModuleHost;
use sg_core::i18n::Lang;
use sg_core::ids::ModuleId;
use sg_core::paths::Paths;
use sg_core::profiles::ProfileStore;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};

/// The computed wallpaper thumbnail, kept until the file changes.
pub struct ThumbCache {
    pub path: PathBuf,
    pub modified_unix: u64,
    pub jpeg: Arc<Vec<u8>>,
}

pub struct AppState {
    pub paths: Paths,
    pub config: Mutex<AppConfig>,
    /// `app.toml` exists but could not be read/backed up at startup: this session does not overwrite it.
    pub config_frozen: bool,
    pub host: ModuleHost,
    pub profiles: ProfileStore,
    pub journal: Arc<Mutex<GuardJournal>>,
    /// Per-part services (used by that part's Tauri commands).
    pub services: crate::modules::Services,
    /// Work the window must do as soon as it opens (for example "confirm-restore" from the tray menu).
    pub pending_intent: Mutex<Option<String>>,
    pub tray: Mutex<Option<TrayHandles>>,
    pub thumb: Mutex<Option<ThumbCache>>,
    /// Quitting for good (from the tray menu): closing the window really closes it.
    pub quitting: AtomicBool,
    /// Cleanup on exit has been done (only runs once).
    pub shut_down: AtomicBool,
    /// "Still running in the tray" was already asked this session: the next close click closes for real,
    /// even if the UI does not answer (so the window can never get stuck unable to close).
    pub tray_notice_asked: AtomicBool,
    /// The guard process. Holds the `Child` (and its stdin pipe) for the app's whole lifetime.
    pub guard_child: Mutex<Option<Child>>,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    /// A copy of the tray menu items. Never hold the `tray` lock while calling the menu: off the main thread,
    /// Tauri forwards menu calls to the main thread and waits, so holding the lock then can hang the whole app.
    pub fn tray(&self) -> Option<TrayHandles> {
        lock(&self.tray).clone()
    }

    pub fn lang(&self) -> Lang {
        lock(&self.config).language.unwrap_or(Lang::En)
    }

    pub fn config(&self) -> AppConfig {
        lock(&self.config).clone()
    }

    /// Edits the config and immediately saves `app.toml`. A save failure is only logged and does not fail the operation.
    pub fn update_config<R>(&self, f: impl FnOnce(&mut AppConfig) -> R) -> R {
        let mut c = lock(&self.config);
        let r = f(&mut c);
        if self.config_frozen {
            tracing::warn!(target: "sg::app", "app.toml was unreadable at startup; change kept in memory only");
        } else if let Err(e) = config::save(&self.paths.app_config_file(), &c) {
            tracing::warn!(target: "sg::app", "cannot save app.toml: {e}");
        }
        r
    }

    /// The configuration currently in use for a part (`config/<part>.toml`); missing or broken gives an empty table.
    pub fn module_config_text(&self, id: ModuleId) -> String {
        std::fs::read_to_string(self.paths.module_config_file(id))
            .map(|t| config::strip_bom(&t).to_string())
            .unwrap_or_default()
    }

    pub fn module_config(&self, id: ModuleId) -> toml::Table {
        let text = self.module_config_text(id);
        match text.parse::<toml::Table>() {
            Ok(t) => t,
            Err(e) => {
                if !text.trim().is_empty() {
                    tracing::warn!(target: "sg::app", "{id}.toml is not valid TOML, using defaults: {e}");
                }
                toml::Table::new()
            }
        }
    }
}
