//! `LabelsService` (shared with the Tauri commands) + `LabelsModule` (plugged into ModuleHost).
//!
//! - Enable: load the DLL (`ExplorerBridge::ensure_loaded`) → send `Labels(..)`. If loading fails (blocked by antivirus, missing DLL,
//!   old DLL still in Explorer) → **fallback method** (a single shared color) and the status states the reason.
//! - Auto: a dedicated thread receives the text layout from the DLL, measures the background brightness right behind each name (wallpaper without icons), with hysteresis,
//!   and sends a per-item color table. Live wallpaper: re-measure every 1 s; static: when the layout or the wallpaper changes.
//! - Disable / app exit / restore: `LabelsOff` + restore the original colors of the fallback method.

use crate::auto::{Hysteresis, auto_style, luminance};
use crate::config::{LabelsConfig, Mode};
use crate::fallback::{Fallback, ListViewColors, RealListView};
use image::RgbaImage;
use serde::Serialize;
use sg_core::guard::RestoreAction;
use sg_core::ids::ModuleId;
use sg_core::module::{Availability, Module, ModuleCtx, ModuleStatus};
use sg_core::text::I18nText;
use sg_desktop::Monitor;
use sg_explorer::{BridgeError, BridgeEvent, ExplorerBridge};
use sg_explorer_proto::{FromDll, ItemStyle, LabelRect, LabelsPaint, PaintMode, ToDll};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

/// Current wallpaper source: `key` changes = image changed; `animated` = live wallpaper (re-measured every second).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BgIdentity {
    pub key: String,
    pub animated: bool,
}

pub type IdentityFn = Arc<dyn Fn() -> BgIdentity + Send + Sync>;
pub type CaptureFn = Arc<dyn Fn(&Monitor) -> Option<RgbaImage> + Send + Sync>;
pub type MonitorsFn = Arc<dyn Fn() -> Vec<Monitor> + Send + Sync>;

/// How the background is obtained (real: `sg-capture` + `sg-wallpaper` wired in by `src-tauri`; tests: fake).
#[derive(Clone)]
pub struct Backgrounds {
    /// Cheap: called every 2 s to find out whether the background changed.
    pub identity: IdentityFn,
    /// Expensive (~0.1 s): the wallpaper image (without icons) of one monitor, already shrunk.
    pub capture: CaptureFn,
    pub monitors: MonitorsFn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    Dll,
    Fallback,
    Off,
}

/// Status for the desktop labels page (`labels_status`, the `labels-status` event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelsStatus {
    pub method: Method,
    pub mode: &'static str,
    pub explorer_pid: Option<u32>,
    /// Number of file names currently painted individually (Auto mode via the DLL).
    pub items: usize,
    /// Why it is in fallback mode / the most recent error.
    pub error: Option<I18nText>,
    pub needs_explorer_restart: bool,
    /// `static` | `animated` | `none`.
    pub source: &'static str,
}

struct Run {
    cfg: LabelsConfig,
    enabled: bool,
    method: Method,
    error: Option<I18nText>,
    needs_restart: bool,
    items: usize,
    source: &'static str,
}

enum WorkerMsg {
    Layout(Vec<LabelRect>),
    Reconfigure,
    ExplorerRestarted,
    Stop,
}

struct Worker {
    tx: Sender<WorkerMsg>,
    join: std::thread::JoinHandle<()>,
}

