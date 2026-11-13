//! `TaskbarModule` (plugged into the ModuleHost) + `TaskbarController` (shared with the Tauri commands).
//!
//! - Turn on: `TaskbarConfig::from_table` → `generate` (or the user's file when `raw::state == EditedByUser`) →
//!   `raw::sync` → run the engine → apply the native taskbar mode.
//! - Turn off / app exit / restore defaults: restore the native taskbar FIRST (`NativeTaskbar::restore`), only then stop the engine.
//! - Engine reports a severe error or exits by itself: restore the native taskbar immediately, then turn the module off as if the user flipped the switch.
//! - The engine and the native taskbar sit behind small traits so the call order can be tested without Win32.

use crate::generate::{Generated, generate};
use crate::model::{Corner, Edge, NativeMode, TaskbarConfig, WidgetKind};
use crate::native::{NativePort, NativeTaskbar};
use crate::raw::{self, RawState};
use crate::reveal::auto_hide_conflicts;
use serde::Serialize;
use sg_core::i18n::Lang;
use sg_core::ids::ModuleId;
use sg_core::module::{Availability, Module, ModuleCtx, ModuleStatus};
use sg_core::text::I18nText;
use std::borrow::Cow;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// Event from the bar (merged from `TaskbarEvent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarEvent {
    OpenApp,
    TurnOff,
    /// The engine rejected the new configuration (the old bar keeps running).
    Rejected(String),
    /// The bar crashed (panic): it no longer runs; the module turns itself off as if the user flipped the switch.
    Fatal(String),
    /// The bar exited by itself because the Windows session is ending (sign-out, shutdown, restart) or it was closed externally: restore
    /// the native taskbar but do NOT change the user's saved choice (`app.toml`).
    Stopped,
}

pub type OnBarEvent = Arc<dyn Fn(BarEvent) + Send + Sync>;

/// A running bar (real: `TaskbarHandle`; test: fake).
pub trait BarEngine: Send {
    fn reload(&self, g: Generated);
    /// Explorer just restarted: reload AND re-register each bar's AppBar.
    fn reload_after_explorer_restart(&self, g: Generated);
    /// Repositions the AppBar of every bar (call AFTER the native taskbar changes mode).
    fn replace_appbars(&self);
    fn shutdown(&self);
}

/// Creates bars + validates files (real: engine; test: fake).
pub trait BarFactory: Send + Sync {
    fn spawn(&self, g: Generated, lang: Lang, events: OnBarEvent) -> Result<Box<dyn BarEngine>, String>;
    fn validate(&self, g: &Generated) -> Result<(), String>;
}

/// Controls the native taskbar (real: `NativeTaskbar`; test: fake).
pub trait NativeControl: Send {
    fn apply(&mut self, mode: NativeMode, hot_corner: Option<Corner>, ctx: &ModuleCtx) -> io::Result<()>;
    fn reapply(&mut self) -> io::Result<()>;
    fn restore(&mut self) -> io::Result<()>;
    /// The edge the horizontal native taskbars are docked to (to drop "auto-hide" when competing for the edge with `reveal` mode).
    fn native_edges(&self) -> Vec<Edge> {
        Vec::new()
    }
}

impl<P: NativePort> NativeControl for NativeTaskbar<P> {
    fn apply(&mut self, mode: NativeMode, hot_corner: Option<Corner>, ctx: &ModuleCtx) -> io::Result<()> {
        NativeTaskbar::apply(self, mode, hot_corner, ctx)
    }
    fn reapply(&mut self) -> io::Result<()> {
        NativeTaskbar::reapply(self)
    }
    fn restore(&mut self) -> io::Result<()> {
        NativeTaskbar::restore(self)
    }
    fn native_edges(&self) -> Vec<Edge> {
        NativeTaskbar::native_edges(self)
    }
}

/// What the app does when the bar asks (set after construction: needs the `AppHandle`).
#[derive(Clone)]
pub struct Hooks {
    /// Turns the module off as if the user flipped the switch (updates `app.toml` + the UI). Must not block.
    pub turn_off: Arc<dyn Fn() + Send + Sync>,
    /// Opens/brings the main window to the front. Must not block.
    pub open_app: Arc<dyn Fn() + Send + Sync>,
    /// Called every time it is turned on: ensures the Explorer-restart watcher is running (does not load the DLL). Safe to call repeatedly.
    pub watch_explorer: Arc<dyn Fn() + Send + Sync>,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks { turn_off: Arc::new(|| {}), open_app: Arc::new(|| {}), watch_explorer: Arc::new(|| {}) }
    }
}

/// A widget in the catalog for the Taskbar page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetCatalogItem {
    pub kind: &'static str,
    pub name_key: String,
    pub desc_key: String,
    /// Each kind can be placed only once on the bar (the generator drops duplicates).
    pub singleton: bool,
}

pub fn widget_catalog() -> Vec<WidgetCatalogItem> {
    WidgetKind::ALL
        .into_iter()
        .map(|k| WidgetCatalogItem {
            kind: k.key(),
            name_key: format!("taskbar.widget.{}.name", k.key()),
            desc_key: format!("taskbar.widget.{}.desc", k.key()),
            singleton: true,
        })
        .collect()
}

