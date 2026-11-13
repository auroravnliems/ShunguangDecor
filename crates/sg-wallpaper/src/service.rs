//! `WallpaperService`: holds the engine (while the part is enabled), the latest state and the library functions for Tauri commands.

use crate::config::{WallpaperConfig, WallpaperPaths};
use crate::engine::EngineHandle;
use crate::state::{EngineEvent, EngineState};
use sg_core::module::ModuleCtx;
use sg_core::text::I18nText;
use std::sync::{Arc, Mutex, MutexGuard};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct WallpaperService {
    ctx: ModuleCtx,
    paths: WallpaperPaths,
    engine: Mutex<Option<Arc<EngineHandle>>>,
    state: Arc<Mutex<EngineState>>,
    /// The running configuration (so `status()` can read the volume, etc. without asking the engine).
    cfg: Mutex<WallpaperConfig>,
}

fn engine_err(detail: impl ToString) -> I18nText {
    I18nText::new("wallpaper.err.engine").arg("detail", detail)
}

impl WallpaperService {
    pub fn new(ctx: ModuleCtx) -> Arc<WallpaperService> {
        let paths = WallpaperPaths::from_paths(&ctx.paths);
        Arc::new(WallpaperService {
            ctx,
            paths,
            engine: Mutex::new(None),
            state: Arc::new(Mutex::new(EngineState::default())),
            cfg: Mutex::new(WallpaperConfig::default()),
        })
    }

    pub fn paths(&self) -> &WallpaperPaths {
        &self.paths
    }

    pub fn ctx(&self) -> &ModuleCtx {
        &self.ctx
    }

    pub fn engine(&self) -> Option<Arc<EngineHandle>> {
        lock(&self.engine).clone()
    }

    pub fn is_running(&self) -> bool {
        lock(&self.engine).is_some()
    }

    /// The latest state the engine has reported (empty while the part is off).
    pub fn state(&self) -> EngineState {
        lock(&self.state).clone()
    }

    pub fn config(&self) -> WallpaperConfig {
        lock(&self.cfg).clone()
    }

    /// Enable: build the engine if it does not exist yet, then apply the configuration.
    pub fn enable(&self, cfg: WallpaperConfig) -> Result<(), I18nText> {
        let handle = {
            let mut g = lock(&self.engine);
            if g.is_none() {
                let ctx = self.ctx.clone();
                let state = self.state.clone();
                let h = EngineHandle::spawn(self.paths.clone(), cfg.clone(), move |ev| match ev {
                    EngineEvent::State(s) => {
                        *lock(&state) = s.clone();
                        ctx.emit("wallpaper-state", s);
                    }
                    EngineEvent::Error { wallpaper_id, message } => {
                        ctx.emit("wallpaper-error", serde_json::json!({ "wallpaperId": wallpaper_id, "message": message }));
                    }
                })
                .map_err(engine_err)?;
                *g = Some(Arc::new(h));
            }
            g.clone().expect("engine vừa được tạo")
        };
        self.apply_to(&handle, cfg)
    }

    pub fn apply(&self, cfg: WallpaperConfig) -> Result<(), I18nText> {
        match self.engine() {
            Some(h) => self.apply_to(&h, cfg),
            None => Ok(()),
        }
    }

    fn apply_to(&self, h: &EngineHandle, cfg: WallpaperConfig) -> Result<(), I18nText> {
        *lock(&self.cfg) = cfg.clone();
        h.apply_config(cfg).map_err(engine_err)
    }

    /// Disable: close every player, restore the old wallpaper, exit the thread. Calling it twice is harmless.
    pub fn disable(&self) {
        let h = lock(&self.engine).take();
        if let Some(h) = h {
            h.shutdown();
        }
        *lock(&self.state) = EngineState::default();
    }

    /// Disable and forget the running layout.
    pub fn restore_os_default(&self) {
        self.disable();
        lock(&self.cfg).layout.clear();
    }

    pub fn toggle_pause(&self) -> Result<bool, I18nText> {
        self.engine().ok_or_else(|| I18nText::new("wallpaper.err.not_enabled"))?.toggle_pause().map_err(engine_err)
    }
}