pub struct LabelsService {
    ctx: ModuleCtx,
    bridge: Arc<ExplorerBridge>,
    bg: Backgrounds,
    fallback: Fallback,
    run: Mutex<Run>,
    worker: Mutex<Option<Worker>>,
    sub: Mutex<Option<u64>>,
    me: Weak<LabelsService>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn reason(e: &BridgeError) -> I18nText {
    I18nText::new(match e {
        BridgeError::NoDll => "labels.reason.no_dll",
        BridgeError::HookFailed(_) => "labels.reason.hook_failed",
        BridgeError::Timeout => "labels.reason.blocked",
        BridgeError::VersionMismatch => "labels.reason.version_mismatch",
        BridgeError::Unsupported => "labels.reason.unsupported",
    })
}

impl LabelsService {
    pub fn new(ctx: ModuleCtx, bridge: Arc<ExplorerBridge>, bg: Backgrounds) -> Arc<LabelsService> {
        let backup = sg_core::labels_backup::path(&ctx.paths);
        LabelsService::with_listview(ctx, bridge, bg, Box::new(RealListView), backup)
    }

    pub fn with_listview(
        ctx: ModuleCtx,
        bridge: Arc<ExplorerBridge>,
        bg: Backgrounds,
        lv: Box<dyn ListViewColors>,
        backup: std::path::PathBuf,
    ) -> Arc<LabelsService> {
        Arc::new_cyclic(|me| LabelsService {
            ctx,
            bridge,
            bg,
            fallback: Fallback::new(lv, backup),
            run: Mutex::new(Run {
                cfg: LabelsConfig::default(),
                enabled: false,
                method: Method::Off,
                error: None,
                needs_restart: false,
                items: 0,
                source: "none",
            }),
            worker: Mutex::new(None),
            sub: Mutex::new(None),
            me: me.clone(),
        })
    }

    pub fn status(&self) -> LabelsStatus {
        let r = lock(&self.run);
        let b = self.bridge.status();
        LabelsStatus {
            method: r.method,
            mode: r.cfg.mode.as_str(),
            explorer_pid: b.explorer_pid,
            items: r.items,
            error: r.error.clone(),
            needs_explorer_restart: r.needs_restart || b.needs_explorer_restart,
            source: r.source,
        }
    }

    pub fn is_enabled(&self) -> bool {
        lock(&self.run).enabled
    }

    fn emit_status(&self) {
        self.ctx.emit("labels-status", self.status());
    }

    fn subscribe(&self) {
        let mut sub = lock(&self.sub);
        if sub.is_some() {
            return;
        }
        let me = self.me.clone();
        *sub = Some(self.bridge.subscribe(move |ev| {
            if let Some(s) = me.upgrade() {
                s.on_bridge(ev);
            }
        }));
    }

    /// Called on the bridge's server/watcher thread: do no heavy work here.
    fn on_bridge(&self, ev: &BridgeEvent) {
        match ev {
            BridgeEvent::Message(FromDll::LabelLayout { items }) => self.to_worker(WorkerMsg::Layout(items.clone())),
            BridgeEvent::Message(FromDll::Status { error: Some(e), .. })
                if e == sg_explorer_dll_errors::VERSION_MISMATCH || e.starts_with("fault") =>
            {
                let why = if e.starts_with("fault") { "labels.reason.dll_fault" } else { "labels.reason.version_mismatch" };
                self.spawn_switch_to_fallback(I18nText::new(why));
            }
            BridgeEvent::ExplorerRestarted => {
                let (enabled, method) = {
                    let r = lock(&self.run);
                    (r.enabled, r.method)
                };
                if enabled && method == Method::Dll && !self.bridge.is_connected() {
                    self.spawn_switch_to_fallback(I18nText::new("labels.reason.blocked"));
                }
                self.to_worker(WorkerMsg::ExplorerRestarted);
                self.emit_status();
            }
            BridgeEvent::Connected(_) | BridgeEvent::Disconnected => self.emit_status(),
            _ => {}
        }
    }

    fn spawn_switch_to_fallback(&self, why: I18nText) {
        let me = self.me.clone();
        let _ = std::thread::Builder::new().name("sg-labels-fallback".into()).spawn(move || {
            if let Some(s) = me.upgrade() {
                s.switch_to_fallback(why);
            }
        });
    }