/// State sent to the UI (`taskbar-status`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskbarStatus {
    pub enabled: bool,
    pub native_hidden: bool,
    pub raw_state: RawState,
    pub error: Option<I18nText>,
}

/// The configuration actually handed to the generator: "auto-hide" is dropped (with a warning) when the native taskbar is in `reveal` mode and on the same edge as
/// the bar, since both compete for that edge.
fn effective_config<'a>(cfg: &'a TaskbarConfig, native: &dyn NativeControl) -> Cow<'a, TaskbarConfig> {
    if cfg.auto_hide && auto_hide_conflicts(cfg.native, &native.native_edges(), cfg.edge) {
        tracing::warn!(
            target: "sg::taskbar",
            "auto-hide ignored: the native taskbar is in reveal mode on the same edge as the bar"
        );
        let mut c = cfg.clone();
        c.auto_hide = false;
        return Cow::Owned(c);
    }
    Cow::Borrowed(cfg)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Run {
    enabled: bool,
    cfg: Option<TaskbarConfig>,
    engine: Option<Box<dyn BarEngine>>,
    native: Box<dyn NativeControl>,
    error: Option<I18nText>,
    /// Incremented each time a new bar is built: events from the old bar are ignored.
    epoch: u64,
}

struct Inner {
    ctx: ModuleCtx,
    lang: Arc<dyn Fn() -> Lang + Send + Sync>,
    factory: Arc<dyn BarFactory>,
    hooks: Mutex<Hooks>,
    run: Mutex<Run>,
    native_hidden: Arc<AtomicBool>,
    me: Weak<Inner>,
}

/// Shared handle (cheap to clone) for Tauri commands and the Explorer-restart listener.
#[derive(Clone)]
pub struct TaskbarController {
    inner: Arc<Inner>,
}

pub struct TaskbarModule {
    ctl: TaskbarController,
}

impl TaskbarModule {
    /// The real thing: the engine + Windows' native taskbar.
    pub fn new(ctx: ModuleCtx, lang: Arc<dyn Fn() -> Lang + Send + Sync>) -> TaskbarModule {
        let (factory, native) = (Arc::new(real::RealFactory) as Arc<dyn BarFactory>, Box::new(NativeTaskbar::system()) as Box<dyn NativeControl>);
        TaskbarModule::with_parts(ctx, lang, factory, native)
    }

    pub fn with_parts(
        ctx: ModuleCtx,
        lang: Arc<dyn Fn() -> Lang + Send + Sync>,
        factory: Arc<dyn BarFactory>,
        native: Box<dyn NativeControl>,
    ) -> TaskbarModule {
        let inner = Arc::new_cyclic(|me| Inner {
            ctx,
            lang,
            factory,
            hooks: Mutex::new(Hooks::default()),
            run: Mutex::new(Run { enabled: false, cfg: None, engine: None, native, error: None, epoch: 0 }),
            native_hidden: Arc::new(AtomicBool::new(false)),
            me: me.clone(),
        });
        TaskbarModule { ctl: TaskbarController { inner } }
    }

    pub fn controller(&self) -> TaskbarController {
        self.ctl.clone()
    }
}

impl TaskbarController {
    pub fn set_hooks(&self, hooks: Hooks) {
        *lock(&self.inner.hooks) = hooks;
    }

    pub fn native_hidden(&self) -> bool {
        self.inner.native_hidden.load(Ordering::SeqCst)
    }

    pub fn raw_dir(&self) -> std::path::PathBuf {
        raw::dir(&self.inner.ctx.paths.roaming)
    }

    pub fn raw_state(&self) -> RawState {
        raw::state(&self.raw_dir())
    }

    pub fn status(&self) -> TaskbarStatus {
        self.inner.status()
    }

    /// "Use the app's controls again" (`keep_user = false`: overwrite the file with the current configuration) or "Keep my file".
    /// If it is on, the bar is reloaded immediately.
    pub fn raw_choose(&self, keep_user: bool) -> Result<(), I18nText> {
        self.inner.raw_choose(keep_user)
    }

    /// Imports a YASB theme from a folder the user picks (R4).
    pub fn import_yasb(&self, dir: &Path) -> Result<(), I18nText> {
        self.inner.import_yasb(dir)
    }

    /// Explorer restarted (`TaskbarCreated`): re-apply the native taskbar mode and re-register the AppBar.
    /// Runs on its own thread: it does not block the watcher's thread.
    pub fn on_explorer_restarted(&self) {
        let me = self.inner.clone();
        let spawned = std::thread::Builder::new().name("sg-taskbar-explorer".into()).spawn(move || me.reapply_after_explorer());
        if let Err(e) = spawned {
            tracing::warn!(target: "sg::taskbar", "cannot start the explorer-restart thread: {e}");
        }
    }
}

impl Inner {
    fn lang(&self) -> Lang {
        (self.lang)()
    }

    fn raw_dir(&self) -> std::path::PathBuf {
        raw::dir(&self.ctx.paths.roaming)
    }

    fn status(&self) -> TaskbarStatus {
        let r = lock(&self.run);
        TaskbarStatus {
            enabled: r.enabled,
            native_hidden: self.native_hidden.load(Ordering::SeqCst),
            raw_state: raw::state(&self.raw_dir()),
            error: r.error.clone(),
        }
    }

    fn emit_status(&self) {
        self.ctx.emit("taskbar-status", self.status());
    }

