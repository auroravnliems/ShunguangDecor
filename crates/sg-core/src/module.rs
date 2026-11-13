//! The common model of a "module": every module implements the same trait so the on/off switch,
//! restore and running state behave the same everywhere.

use crate::guard::{GuardJournal, RestoreAction};
use crate::ids::ModuleId;
use crate::paths::Paths;
use crate::text::I18nText;
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::{Arc, Mutex};

type EmitFn = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

/// What a module needs from the app: data paths, the restore journal of the guard process, and
/// a way to emit events to the UI (if the window is closed the event is dropped, no error).
#[derive(Clone)]
pub struct ModuleCtx {
    pub paths: Paths,
    journal: Arc<Mutex<GuardJournal>>,
    emit: EmitFn,
}

impl ModuleCtx {
    pub fn new(paths: Paths, journal: Arc<Mutex<GuardJournal>>, emit: EmitFn) -> ModuleCtx {
        ModuleCtx { paths, journal, emit }
    }

    /// For tests: the journal lives under `paths`, events are dropped.
    pub fn for_test(paths: Paths) -> ModuleCtx {
        let journal = GuardJournal::open(&paths.guard_journal_file());
        ModuleCtx { paths, journal: Arc::new(Mutex::new(journal)), emit: Arc::new(|_, _| {}) }
    }

    /// Sends `event` to the UI window (if it is open).
    pub fn emit(&self, event: &str, payload: impl Serialize) {
        match serde_json::to_value(payload) {
            Ok(v) => (self.emit)(event, v),
            Err(e) => tracing::warn!(target: "sg::core", "cannot serialize event {event}: {e}"),
        }
    }

    /// Records the restore action in the journal **before** changing Windows.
    pub fn journal_add(&self, a: RestoreAction) -> io::Result<()> {
        self.journal.lock().unwrap_or_else(|e| e.into_inner()).add(a)
    }

    /// Whether the journal currently holds action `a` (e.g. a previous restore failed and left it behind).
    pub fn journal_contains(&self, a: RestoreAction) -> bool {
        self.journal.lock().unwrap_or_else(|e| e.into_inner()).contains(a)
    }

    /// Removes the restore action once Windows has been put back as it was.
    pub fn journal_remove(&self, a: RestoreAction) -> io::Result<()> {
        self.journal.lock().unwrap_or_else(|e| e.into_inner()).remove(a)
    }
}

/// How far this module can work on this machine. The UI shows exactly this, nothing is hidden silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Availability {
    Available,
    Partial { reason: I18nText },
    Unsupported { reason: I18nText },
}

/// Summary for the Home page card: one subtitle line + at most 4 "label: value" pairs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModuleStatus {
    pub summary: Option<I18nText>,
    pub details: Vec<(I18nText, I18nText)>,
}

/// Every module implements this trait. Returned errors are translatable text for the UI to show the user.
pub trait Module: Send {
    fn id(&self) -> ModuleId;
    fn availability(&self) -> Availability;
    fn enable(&mut self, cfg: &toml::Table) -> Result<(), I18nText>;
    fn apply(&mut self, cfg: &toml::Table) -> Result<(), I18nText>;
    fn disable(&mut self) -> Result<(), I18nText>;
    /// Resets to Windows defaults everything this module has changed.
    fn restore_os_default(&mut self) -> Result<(), I18nText>;
    /// Called when the app exits. By default the module is disabled; Cursor and Hidden features override it to
    /// "leave as is", because those are persistent Windows settings.
    fn on_app_exit(&mut self) -> Result<(), I18nText> {
        self.disable()
    }
    fn status(&self) -> ModuleStatus;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleInfo {
    pub id: ModuleId,
    pub number: u8,
    pub availability: Availability,
    pub enabled: bool,
    /// Set when the module just hit a severe error (panic) and was disabled.
    pub fault: Option<String>,
    pub status: ModuleStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_serde_tagged() {
        let a = Availability::Unsupported { reason: I18nText::new("reason.coming_soon") };
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"kind":"unsupported","reason":{"key":"reason.coming_soon","args":{}}}"#
        );
        assert_eq!(serde_json::to_string(&Availability::Available).unwrap(), r#"{"kind":"available"}"#);
    }

    #[test]
    fn module_info_camel_case() {
        let i = ModuleInfo {
            id: ModuleId::Cursor,
            number: 4,
            availability: Availability::Available,
            enabled: true,
            fault: None,
            status: ModuleStatus::default(),
        };
        let v: serde_json::Value = serde_json::to_value(&i).unwrap();
        assert_eq!(v["id"], "cursor");
        assert_eq!(v["number"], 4);
        assert_eq!(v["status"]["details"], serde_json::json!([]));
        assert!(v["status"]["summary"].is_null());
    }
}
