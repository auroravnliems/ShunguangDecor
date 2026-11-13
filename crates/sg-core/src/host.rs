//! ModuleHost: runs each part on its own thread, wrapped in `catch_unwind`, with a wait deadline.
//! If one part panics, only that part is disabled and marked faulted; the other parts keep running.

use crate::ids::ModuleId;
use crate::module::{Availability, Module, ModuleInfo, ModuleStatus};
use crate::text::I18nText;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Maximum time to wait for a command sent to a part.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub enum HostEvent {
    Changed(ModuleInfo),
    Faulted { id: ModuleId, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    Unavailable(I18nText),
    Module(I18nText),
    Timeout,
    Stopped,
}

impl HostError {
    pub fn to_text(&self) -> I18nText {
        match self {
            HostError::Unavailable(t) | HostError::Module(t) => t.clone(),
            HostError::Timeout => I18nText::new("err.module.timeout"),
            HostError::Stopped => I18nText::new("err.module.stopped"),
        }
    }
}

type Job = Box<dyn FnOnce(&mut dyn Module) + Send>;
pub type OnEvent = Arc<dyn Fn(HostEvent) + Send + Sync>;

enum Op {
    Enable(toml::Table),
    Disable,
    Apply(toml::Table),
    Restore { was_enabled: bool },
    /// Only recompute the displayed status, without changing enabled/disabled.
    Refresh,
}

/// Result of a job run on a part's thread.
struct Outcome {
    result: Result<(), HostError>,
    /// Enabled/disabled state after this job; `None` = unchanged.
    enabled_after: Option<bool>,
    availability: Availability,
    status: ModuleStatus,
}

struct Slot {
    id: ModuleId,
    tx: Sender<Job>,
    info: Arc<Mutex<ModuleInfo>>,
}

pub struct ModuleHost {
    slots: Vec<Slot>,
    on_event: OnEvent,
    timeout: Duration,
}

impl ModuleHost {
    pub fn new(modules: Vec<Box<dyn Module>>, on_event: OnEvent) -> ModuleHost {
        ModuleHost::with_timeout(modules, on_event, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(modules: Vec<Box<dyn Module>>, on_event: OnEvent, timeout: Duration) -> ModuleHost {
        let slots = modules
            .into_iter()
            .map(|mut m| {
                let id = m.id();
                let info = ModuleInfo {
                    id,
                    number: id.number(),
                    availability: m.availability(),
                    enabled: false,
                    fault: None,
                    status: m.status(),
                };
                let (tx, rx) = mpsc::channel::<Job>();
                let spawned = std::thread::Builder::new().name(format!("sg-mod-{id}")).spawn(move || {
                    for job in rx {
                        job(m.as_mut());
                    }
                });
                if let Err(e) = spawned {
                    tracing::error!(target: "sg::host", "cannot start thread for {id}: {e}");
                }
                Slot { id, tx, info: Arc::new(Mutex::new(info)) }
            })
            .collect();
        ModuleHost { slots, on_event, timeout }
    }

    fn slot(&self, id: ModuleId) -> Option<&Slot> {
        self.slots.iter().find(|s| s.id == id)
    }

    /// Cached copy; does not wait for a busy thread.
    pub fn infos(&self) -> Vec<ModuleInfo> {
        self.slots.iter().map(|s| lock(&s.info).clone()).collect()
    }

    pub fn info(&self, id: ModuleId) -> ModuleInfo {
        match self.slot(id) {
            Some(s) => lock(&s.info).clone(),
            None => ModuleInfo {
                id,
                number: id.number(),
                availability: Availability::Unsupported { reason: I18nText::new("reason.coming_soon") },
                enabled: false,
                fault: None,
                status: ModuleStatus::default(),
            },
        }
    }

    pub fn set_enabled(&self, id: ModuleId, on: bool, cfg: &toml::Table) -> Result<ModuleInfo, HostError> {
        let op = if on { Op::Enable(cfg.clone()) } else { Op::Disable };
        self.run(id, op)
    }

    pub fn apply(&self, id: ModuleId, cfg: &toml::Table) -> Result<(), HostError> {
        self.run(id, Op::Apply(cfg.clone())).map(|_| ())
    }

    /// Recompute `status()` on the part's thread, then update the cached copy and emit `Changed`. Use when the state
    /// changes outside the Enable/Disable/Apply cycle (e.g. admin rights, the number of changed configs in the Hidden Features part).
    pub fn refresh(&self, id: ModuleId) -> Result<ModuleInfo, HostError> {
        self.run(id, Op::Refresh)
    }

    /// "Restore Windows defaults" for everything: disable parts that are enabled, then restore.
    pub fn restore_all(&self) -> Vec<(ModuleId, Result<(), HostError>)> {
        self.slots
            .iter()
            .map(|s| {
                let was_enabled = lock(&s.info).enabled;
                (s.id, self.run(s.id, Op::Restore { was_enabled }).map(|_| ()))
            })
            .collect()
    }

    /// App exit: calls `on_app_exit` on every enabled part concurrently, waiting at most `per_module` in total.
    /// Parts that exceed the deadline are skipped so that quitting the app never hangs.
    /// Returns the parts that did not finish cleaning up (error, panic, timeout) so the caller can keep the journal for the watchdog process.
    #[must_use]
    pub fn shutdown(&self, per_module: Duration) -> Vec<ModuleId> {
        let deadline = Instant::now() + per_module;
        let mut waits = Vec::new();
        for s in &self.slots {
            if !lock(&s.info).enabled {
                continue;
            }
            let (rtx, rrx) = mpsc::sync_channel::<Result<(), String>>(1);
            let job: Job = Box::new(move |m| {
                let r = match catch_unwind(AssertUnwindSafe(|| m.on_app_exit())) {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(t)) => Err(t.to_string()),
                    Err(p) => Err(panic_message(p)),
                };
                let _ = rtx.send(r);
            });
            if s.tx.send(job).is_ok() {
                waits.push((s.id, rrx));
            }
        }
        let mut failed = Vec::new();
        for (id, rrx) in waits {
            let left = deadline.saturating_duration_since(Instant::now());
            match rrx.recv_timeout(left) {
                Ok(Ok(())) => continue,
                Ok(Err(e)) => tracing::warn!(target: "sg::host", "{id}: on_app_exit failed: {e}"),
                Err(_) => tracing::warn!(target: "sg::host", "{id}: on_app_exit timed out, skipped"),
            }
            failed.push(id);
        }
        failed
    }

    fn run(&self, id: ModuleId, op: Op) -> Result<ModuleInfo, HostError> {
        let slot = self.slot(id).ok_or(HostError::Stopped)?;
        let info = slot.info.clone();
        let on_event = self.on_event.clone();
        let (rtx, rrx) = mpsc::sync_channel::<Result<ModuleInfo, HostError>>(1);
        let job: Job = Box::new(move |m| {
            let r = match catch_unwind(AssertUnwindSafe(|| exec(m, op))) {
                Ok(o) => Ok(o),
                Err(p) => {
                    // The part just panicked: try to disable it (also wrapped in catch_unwind) so nothing is left half-done.
                    let _ = catch_unwind(AssertUnwindSafe(|| m.disable()));
                    Err(panic_message(p))
                }
            };
            // Record the state right on the part's thread: even if the caller has timed out, a late result
            // is still saved and reported to the UI/tray, so `shutdown` doesn't miss a part that actually got enabled.
            let _ = rtx.send(record(id, &info, &*on_event, r));
        });
        slot.tx.send(job).map_err(|_| HostError::Stopped)?;
        match rrx.recv_timeout(self.timeout) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!(target: "sg::host", "{id}: call timed out after {:?}; its result will still be applied", self.timeout);
                Err(HostError::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(HostError::Stopped),
        }
    }
}

/// Apply a call's result to the part's state and emit events. Runs on the part's thread.
fn record(
    id: ModuleId,
    info: &Mutex<ModuleInfo>,
    on_event: &(dyn Fn(HostEvent) + Send + Sync),
    received: Result<Outcome, String>,
) -> Result<ModuleInfo, HostError> {
    match received {
        Ok(o) => {
            let snapshot = {
                let mut info = lock(info);
                info.availability = o.availability;
                info.status = o.status;
                if let Some(on) = o.enabled_after {
                    info.enabled = on;
                    if on {
                        info.fault = None;
                    }
                }
                info.clone()
            };
            on_event(HostEvent::Changed(snapshot.clone()));
            o.result.map(|_| snapshot)
        }
        Err(message) => {
            tracing::error!(target: "sg::host", "{id}: module panicked: {message}");
            let snapshot = {
                let mut info = lock(info);
                info.enabled = false;
                info.fault = Some(message.clone());
                info.clone()
            };
            on_event(HostEvent::Faulted { id, message: message.clone() });
            on_event(HostEvent::Changed(snapshot));
            Err(HostError::Module(I18nText::new("err.module.crashed").arg("message", message)))
        }
    }
}

/// Runs on the part's thread.
fn exec(m: &mut dyn Module, op: Op) -> Outcome {
    let availability = m.availability();
    let (result, enabled_after) = match op {
        Op::Enable(cfg) => match &availability {
            Availability::Unsupported { reason } => (Err(HostError::Unavailable(reason.clone())), Some(false)),
            _ => match m.enable(&cfg) {
                Ok(()) => (Ok(()), Some(true)),
                Err(t) => (Err(HostError::Module(t)), Some(false)),
            },
        },
        // If disabling fails, keep the state so the user can retry.
        Op::Disable => match m.disable() {
            Ok(()) => (Ok(()), Some(false)),
            Err(t) => (Err(HostError::Module(t)), None),
        },
        Op::Apply(cfg) => (m.apply(&cfg).map_err(HostError::Module), None),
        Op::Refresh => (Ok(()), None),
        Op::Restore { was_enabled } => {
            let disabled = if was_enabled { m.disable().map_err(HostError::Module) } else { Ok(()) };
            match disabled {
                Ok(()) => (m.restore_os_default().map_err(HostError::Module), Some(false)),
                Err(e) => (Err(e), None),
            }
        }
    };
    Outcome { result, enabled_after, availability, status: m.status() }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // The mutex is "poisoned" because a thread panicked while holding it: keep using the inner data anyway.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{Availability, Module, ModuleStatus};
    use crate::text::I18nText;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Counters {
        enable: AtomicUsize,
        disable: AtomicUsize,
        restore: AtomicUsize,
        exit: AtomicUsize,
        panic_on_enable: AtomicBool,
        sleep_ms: AtomicUsize,
        /// Number shown in `status()` (to test refresh).
        status_n: AtomicUsize,
    }

    struct Fake {
        id: ModuleId,
        c: Arc<Counters>,
    }

    impl Fake {
        fn boxed(id: ModuleId) -> (Box<dyn Module>, Arc<Counters>) {
            let c = Arc::new(Counters::default());
            (Box::new(Fake { id, c: c.clone() }), c)
        }
        fn nap(&self) {
            let ms = self.c.sleep_ms.load(Ordering::SeqCst) as u64;
            if ms > 0 {
                std::thread::sleep(Duration::from_millis(ms));
            }
        }
    }

    impl Module for Fake {
        fn id(&self) -> ModuleId {
            self.id
        }
        fn availability(&self) -> Availability {
            Availability::Available
        }
        fn enable(&mut self, _cfg: &toml::Table) -> Result<(), I18nText> {
            self.nap();
            if self.c.panic_on_enable.load(Ordering::SeqCst) {
                panic!("fake panic");
            }
            self.c.enable.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn apply(&mut self, _cfg: &toml::Table) -> Result<(), I18nText> {
            Ok(())
        }
        fn disable(&mut self) -> Result<(), I18nText> {
            self.c.disable.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn restore_os_default(&mut self) -> Result<(), I18nText> {
            self.c.restore.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn on_app_exit(&mut self) -> Result<(), I18nText> {
            self.nap();
            self.c.exit.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn status(&self) -> ModuleStatus {
            let n = self.c.status_n.load(Ordering::SeqCst);
            ModuleStatus { summary: None, details: vec![(I18nText::new("k"), I18nText::new("v").arg("n", n))] }
        }
    }

    type Events = Arc<Mutex<Vec<HostEvent>>>;

    fn sink() -> (Events, Arc<dyn Fn(HostEvent) + Send + Sync>) {
        let ev: Events = Arc::new(Mutex::new(Vec::new()));
        let e2 = ev.clone();
        (ev, Arc::new(move |e| e2.lock().unwrap().push(e)))
    }

    fn empty() -> toml::Table {
        toml::Table::new()
    }

    #[test]
    fn refresh_recomputes_status_without_changing_enabled() {
        let (ev, on) = sink();
        let (m, c) = Fake::boxed(ModuleId::Cursor);
        let host = ModuleHost::new(vec![m], on);
        assert_eq!(host.info(ModuleId::Cursor).status.details[0].1.args["n"], "0");
        c.status_n.store(7, Ordering::SeqCst);
        // Before refresh it is still the old cached copy.
        assert_eq!(host.info(ModuleId::Cursor).status.details[0].1.args["n"], "0");
        let info = host.refresh(ModuleId::Cursor).unwrap();
        assert_eq!(info.status.details[0].1.args["n"], "7");
        assert_eq!(host.info(ModuleId::Cursor).status.details[0].1.args["n"], "7");
        assert!(!host.info(ModuleId::Cursor).enabled);
        assert_eq!(c.enable.load(Ordering::SeqCst) + c.disable.load(Ordering::SeqCst), 0);
        assert_eq!(ev.lock().unwrap().iter().filter(|e| matches!(e, HostEvent::Changed(_))).count(), 1);
    }

    #[test]
    fn enable_unavailable_is_rejected() {
        let (_ev, on) = sink();
        let host = ModuleHost::new(vec![Box::new(crate::placeholder::Placeholder::new(ModuleId::Cursor))], on);
        let r = host.set_enabled(ModuleId::Cursor, true, &empty());
        assert!(matches!(r, Err(HostError::Unavailable(_))), "{r:?}");
        assert!(!host.info(ModuleId::Cursor).enabled);
    }

    #[test]
    fn enable_disable_emits_changed() {
        let (ev, on) = sink();
        let (m, c) = Fake::boxed(ModuleId::Cursor);
        let host = ModuleHost::new(vec![m], on);
        let info = host.set_enabled(ModuleId::Cursor, true, &empty()).unwrap();
        assert!(info.enabled);
        assert_eq!(c.enable.load(Ordering::SeqCst), 1);
        assert_eq!(
            ev.lock().unwrap().iter().filter(|e| matches!(e, HostEvent::Changed(_))).count(),
            1
        );
        let info = host.set_enabled(ModuleId::Cursor, false, &empty()).unwrap();
        assert!(!info.enabled);
        assert_eq!(c.disable.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn panic_marks_faulted_and_isolates() {
        let (ev, on) = sink();
        let (a, ca) = Fake::boxed(ModuleId::Cursor);
        let (b, _cb) = Fake::boxed(ModuleId::Labels);
        let host = ModuleHost::new(vec![a, b], on);
        ca.panic_on_enable.store(true, Ordering::SeqCst);
        let r = host.set_enabled(ModuleId::Cursor, true, &empty());
        assert!(matches!(r, Err(HostError::Module(_))), "{r:?}");
        let ia = host.info(ModuleId::Cursor);
        assert!(ia.fault.is_some());
        assert!(!ia.enabled);
        assert!(ev.lock().unwrap().iter().any(|e| matches!(e, HostEvent::Faulted { id: ModuleId::Cursor, .. })));
        // Other parts are unaffected.
        assert!(host.set_enabled(ModuleId::Labels, true, &empty()).unwrap().enabled);
        // Re-enabling the faulted part once it stops failing works, and clears the fault mark.
        ca.panic_on_enable.store(false, Ordering::SeqCst);
        let ia = host.set_enabled(ModuleId::Cursor, true, &empty()).unwrap();
        assert!(ia.enabled);
        assert_eq!(ia.fault, None);
    }

    #[test]
    fn slow_call_times_out() {
        let (_ev, on) = sink();
        let (m, c) = Fake::boxed(ModuleId::Cursor);
        c.sleep_ms.store(1500, Ordering::SeqCst);
        let host = ModuleHost::with_timeout(vec![m], on, Duration::from_millis(200));
        let t = std::time::Instant::now();
        let r = host.set_enabled(ModuleId::Cursor, true, &empty());
        assert!(matches!(r, Err(HostError::Timeout)), "{r:?}");
        assert!(t.elapsed() < Duration::from_millis(1200));
    }

    #[test]
    fn restore_all_disables_everything() {
        let (_ev, on) = sink();
        let (a, ca) = Fake::boxed(ModuleId::Cursor);
        let (b, cb) = Fake::boxed(ModuleId::Labels);
        let host = ModuleHost::new(vec![a, b], on);
        host.set_enabled(ModuleId::Cursor, true, &empty()).unwrap();
        host.set_enabled(ModuleId::Labels, true, &empty()).unwrap();
        let res = host.restore_all();
        assert_eq!(res.len(), 2);
        assert!(res.iter().all(|(_, r)| r.is_ok()));
        assert!(!host.info(ModuleId::Cursor).enabled);
        assert!(!host.info(ModuleId::Labels).enabled);
        assert_eq!(ca.restore.load(Ordering::SeqCst), 1);
        assert_eq!(cb.restore.load(Ordering::SeqCst), 1);
        assert_eq!(ca.disable.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn shutdown_is_bounded() {
        let (_ev, on) = sink();
        let (m, c) = Fake::boxed(ModuleId::Cursor);
        let host = ModuleHost::new(vec![m], on);
        host.set_enabled(ModuleId::Cursor, true, &empty()).unwrap();
        c.sleep_ms.store(5000, Ordering::SeqCst);
        let t = std::time::Instant::now();
        assert_eq!(host.shutdown(Duration::from_millis(300)), vec![ModuleId::Cursor]);
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn shutdown_reports_nothing_when_all_clean() {
        let (_ev, on) = sink();
        let (m, c) = Fake::boxed(ModuleId::Cursor);
        let host = ModuleHost::new(vec![m], on);
        host.set_enabled(ModuleId::Cursor, true, &empty()).unwrap();
        assert!(host.shutdown(Duration::from_millis(500)).is_empty());
        assert_eq!(c.exit.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn late_result_after_timeout_is_still_recorded() {
        let (ev, on) = sink();
        let (m, c) = Fake::boxed(ModuleId::Cursor);
        c.sleep_ms.store(400, Ordering::SeqCst);
        let host = ModuleHost::with_timeout(vec![m], on, Duration::from_millis(100));
        let r = host.set_enabled(ModuleId::Cursor, true, &empty());
        assert!(matches!(r, Err(HostError::Timeout)), "{r:?}");
        assert!(!host.info(ModuleId::Cursor).enabled);
        std::thread::sleep(Duration::from_millis(800));
        // The part actually finished enabling: state and events must reflect that.
        assert!(host.info(ModuleId::Cursor).enabled);
        assert!(ev.lock().unwrap().iter().any(|e| matches!(e, HostEvent::Changed(i) if i.id == ModuleId::Cursor && i.enabled)));
    }

    #[test]
    fn shutdown_skips_disabled_modules() {
        let (_ev, on) = sink();
        let (m, c) = Fake::boxed(ModuleId::Cursor);
        let host = ModuleHost::new(vec![m], on);
        assert!(host.shutdown(Duration::from_millis(300)).is_empty());
        assert_eq!(c.exit.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn infos_lists_in_order() {
        let (_ev, on) = sink();
        let host = ModuleHost::new(
            ModuleId::ALL.iter().map(|&id| Box::new(crate::placeholder::Placeholder::new(id)) as Box<dyn Module>).collect(),
            on,
        );
        let ids: Vec<ModuleId> = host.infos().iter().map(|i| i.id).collect();
        assert_eq!(ids, ModuleId::ALL.to_vec());
        assert_eq!(host.info(ModuleId::Wallpaper).number, 2);
    }
}