    /// The two files the engine will read: the app's (already written to disk) or the user's if they edited them.
    fn files_for(&self, cfg: &TaskbarConfig, native: &dyn NativeControl) -> Result<Generated, I18nText> {
        let dir = self.raw_dir();
        let g = generate(&effective_config(cfg, native), self.lang().code());
        let before = raw::state(&dir);
        match raw::sync(&dir, &g) {
            Ok(RawState::Generated) => Ok(g),
            Ok(RawState::EditedByUser) => {
                tracing::info!(target: "sg::taskbar", "taskbar-raw was edited by the user: using their files");
                raw::read(&dir)
            }
            // Writing failed but there is nothing of the user's to preserve: it can still run from the in-memory copy.
            Err(e) if before == RawState::Generated => {
                tracing::warn!(target: "sg::taskbar", "cannot write taskbar-raw ({e}); running from memory");
                Ok(g)
            }
            Err(e) => Err(I18nText::new("taskbar.err.raw_io").arg("detail", e)),
        }
    }

    fn make_events(&self, epoch: u64) -> OnBarEvent {
        let me = self.me.clone();
        Arc::new(move |ev| {
            if let Some(inner) = me.upgrade() {
                inner.on_bar_event(epoch, ev);
            }
        })
    }

    /// Runs on the engine's thread: do not lock `run` here (the module's thread may be holding it while waiting for the engine to exit).
    fn on_bar_event(self: &Arc<Self>, epoch: u64, ev: BarEvent) {
        match ev {
            BarEvent::OpenApp => (lock(&self.hooks).open_app.clone())(),
            BarEvent::TurnOff => (lock(&self.hooks).turn_off.clone())(),
            BarEvent::Rejected(msg) => {
                let me = self.clone();
                self.spawn_worker("sg-taskbar-rejected", move || {
                    let mut r = lock(&me.run);
                    if r.enabled && r.epoch == epoch {
                        r.error = Some(I18nText::new("taskbar.err.config_rejected").arg("detail", msg));
                        drop(r);
                        me.emit_status();
                    }
                });
            }
            BarEvent::Fatal(msg) => {
                let me = self.clone();
                self.spawn_worker("sg-taskbar-fatal", move || me.fail(epoch, msg, true));
            }
            BarEvent::Stopped => {
                let me = self.clone();
                self.spawn_worker("sg-taskbar-stopped", move || me.fail(epoch, "stopped".into(), false));
            }
        }
    }