    fn switch_to_fallback(&self, why: I18nText) {
        {
            let mut r = lock(&self.run);
            if !r.enabled || r.method == Method::Fallback {
                return;
            }
            tracing::warn!(target: "sg::labels", "switching to the fallback: {why}");
            r.needs_restart |= why.key == "labels.reason.version_mismatch";
            r.method = Method::Fallback;
            r.error = Some(why);
            r.items = 0;
        }
        self.bridge.send(ToDll::LabelsOff);
        if let Err(e) = self.apply_now() {
            tracing::warn!(target: "sg::labels", "fallback failed: {e}");
        }
        self.emit_status();
    }

    fn to_worker(&self, m: WorkerMsg) {
        if let Some(w) = lock(&self.worker).as_ref() {
            let _ = w.tx.send(m);
        }
    }

    fn stop_worker(&self) {
        let w = lock(&self.worker).take();
        if let Some(w) = w {
            let _ = w.tx.send(WorkerMsg::Stop);
            let _ = w.join.join();
        }
    }

    fn ensure_worker(&self) {
        let mut g = lock(&self.worker);
        if let Some(w) = g.as_ref() {
            let _ = w.tx.send(WorkerMsg::Reconfigure);
            return;
        }
        let (tx, rx) = channel();
        let me = self.me.clone();
        match std::thread::Builder::new().name("sg-labels-auto".into()).spawn(move || worker_loop(me, rx)) {
            Ok(join) => *g = Some(Worker { tx, join }),
            Err(e) => tracing::warn!(target: "sg::labels", "cannot start the labels worker: {e}"),
        }
    }

    pub fn enable(&self, cfg: LabelsConfig) -> Result<(), I18nText> {
        {
            let mut r = lock(&self.run);
            r.cfg = cfg;
            r.enabled = true;
            r.error = None;
            r.needs_restart = false;
        }
        self.subscribe();
        let loaded = self.bridge.ensure_loaded();
        {
            let mut r = lock(&self.run);
            match &loaded {
                Ok(()) => {
                    r.method = Method::Dll;
                }
                Err(e) => {
                    tracing::warn!(target: "sg::labels", "DLL not available ({e}); using the fallback");
                    r.method = Method::Fallback;
                    r.error = Some(reason(e));
                    r.needs_restart = *e == BridgeError::VersionMismatch;
                }
            }
        }
        if loaded.is_ok() && self.fallback.is_applied() {
            // Last time the fallback method was used (or it crashed midway): restore the original colors before the DLL paints.
            self.restore_fallback();
        }
        let r = self.apply_now();
        self.emit_status();
        r
    }

    pub fn apply(&self, cfg: LabelsConfig) -> Result<(), I18nText> {
        lock(&self.run).cfg = cfg;
        let r = self.apply_now();
        self.emit_status();
        r
    }

    /// Applies the current configuration using the method in use.
    fn apply_now(&self) -> Result<(), I18nText> {
        let (cfg, method) = {
            let r = lock(&self.run);
            (r.cfg, r.method)
        };
        match (method, cfg.mode) {
            (Method::Off, _) => Ok(()),
            (Method::Dll, Mode::Custom) => {
                self.stop_worker();
                self.bridge.send(ToDll::Labels(LabelsPaint { mode: PaintMode::Uniform, uniform: Some(cfg.custom.to_style()), items: vec![] }));
                let mut r = lock(&self.run);
                r.items = 0;
                r.source = "none";
                Ok(())
            }
            (Method::Dll, Mode::Auto) => {
                // The background thread must exist before requesting the layout, or an early reply would be dropped.
                self.ensure_worker();
                self.bridge.send(ToDll::GetLayout);
                Ok(())
            }
            (Method::Fallback, mode) => {
                if let Err(e) = self.ctx.journal_add(RestoreAction::RestoreDesktopLabels) {
                    tracing::warn!(target: "sg::labels", "cannot write the restore journal: {e}");
                }
                if mode == Mode::Custom {
                    self.fallback.apply_uniform(cfg.custom.color).map_err(|e| I18nText::new("labels.err.apply").arg("detail", e))?;
                }
                // Auto: the background thread computes the shared color; both modes: the background thread re-applies when Explorer resets the colors.
                self.ensure_worker();
                Ok(())
            }
        }
    }