    fn spawn_worker(&self, name: &str, f: impl FnOnce() + Send + 'static) {
        if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
            tracing::error!(target: "sg::taskbar", "cannot start thread {name}: {e}");
        }
    }

    /// The bar died: restore the native taskbar immediately (never leave the user without any taskbar), record the error. `turn_off`: turn
    /// the module off as if the switch was flipped (severe error); `false` when the Windows session is ending (keep the saved choice).
    fn fail(&self, epoch: u64, msg: String, turn_off: bool) {
        {
            let mut r = lock(&self.run);
            if !r.enabled || r.epoch != epoch {
                return;
            }
            tracing::error!(target: "sg::taskbar", "the bar stopped: {msg}; restoring the native taskbar");
            // The restore error has already been logged in `stop_locked`; the journal is kept, and the watchdog process / next startup finishes the job.
            let _ = self.stop_locked(&mut r);
            r.error = Some(I18nText::new("taskbar.err.stopped").arg("detail", msg));
        }
        self.emit_status();
        if turn_off {
            let hook = lock(&self.hooks).turn_off.clone();
            hook();
        }
    }

    /// Restores the native taskbar FIRST, only then stops the engine (never leaves the user without a taskbar).
    ///
    /// Returns `Err` when restoring the native taskbar fails (the caller MUST report it so the restore journal is kept, not deleted):
    /// the engine is still stopped, but `native.restore()` keeps the unfinished tasks in the journal.
    fn stop_locked(&self, r: &mut Run) -> io::Result<()> {
        let restored = r.native.restore();
        if let Err(e) = &restored {
            tracing::error!(target: "sg::taskbar", "cannot restore the native taskbar: {e}; the restore journal is kept");
        }
        // If the restore failed the native taskbar may still be hidden: do not report "shown".
        self.native_hidden.store(restored.is_err() && self.native_hidden.load(Ordering::SeqCst), Ordering::SeqCst);
        if let Some(engine) = r.engine.take() {
            engine.shutdown();
        }
        r.enabled = false;
        r.cfg = None;
        restored
    }

    /// The saved `config/taskbar.toml` (missing/corrupt → defaults).
    fn saved_config(&self) -> TaskbarConfig {
        let table = std::fs::read_to_string(self.ctx.paths.module_config_file(ModuleId::Taskbar))
            .ok()
            .and_then(|s| s.parse::<toml::Table>().ok())
            .unwrap_or_default();
        Inner::config_of(&table)
    }

    fn config_of(table: &toml::Table) -> TaskbarConfig {
        let (cfg, warns) = TaskbarConfig::from_table(table);
        for w in warns {
            tracing::warn!(target: "sg::taskbar", "taskbar.toml: {w}");
        }
        cfg
    }

    fn enable(&self, table: &toml::Table) -> Result<(), I18nText> {
        let mut r = lock(&self.run);
        if r.enabled {
            drop(r);
            return self.apply(table);
        }
        let cfg = Inner::config_of(table);
        let files = self.files_for(&cfg, &*r.native)?;
        r.error = None;
        (lock(&self.hooks).watch_explorer.clone())();
        r.epoch += 1;
        let events = self.make_events(r.epoch);
        let engine = self
            .factory
            .spawn(files, self.lang(), events)
            .map_err(|e| I18nText::new("taskbar.err.spawn").arg("detail", e))?;
        r.engine = Some(engine);
        r.enabled = true;
        if let Err(e) = r.native.apply(cfg.native, cfg.hot_corner, &self.ctx) {
            tracing::error!(target: "sg::taskbar", "cannot apply the native taskbar mode: {e}");
            let _ = self.stop_locked(&mut r);
            return Err(I18nText::new("taskbar.err.native").arg("detail", e));
        }
        // The native taskbar just changed its reserved area: reposition the AppBar AFTER that (at spawn time it was still auto-hiding, so the bar was placed on top of it).
        if let Some(engine) = &r.engine {
            engine.replace_appbars();
        }
        self.native_hidden.store(cfg.native != NativeMode::Keep, Ordering::SeqCst);
        r.cfg = Some(cfg);
        drop(r);
        self.emit_status();
        Ok(())
    }

    fn apply(&self, table: &toml::Table) -> Result<(), I18nText> {
        let mut r = lock(&self.run);
        if !r.enabled {
            return Ok(());
        }
        let cfg = Inner::config_of(table);
        let files = self.files_for(&cfg, &*r.native)?;
        r.error = None;
        if let Some(engine) = &r.engine {
            engine.reload(files);
        }
        if let Err(e) = r.native.apply(cfg.native, cfg.hot_corner, &self.ctx) {
            tracing::error!(target: "sg::taskbar", "cannot apply the native taskbar mode: {e}");
            let _ = self.stop_locked(&mut r);
            return Err(I18nText::new("taskbar.err.native").arg("detail", e));
        }
        if let Some(engine) = &r.engine {
            engine.replace_appbars();
        }
        self.native_hidden.store(cfg.native != NativeMode::Keep, Ordering::SeqCst);
        r.cfg = Some(cfg);
        drop(r);
        self.emit_status();
        Ok(())
    }

    /// Turns the module off. `Err` when the native taskbar could not be restored: the host still treats the module as on so the user can retry, and
    /// the restore journal is not deleted.
    fn disable(&self) -> Result<(), I18nText> {
        let mut r = lock(&self.run);
        let was_enabled = r.enabled;
        let restored = self.stop_locked(&mut r);
        if was_enabled && restored.is_ok() {
            r.error = None;
        }
        drop(r);
        self.emit_status();
        restored.map_err(|e| I18nText::new("taskbar.err.restore").arg("detail", e))
    }

    fn reapply_after_explorer(&self) {
        let mut r = lock(&self.run);
        if !r.enabled {
            return;
        }
        tracing::info!(target: "sg::taskbar", "explorer restarted: re-applying the native taskbar mode and the bar");
        if let Err(e) = r.native.reapply() {
            tracing::warn!(target: "sg::taskbar", "cannot re-apply the native taskbar mode: {e}");
        }
        // Rebuild the bar (re-register the AppBar) from exactly the files in use.
        if let Some(cfg) = r.cfg.clone() {
            match self.files_for(&cfg, &*r.native) {
                Ok(files) => {
                    if let Some(engine) = &r.engine {
                        engine.reload_after_explorer_restart(files);
                        // `native.reapply` finished earlier: reposition the AppBar according to the native taskbar's new reserved area.
                        engine.replace_appbars();
                    }
                }
                Err(e) => tracing::warn!(target: "sg::taskbar", "cannot reload the bar after explorer restart: {}", e.key),
            }
        }
    }

    fn raw_choose(&self, keep_user: bool) -> Result<(), I18nText> {
        let r = lock(&self.run);
        // While it is off, use the saved configuration (not the defaults) so the written file matches the user's controls.
        let cfg = r.cfg.clone().unwrap_or_else(|| self.saved_config());
        let dir = self.raw_dir();
        let g = generate(&effective_config(&cfg, &*r.native), self.lang().code());
        raw::choose(&dir, keep_user, &g).map_err(|e| I18nText::new("taskbar.err.raw_io").arg("detail", e))?;
        let files = if r.enabled { raw::read(&dir).ok() } else { None };
        if let (Some(files), Some(engine)) = (files, &r.engine) {
            engine.reload(files);
        }
        drop(r);
        self.emit_status();
        Ok(())
    }

    fn import_yasb(&self, src: &Path) -> Result<(), I18nText> {
        let r = lock(&self.run);
        let dir = self.raw_dir();
        let g = raw::import(src, &dir, &|g| self.factory.validate(g))?;
        if let Some(engine) = &r.engine {
            engine.reload(g);
        }
        drop(r);
        self.emit_status();
        Ok(())
    }
}

impl Module for TaskbarModule {
    fn id(&self) -> ModuleId {
        ModuleId::Taskbar
    }

    fn availability(&self) -> Availability {
        Availability::Available
    }

    fn enable(&mut self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.ctl.inner.enable(cfg)
    }

    fn apply(&mut self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.ctl.inner.apply(cfg)
    }

    fn disable(&mut self) -> Result<(), I18nText> {
        self.ctl.inner.disable()
    }

    fn restore_os_default(&mut self) -> Result<(), I18nText> {
        self.ctl.inner.disable()
    }

    fn status(&self) -> ModuleStatus {
        let r = lock(&self.ctl.inner.run);
        let Some(cfg) = r.cfg.as_ref().filter(|_| r.enabled) else {
            return ModuleStatus { summary: r.error.clone(), details: Vec::new() };
        };
        let edge = match cfg.edge {
            crate::model::Edge::Top => "top",
            crate::model::Edge::Bottom => "bottom",
        };
        let native = match cfg.native {
            NativeMode::Keep => "keep",
            NativeMode::Hide => "hide",
            NativeMode::Reveal => "reveal",
        };
        let widgets = cfg.widgets.left.len() + cfg.widgets.center.len() + cfg.widgets.right.len();
        let details = vec![
            (I18nText::new("taskbar.status.position"), I18nText::new(format!("taskbar.edge.{edge}"))),
            (I18nText::new("taskbar.status.height"), I18nText::new("taskbar.val.px").arg("n", cfg.height)),
            (I18nText::new("taskbar.status.widgets"), I18nText::new("taskbar.val.n_widgets").arg("n", widgets)),
            (I18nText::new("taskbar.status.native"), I18nText::new(format!("taskbar.native.{native}"))),
        ];
        ModuleStatus { summary: r.error.clone(), details }
    }
}

mod real {
    use super::{BarEngine, BarEvent, BarFactory, OnBarEvent};
    use crate::engine::{TaskbarEvent, TaskbarHandle};
    use crate::generate::Generated;
    use sg_core::i18n::Lang;

    struct Engine(TaskbarHandle);

    impl BarEngine for Engine {
        fn reload(&self, g: Generated) {
            self.0.reload(g);
        }
        fn reload_after_explorer_restart(&self, g: Generated) {
            self.0.reload_after_explorer_restart(g);
        }
        fn replace_appbars(&self) {
            self.0.replace_appbars();
        }
        fn shutdown(&self) {
            self.0.shutdown();
        }
    }

    pub struct RealFactory;

    impl BarFactory for RealFactory {
        fn spawn(&self, g: Generated, lang: Lang, events: OnBarEvent) -> Result<Box<dyn BarEngine>, String> {
            TaskbarHandle::spawn(g, lang, move |ev| {
                events(match ev {
                    TaskbarEvent::OpenApp => BarEvent::OpenApp,
                    TaskbarEvent::TurnOff => BarEvent::TurnOff,
                    TaskbarEvent::ConfigRejected(m) => BarEvent::Rejected(m),
                    TaskbarEvent::Error(m) => BarEvent::Fatal(m),
                    TaskbarEvent::Stopped => BarEvent::Stopped,
                })
            })
            .map(|h| Box::new(Engine(h)) as Box<dyn BarEngine>)
        }