    fn restore_fallback(&self) {
        match self.fallback.restore() {
            Ok(true) => tracing::info!(target: "sg::labels", "fallback colors restored"),
            Ok(false) => {}
            Err(e) => tracing::warn!(target: "sg::labels", "cannot restore fallback colors: {e}"),
        }
        if !self.fallback.is_applied() {
            let _ = self.ctx.journal_remove(RestoreAction::RestoreDesktopLabels);
        }
    }

    pub fn disable(&self) {
        self.stop_worker();
        self.bridge.send(ToDll::LabelsOff);
        self.restore_fallback();
        {
            let mut r = lock(&self.run);
            r.enabled = false;
            r.method = Method::Off;
            r.items = 0;
            r.source = "none";
        }
        self.emit_status();
    }

    /// The "Restart Explorer" button (confirmed in the UI).
    pub fn restart_explorer(&self) -> Result<(), I18nText> {
        self.bridge.restart_explorer().map_err(|e| I18nText::new("labels.err.restart").arg("detail", e))
    }

    // ---- Background thread ----

    fn snapshot(&self) -> (LabelsConfig, Method, bool) {
        let r = lock(&self.run);
        (r.cfg, r.method, r.enabled)
    }

    fn set_progress(&self, items: usize, source: &'static str) {
        let changed = {
            let mut r = lock(&self.run);
            let c = r.items != items || r.source != source;
            r.items = items;
            r.source = source;
            c
        };
        if changed {
            self.emit_status();
        }
    }
}

/// Only so the error string sent up by the DLL is not mistyped.
mod sg_explorer_dll_errors {
    pub const VERSION_MISMATCH: &str = "version-mismatch";
}

fn rect_center(r: &LabelRect) -> (i32, i32) {
    ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
}

fn monitor_for<'a>(mons: &'a [Monitor], r: &LabelRect) -> Option<&'a Monitor> {
    let (x, y) = rect_center(r);
    mons.iter()
        .find(|m| x >= m.rect.left && x < m.rect.right && y >= m.rect.top && y < m.rect.bottom)
        .or_else(|| mons.first())
}

/// Converts a screen-coordinate region to coordinates within monitor `m`'s wallpaper image.
fn to_image(m: &Monitor, img: &RgbaImage, r: &LabelRect) -> (i32, i32, i32, i32) {
    let sx = img.width() as f32 / m.rect.width().max(1) as f32;
    let sy = img.height() as f32 / m.rect.height().max(1) as f32;
    let f = |v: i32, o: i32, s: f32| ((v - o) as f32 * s).round() as i32;
    (f(r.left, m.rect.left, sx), f(r.top, m.rect.top, sy), f(r.right, m.rect.left, sx).max(f(r.left, m.rect.left, sx) + 1), f(r.bottom, m.rect.top, sy).max(f(r.top, m.rect.top, sy) + 1))
}

/// Per-item color table for Auto mode.
pub fn compute_paint(
    layout: &[LabelRect],
    mons: &[Monitor],
    images: &HashMap<String, RgbaImage>,
    hyst: &mut Hysteresis,
    outline: bool,
    now_ms: u64,
) -> LabelsPaint {
    let mut items = Vec::with_capacity(layout.len());
    for r in layout {
        let Some(m) = monitor_for(mons, r) else { continue };
        let Some(img) = images.get(&m.device) else { continue };
        let Some(lum) = luminance(img, to_image(m, img, r)) else { continue };
        hyst.update(r.index, lum, now_ms);
        if let Some(light) = hyst.get(r.index) {
            items.push(ItemStyle { index: r.index, style: auto_style(light, outline) });
        }
    }
    items.sort_by_key(|i| i.index);
    LabelsPaint { mode: PaintMode::PerItem, uniform: None, items }
}