        fn validate(&self, g: &Generated) -> Result<(), String> {
            crate::engine::validate(g)
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use sg_core::paths::Paths;
    use std::time::{Duration, Instant};

    type Log = Arc<Mutex<Vec<String>>>;

    struct FakeEngine {
        log: Log,
    }

    impl BarEngine for FakeEngine {
        fn reload(&self, g: Generated) {
            lock(&self.log).push(format!("engine.reload:{}", g.yaml.len()));
        }
        fn reload_after_explorer_restart(&self, g: Generated) {
            lock(&self.log).push(format!("engine.reload_forced:{}", g.yaml.len()));
        }
        fn replace_appbars(&self) {
            lock(&self.log).push("engine.replace_appbars".into());
        }
        fn shutdown(&self) {
            lock(&self.log).push("engine.shutdown".into());
        }
    }

    struct FakeFactory {
        log: Log,
        events: Mutex<Option<OnBarEvent>>,
        fail: bool,
        used: Mutex<Vec<Generated>>,
    }

    impl BarFactory for FakeFactory {
        fn spawn(&self, g: Generated, _: Lang, events: OnBarEvent) -> Result<Box<dyn BarEngine>, String> {
            lock(&self.log).push("engine.spawn".into());
            lock(&self.used).push(g);
            if self.fail {
                return Err("boom".into());
            }
            *lock(&self.events) = Some(events);
            Ok(Box::new(FakeEngine { log: self.log.clone() }))
        }
        fn validate(&self, g: &Generated) -> Result<(), String> {
            if g.yaml.contains("BAD") { Err("bad".into()) } else { Ok(()) }
        }
    }

    struct FakeNative {
        log: Log,
        fail_apply: bool,
        /// Enable to make `restore()` simulate a failure (taskbar not found / SHAppBarMessage error).
        fail_restore: Arc<AtomicBool>,
    }

    impl NativeControl for FakeNative {
        fn apply(&mut self, mode: NativeMode, _: Option<Corner>, _: &ModuleCtx) -> io::Result<()> {
            lock(&self.log).push(format!("native.apply:{mode:?}"));
            if self.fail_apply { Err(io::Error::other("no")) } else { Ok(()) }
        }
        fn reapply(&mut self) -> io::Result<()> {
            lock(&self.log).push("native.reapply".into());
            Ok(())
        }
        fn restore(&mut self) -> io::Result<()> {
            lock(&self.log).push("native.restore".into());
            if self.fail_restore.load(Ordering::SeqCst) { Err(io::Error::other("restore failed")) } else { Ok(()) }
        }
    }

    struct Rig {
        module: TaskbarModule,
        log: Log,
        factory: Arc<FakeFactory>,
        turned_off: Arc<Mutex<u32>>,
        opened: Arc<Mutex<u32>>,
        fail_restore: Arc<AtomicBool>,
        _dir: tempfile::TempDir,
    }

    fn rig_with(fail_spawn: bool, fail_native: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { roaming: dir.path().join("r"), local: dir.path().join("l"), portable: true };
        let log: Log = Arc::default();
        let factory = Arc::new(FakeFactory { log: log.clone(), events: Mutex::new(None), fail: fail_spawn, used: Mutex::default() });
        let fail_restore = Arc::new(AtomicBool::new(false));
        let module = TaskbarModule::with_parts(
            ModuleCtx::for_test(paths),
            Arc::new(|| Lang::Vi),
            factory.clone(),
            Box::new(FakeNative { log: log.clone(), fail_apply: fail_native, fail_restore: fail_restore.clone() }),
        );
        let (turned_off, opened) = (Arc::new(Mutex::new(0)), Arc::new(Mutex::new(0)));
        let (t, o) = (turned_off.clone(), opened.clone());
        module.controller().set_hooks(Hooks {
            turn_off: Arc::new(move || *lock(&t) += 1),
            open_app: Arc::new(move || *lock(&o) += 1),
            watch_explorer: Arc::new(|| {}),
        });
        Rig { module, log, factory, turned_off, opened, fail_restore, _dir: dir }
    }

    fn rig() -> Rig {
        rig_with(false, false)
    }

    fn entries(log: &Log) -> Vec<String> {
        lock(log).clone()
    }

    fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    /// A fake native taskbar that only reports the edge it is docked to.
    struct EdgeNative(Vec<Edge>);

    impl NativeControl for EdgeNative {
        fn apply(&mut self, _: NativeMode, _: Option<Corner>, _: &ModuleCtx) -> io::Result<()> {
            Ok(())
        }
        fn reapply(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn restore(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn native_edges(&self) -> Vec<Edge> {
            self.0.clone()
        }
    }

    #[test]
    fn auto_hide_is_dropped_only_when_it_fights_a_revealing_native_taskbar() {
        let base = TaskbarConfig { auto_hide: true, native: NativeMode::Reveal, edge: Edge::Bottom, ..TaskbarConfig::default() };
        // same edge + reveal: drop auto-hide
        assert!(!effective_config(&base, &EdgeNative(vec![Edge::Bottom])).auto_hide);
        assert!(generate(&effective_config(&base, &EdgeNative(vec![Edge::Bottom])), "vi").yaml.contains("auto_hide: false"));
        // different edge, or not reveal: keep as is
        assert!(effective_config(&base, &EdgeNative(vec![Edge::Top])).auto_hide);
        let hide = TaskbarConfig { native: NativeMode::Hide, ..base.clone() };
        assert!(effective_config(&hide, &EdgeNative(vec![Edge::Bottom])).auto_hide);
        // if the user did not enable auto-hide, touch nothing
        let off = TaskbarConfig { auto_hide: false, ..base };
        assert!(matches!(effective_config(&off, &EdgeNative(vec![Edge::Bottom])), Cow::Borrowed(_)));
    }

    #[test]
    fn disable_restores_native_before_shutdown() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        // repositioning the AppBar must come AFTER the native taskbar has changed mode (R24)
        assert_eq!(entries(&r.log), ["engine.spawn", "native.apply:Reveal", "engine.replace_appbars"]);
        lock(&r.log).clear();
        r.module.disable().unwrap();
        assert_eq!(entries(&r.log), ["native.restore", "engine.shutdown"]);

        // App exit and restore-defaults follow the same order.
        for exit in [true, false] {
            r.module.enable(&toml::Table::new()).unwrap();
            lock(&r.log).clear();
            if exit { r.module.on_app_exit().unwrap() } else { r.module.restore_os_default().unwrap() }
            assert_eq!(entries(&r.log), ["native.restore", "engine.shutdown"], "exit={exit}");
        }
    }

    #[test]
    fn failed_native_restore_is_returned_so_the_journal_is_kept() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        r.fail_restore.store(true, Ordering::SeqCst);
        // disable / on_app_exit / restore_os_default must all report the error (lib.rs relies on that to NOT delete the journal).
        assert!(r.module.disable().is_err());
        assert!(entries(&r.log).contains(&"engine.shutdown".to_string()), "engine vẫn được dừng");
        for exit in [true, false] {
            r.module.enable(&toml::Table::new()).unwrap();
            let res = if exit { r.module.on_app_exit() } else { r.module.restore_os_default() };
            assert!(res.is_err(), "exit={exit}");
        }
        // Once the restore succeeds, Ok returns.
        r.fail_restore.store(false, Ordering::SeqCst);
        r.module.enable(&toml::Table::new()).unwrap();
        r.module.disable().unwrap();
    }

    #[test]
    fn enable_writes_raw_files_and_flags_native_hidden() {
        let mut r = rig();
        let ctl = r.module.controller();
        assert!(!ctl.native_hidden());
        r.module.enable(&toml::Table::new()).unwrap();
        assert!(ctl.native_hidden());
        let dir = ctl.raw_dir();
        assert_eq!(raw::state(&dir), RawState::Generated);
        assert!(std::fs::read_to_string(dir.join("config.yaml")).unwrap().contains("bars"));
        let mut keep = toml::Table::new();
        keep.insert("native".into(), "keep".into());
        r.module.apply(&keep).unwrap();
        assert!(!ctl.native_hidden());
        r.module.apply(&toml::Table::new()).unwrap();
        assert!(ctl.native_hidden());
        r.module.disable().unwrap();
        assert!(!ctl.native_hidden());
    }

    #[test]
    fn apply_regenerates_reloads_and_reapplies_native() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        lock(&r.log).clear();
        r.module.apply(&toml::Table::new()).unwrap();
        let e = entries(&r.log);
        assert!(e[0].starts_with("engine.reload:"), "{e:?}");
        assert_eq!(e[1], "native.apply:Reveal");
        assert_eq!(e[2], "engine.replace_appbars", "đặt lại AppBar sau native.apply: {e:?}");
        assert_eq!(e.len(), 3);
    }