/// Wallpaper image size used for measuring (enough for a few dozen pixels per file name).
pub const MEASURE_WIDTH: u32 = 960;
const IDENTITY_EVERY_MS: u64 = 2000;
const ANIMATED_EVERY_MS: u64 = 1000;
/// A dummy item for the fallback method (a single shared color).
const FALLBACK_INDEX: u32 = u32::MAX;

fn worker_loop(me: Weak<LabelsService>, rx: Receiver<WorkerMsg>) {
    let start = Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    let mut hyst = Hysteresis::default();
    let mut layout: Vec<LabelRect> = Vec::new();
    let mut ident: Option<BgIdentity> = None;
    let mut cache: HashMap<String, RgbaImage> = HashMap::new();
    let mut last_sent: Option<LabelsPaint> = None;
    let (mut last_ident, mut last_compute) = (0u64, 0u64);
    let mut dirty = true;
    tracing::debug!(target: "sg::labels", "labels worker started");
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(WorkerMsg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(WorkerMsg::Layout(items)) => {
                let old: HashMap<u32, LabelRect> = layout.iter().map(|r| (r.index, *r)).collect();
                for r in &items {
                    if old.get(&r.index) != Some(r) {
                        hyst.reset(r.index); // new/moved item: decide again right away based on the new background
                    }
                }
                let keep: std::collections::HashSet<u32> = items.iter().map(|r| r.index).collect();
                hyst.retain(|i| i == FALLBACK_INDEX || keep.contains(&i));
                layout = items;
                dirty = true;
            }
            Ok(WorkerMsg::Reconfigure) => {
                last_sent = None;
                dirty = true;
            }
            Ok(WorkerMsg::ExplorerRestarted) => {
                cache.clear();
                hyst.clear();
                last_sent = None;
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let Some(svc) = me.upgrade() else { break };
        let t = now();
        if ident.is_none() || t.saturating_sub(last_ident) >= IDENTITY_EVERY_MS {
            last_ident = t;
            let id = (svc.bg.identity)();
            if ident.as_ref() != Some(&id) {
                cache.clear();
                hyst.clear();
                ident = Some(id);
                dirty = true;
            }
        }
        let animated = ident.as_ref().is_some_and(|i| i.animated);
        if animated && t.saturating_sub(last_compute) >= ANIMATED_EVERY_MS {
            dirty = true;
        }
        let (cfg, method, enabled) = svc.snapshot();
        if !enabled {
            break;
        }
        if method == Method::Fallback && !dirty && t.saturating_sub(last_compute) >= IDENTITY_EVERY_MS {
            // When Explorer restarts / the theme changes, the colors get reset: check periodically.
            dirty = true;
        }
        if !dirty {
            continue;
        }
        dirty = false;
        last_compute = t;
        let source = if animated { "animated" } else { "static" };
        let mons = (svc.bg.monitors)();
        match (method, cfg.mode) {
            (Method::Dll, Mode::Auto) => {
                if layout.is_empty() {
                    continue;
                }
                // Wallpaper images of the monitors that have file names (static: kept until the background changes; live: re-captured each time).
                let mut images: HashMap<String, RgbaImage> = if animated { HashMap::new() } else { cache.clone() };
                for r in &layout {
                    if let Some(m) = monitor_for(&mons, r)
                        && !images.contains_key(&m.device)
                        && let Some(img) = (svc.bg.capture)(m)
                    {
                        images.insert(m.device.clone(), img);
                    }
                }
                if !animated {
                    cache = images.clone();
                }
                let paint = compute_paint(&layout, &mons, &images, &mut hyst, cfg.outline, t);
                svc.set_progress(paint.items.len(), source);
                if last_sent.as_ref() != Some(&paint) && !paint.items.is_empty() {
                    svc.bridge.send(ToDll::Labels(paint.clone()));
                    last_sent = Some(paint);
                }
            }
            (Method::Fallback, mode) => {
                let color = if mode == Mode::Custom {
                    cfg.custom.color
                } else {
                    // No per-name positions (needs the DLL): measure the left third of the primary monitor, where icons usually are.
                    let m = mons.iter().find(|m| m.primary).or(mons.first());
                    let img = m.and_then(|m| {
                        if !animated && let Some(c) = cache.get(&m.device) {
                            return Some(c.clone());
                        }
                        let i = (svc.bg.capture)(m)?;
                        if !animated {
                            cache.insert(m.device.clone(), i.clone());
                        }
                        Some(i)
                    });
                    let lum = img.as_ref().and_then(|i| luminance(i, (0, 0, i.width() as i32 / 3, i.height() as i32)));
                    if let Some(l) = lum {
                        hyst.update(FALLBACK_INDEX, l, t);
                    }
                    let light = hyst.get(FALLBACK_INDEX).unwrap_or(true);
                    auto_style(light, false).color
                };
                if !svc.fallback.matches(color)
                    && let Err(e) = svc.fallback.apply_uniform(color)
                {
                    tracing::warn!(target: "sg::labels", "fallback color: {e}");
                }
                svc.set_progress(0, if mode == Mode::Auto { source } else { "none" });
            }
            _ => {}
        }
    }
    tracing::debug!(target: "sg::labels", "labels worker stopped");
}

pub struct LabelsModule {
    service: Arc<LabelsService>,
}

impl LabelsModule {
    pub fn new(service: Arc<LabelsService>) -> LabelsModule {
        LabelsModule { service }
    }
}

impl Module for LabelsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Labels
    }

    fn availability(&self) -> Availability {
        Availability::Available
    }

    fn enable(&mut self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.service.enable(LabelsConfig::from_table(cfg))
    }

    fn apply(&mut self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.service.apply(LabelsConfig::from_table(cfg))
    }

    fn disable(&mut self) -> Result<(), I18nText> {
        self.service.disable();
        Ok(())
    }

    fn restore_os_default(&mut self) -> Result<(), I18nText> {
        self.service.disable();
        Ok(())
    }

    fn status(&self) -> ModuleStatus {
        let s = self.service.status();
        if s.method == Method::Off {
            return ModuleStatus::default();
        }
        let mode = I18nText::new(if s.mode == "custom" { "labels.val.custom" } else { "labels.val.auto" });
        let method = I18nText::new(if s.method == Method::Dll { "labels.val.dll" } else { "labels.val.fallback" });
        let mut details = vec![(I18nText::new("labels.status.mode"), mode.clone()), (I18nText::new("labels.status.method"), method)];
        if s.method == Method::Dll && s.mode == "auto" {
            details.push((I18nText::new("labels.status.items"), I18nText::new("labels.val.n_items").arg("n", s.items)));
        }
        if s.source != "none" {
            details.push((
                I18nText::new("labels.status.source"),
                I18nText::new(if s.source == "animated" { "labels.val.animated" } else { "labels.val.static" }),
            ));
        }
        ModuleStatus { summary: Some(mode), details }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback::colorref;
    use crate::fallback::tests::FakeLv;
    use sg_core::paths::Paths;
    use sg_desktop::Rect;

    fn monitor(device: &str, rect: Rect) -> Monitor {
        Monitor { number: 1, device: device.into(), rect, work: rect, primary: true, dpi: 96, handle: 0 }
    }

    fn fake_bg() -> Backgrounds {
        Backgrounds {
            identity: Arc::new(|| BgIdentity { key: "fixed".into(), animated: false }),
            capture: Arc::new(|_| Some(RgbaImage::from_pixel(96, 54, image::Rgba([250, 250, 250, 255])))),
            monitors: Arc::new(|| vec![monitor("A", Rect::new(0, 0, 1920, 1080))]),
        }
    }

    #[test]
    fn compute_paint_maps_rects_to_monitor_images() {
        let mons = vec![monitor("A", Rect::new(0, 0, 100, 100)), Monitor { primary: false, ..monitor("B", Rect::new(100, 0, 200, 100)) }];
        let mut imgs = HashMap::new();
        imgs.insert("A".to_string(), RgbaImage::from_pixel(50, 50, image::Rgba([0, 0, 0, 255])));
        imgs.insert("B".to_string(), RgbaImage::from_pixel(50, 50, image::Rgba([255, 255, 255, 255])));
        let layout = vec![
            LabelRect { index: 2, left: 120, top: 10, right: 180, bottom: 30 },
            LabelRect { index: 0, left: 10, top: 10, right: 60, bottom: 30 },
        ];
        let mut h = Hysteresis::default();
        let p = compute_paint(&layout, &mons, &imgs, &mut h, true, 0);
        assert_eq!(p.mode, PaintMode::PerItem);
        assert_eq!(p.items.iter().map(|i| i.index).collect::<Vec<_>>(), vec![0, 2], "xếp theo chỉ số");
        assert_eq!(p.items[0].style.color, crate::auto::LIGHT_TEXT, "nền đen → chữ sáng");
        assert_eq!(p.items[1].style.color, crate::auto::DARK_TEXT, "nền trắng (màn hình B) → chữ tối");
    }

    #[test]
    fn without_dll_uses_fallback_and_restores() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(d.path(), true, None, None);
        let ctx = ModuleCtx::for_test(paths.clone());
        // A build without the DLL → fallback method immediately.
        let bridge = ExplorerBridge::with_pipe_name(b"", &paths, "unused".into(), None, false);
        let lv = FakeLv::new(0x00FF_FFFF);
        let svc = LabelsService::with_listview(ctx, bridge, fake_bg(), Box::new(lv.clone()), sg_core::labels_backup::path(&paths));
        let cfg = LabelsConfig { mode: Mode::Custom, ..LabelsConfig::default() };
        let mut cfg = cfg;
        cfg.custom.color = 0xFF69B4;
        svc.enable(cfg).unwrap();
        let st = svc.status();
        assert_eq!(st.method, Method::Fallback);
        assert_eq!(st.error.as_ref().map(|e| e.key.as_str()), Some("labels.reason.no_dll"));
        assert_eq!(lv.current().text, colorref(0xFF69B4));
        let journal = sg_core::guard::GuardJournal::open(&paths.guard_journal_file());
        assert_eq!(journal.actions(), vec![RestoreAction::RestoreDesktopLabels], "ghi nhật ký cho tiến trình canh");
        // Auto, white background → dark text (after the background thread runs).
        svc.apply(LabelsConfig::default()).unwrap();
        let t0 = Instant::now();
        while lv.current().text != colorref(crate::auto::DARK_TEXT) && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(lv.current().text, colorref(crate::auto::DARK_TEXT));
        svc.disable();
        assert_eq!(lv.current().text, 0x00FF_FFFF, "tắt → về màu gốc");
        assert_eq!(svc.status().method, Method::Off);
        assert!(sg_core::guard::GuardJournal::open(&paths.guard_journal_file()).actions().is_empty());
    }

    #[test]
    fn status_json_is_camel_case() {
        let s = LabelsStatus {
            method: Method::Fallback,
            mode: "auto",
            explorer_pid: Some(5),
            items: 3,
            error: Some(I18nText::new("labels.reason.blocked")),
            needs_explorer_restart: true,
            source: "static",
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["method"], "fallback");
        assert_eq!(v["explorerPid"], 5);
        assert_eq!(v["needsExplorerRestart"], true);
        assert_eq!(v["error"]["key"], "labels.reason.blocked");
    }
}