    #[test]
    fn user_edited_files_are_used_and_never_overwritten() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        let ctl = r.module.controller();
        let dir = ctl.raw_dir();
        std::fs::write(dir.join("styles.css"), "/* mine */").unwrap();
        assert_eq!(ctl.raw_state(), RawState::EditedByUser);
        let mut t = toml::Table::new();
        t.insert("height".into(), 48.into());
        r.module.apply(&t).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("styles.css")).unwrap(), "/* mine */");
        assert_eq!(ctl.raw_state(), RawState::EditedByUser);
        // Use the app's controls again: overwrite and reload.
        lock(&r.log).clear();
        ctl.raw_choose(false).unwrap();
        assert_eq!(ctl.raw_state(), RawState::Generated);
        assert_ne!(std::fs::read_to_string(dir.join("styles.css")).unwrap(), "/* mine */");
        assert!(entries(&r.log)[0].starts_with("engine.reload:"));
        // Turn off and on again while the file is the user's: the engine receives exactly their file.
        std::fs::write(dir.join("config.yaml"), "mine: true\n").unwrap();
        r.module.disable().unwrap();
        r.module.enable(&toml::Table::new()).unwrap();
        assert_eq!(lock(&r.factory.used).last().unwrap().yaml, "mine: true\n");
    }

    #[test]
    fn import_yasb_validates_then_marks_edited_and_reloads() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        let ctl = r.module.controller();
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("config.yaml"), "BAD").unwrap();
        std::fs::write(src.path().join("styles.css"), ".a{}").unwrap();
        let before = raw::read(&ctl.raw_dir()).unwrap();
        let e = ctl.import_yasb(src.path()).unwrap_err();
        assert_eq!(e.key, "taskbar.err.import_invalid");
        assert_eq!(raw::read(&ctl.raw_dir()).unwrap(), before);
        assert_eq!(ctl.raw_state(), RawState::Generated);

        std::fs::write(src.path().join("config.yaml"), "good: 1").unwrap();
        lock(&r.log).clear();
        ctl.import_yasb(src.path()).unwrap();
        assert_eq!(ctl.raw_state(), RawState::EditedByUser);
        assert_eq!(raw::read(&ctl.raw_dir()).unwrap().yaml, "good: 1");
        assert_eq!(entries(&r.log), ["engine.reload:7"]);
    }

    #[test]
    fn spawn_failure_leaves_native_untouched() {
        let mut r = rig_with(true, false);
        let e = r.module.enable(&toml::Table::new()).unwrap_err();
        assert_eq!(e.key, "taskbar.err.spawn");
        assert_eq!(entries(&r.log), ["engine.spawn"]);
        assert!(!r.module.controller().native_hidden());
    }

    #[test]
    fn native_failure_stops_the_bar_and_restores() {
        let mut r = rig_with(false, true);
        let e = r.module.enable(&toml::Table::new()).unwrap_err();
        assert_eq!(e.key, "taskbar.err.native");
        assert_eq!(entries(&r.log), ["engine.spawn", "native.apply:Reveal", "native.restore", "engine.shutdown"]);
        assert!(!r.module.controller().status().enabled);
    }

    #[test]
    fn fatal_event_restores_native_first_marks_error_and_turns_off() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        lock(&r.log).clear();
        let ev = lock(&r.factory.events).clone().unwrap();
        ev(BarEvent::Fatal("panic!".into()));
        assert!(wait_for(|| *lock(&r.turned_off) == 1));
        assert_eq!(entries(&r.log), ["native.restore", "engine.shutdown"]);
        let ctl = r.module.controller();
        let st = ctl.status();
        assert!(!st.enabled && !st.native_hidden);
        assert_eq!(st.error.as_ref().unwrap().key, "taskbar.err.stopped");
        // The host calls disable right after (the switch): the error must remain so the UI can show it.
        r.module.disable().unwrap();
        assert_eq!(ctl.status().error.as_ref().unwrap().key, "taskbar.err.stopped");
        assert_eq!(r.module.status().summary.unwrap().key, "taskbar.err.stopped");
        // Turning it on again clears the error.
        r.module.enable(&toml::Table::new()).unwrap();
        assert!(ctl.status().error.is_none());
    }

    #[test]
    fn session_end_stops_without_turning_the_part_off() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        lock(&r.log).clear();
        let ev = lock(&r.factory.events).clone().unwrap();
        ev(BarEvent::Stopped);
        assert!(wait_for(|| !r.module.controller().status().enabled));
        assert_eq!(entries(&r.log), ["native.restore", "engine.shutdown"]);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(*lock(&r.turned_off), 0, "đăng xuất không được tắt phần (app.toml giữ nguyên)");
        assert!(!r.module.controller().native_hidden());
        // TurnOff (initiated by the user) calls the hook.
        r.module.enable(&toml::Table::new()).unwrap();
        let ev = lock(&r.factory.events).clone().unwrap();
        ev(BarEvent::TurnOff);
        assert_eq!(*lock(&r.turned_off), 1);
    }

    #[test]
    fn raw_choose_while_disabled_uses_the_saved_config() {
        let r = rig();
        let ctl = r.module.controller();
        let cfg_file = r._dir.path().join("r").join("config").join("taskbar.toml");
        std::fs::create_dir_all(cfg_file.parent().unwrap()).unwrap();
        std::fs::write(&cfg_file, "height = 52
").unwrap();
        ctl.raw_choose(false).unwrap();
        let g = raw::read(&ctl.raw_dir()).unwrap();
        assert!(g.css.contains("52") || g.yaml.contains("52"), "tệp phải theo cấu hình đã lưu (height 52)");
        assert_eq!(ctl.raw_state(), RawState::Generated);
    }

    #[test]
    fn events_from_a_stale_bar_are_ignored() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        let old = lock(&r.factory.events).clone().unwrap();
        r.module.disable().unwrap();
        r.module.enable(&toml::Table::new()).unwrap();
        lock(&r.log).clear();
        old(BarEvent::Fatal("late".into()));
        std::thread::sleep(Duration::from_millis(100));
        assert!(entries(&r.log).is_empty());
        assert_eq!(*lock(&r.turned_off), 0);
        assert!(r.module.controller().status().enabled);
    }

    #[test]
    fn turn_off_and_open_app_use_the_hooks_and_rejected_keeps_running() {
        let mut r = rig();
        r.module.enable(&toml::Table::new()).unwrap();
        let ev = lock(&r.factory.events).clone().unwrap();
        ev(BarEvent::OpenApp);
        ev(BarEvent::TurnOff);
        assert_eq!((*lock(&r.opened), *lock(&r.turned_off)), (1, 1));
        // TurnOff only asks the app to turn it off: the event itself does not touch the engine or the native taskbar.
        assert!(r.module.controller().status().enabled);
        ev(BarEvent::Rejected("x".into()));
        assert!(wait_for(|| r.module.controller().status().error.is_some()));
        let st = r.module.controller().status();
        assert!(st.enabled);
        assert_eq!(st.error.unwrap().key, "taskbar.err.config_rejected");
        // The next apply clears the warning.
        r.module.apply(&toml::Table::new()).unwrap();
        assert!(r.module.controller().status().error.is_none());
    }

    #[test]
    fn explorer_restart_reapplies_native_and_reloads_only_when_enabled() {
        let mut r = rig();
        let ctl = r.module.controller();
        ctl.on_explorer_restarted();
        std::thread::sleep(Duration::from_millis(100));
        assert!(entries(&r.log).is_empty());
        r.module.enable(&toml::Table::new()).unwrap();
        lock(&r.log).clear();
        ctl.on_explorer_restarted();
        assert!(wait_for(|| entries(&r.log).len() == 3));
        let e = entries(&r.log);
        assert_eq!(e[0], "native.reapply");
        // It must be the path that forces AppBar re-registration, not a plain reload.
        assert!(e[1].starts_with("engine.reload_forced:"), "{e:?}");
        assert_eq!(e[2], "engine.replace_appbars", "đặt lại AppBar sau native.reapply: {e:?}");
    }

    #[test]
    fn status_details_and_catalog() {
        let mut r = rig();
        assert!(r.module.status().details.is_empty());
        r.module.enable(&toml::Table::new()).unwrap();
        let s = r.module.status();
        let keys: Vec<_> = s.details.iter().map(|(k, _)| k.key.as_str()).collect();
        assert_eq!(keys, ["taskbar.status.position", "taskbar.status.height", "taskbar.status.widgets", "taskbar.status.native"]);
        assert_eq!(s.details[0].1.key, "taskbar.edge.bottom");
        assert_eq!(s.details[1].1.args["n"], "36");
        assert_eq!(s.details[2].1.args["n"], "9");
        assert_eq!(s.details[3].1.key, "taskbar.native.reveal");

        let cat = widget_catalog();
        assert_eq!(cat.len(), 13);
        assert_eq!(cat[0].kind, "windows_button");
        assert_eq!(cat[0].name_key, "taskbar.widget.windows_button.name");
        assert_eq!(cat[0].desc_key, "taskbar.widget.windows_button.desc");
        assert!(cat.iter().all(|c| c.singleton));
    }

    #[test]
    fn availability_follows_platform() {
        let r = rig();
        let a = r.module.availability();
        assert_eq!(a, Availability::Available);
    }
}
