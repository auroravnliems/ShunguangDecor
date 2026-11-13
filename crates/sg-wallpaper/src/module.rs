//! `WallpaperModule`: plugs the Wallpaper part into the ModuleHost.

use crate::config::WallpaperConfig;
use crate::library;
use crate::service::WallpaperService;
use sg_core::ids::ModuleId;
use sg_core::module::{Availability, Module, ModuleStatus};
use sg_core::text::I18nText;
use std::sync::Arc;

pub struct WallpaperModule {
    service: Arc<WallpaperService>,
}

impl WallpaperModule {
    pub fn new(service: Arc<WallpaperService>) -> WallpaperModule {
        WallpaperModule { service }
    }
}

fn text(v: impl ToString) -> I18nText {
    I18nText::new("wallpaper.val.text").arg("v", v)
}

fn backend_label(b: &str) -> I18nText {
    I18nText::new(match b {
        "media-foundation" => "wallpaper.val.mf",
        "mpv" => "wallpaper.val.mpv",
        "webview2" => "wallpaper.val.web",
        "windows-wallpaper" => "wallpaper.val.picture",
        "app" => "wallpaper.val.app",
        _ => "wallpaper.val.unknown",
    })
}

impl Module for WallpaperModule {
    fn id(&self) -> ModuleId {
        ModuleId::Wallpaper
    }

    fn availability(&self) -> Availability {
        Availability::Available
    }

    fn enable(&mut self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.service.enable(WallpaperConfig::from_table(cfg))
    }

    fn apply(&mut self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.service.apply(WallpaperConfig::from_table(cfg))
    }

    fn disable(&mut self) -> Result<(), I18nText> {
        self.service.disable();
        Ok(())
    }

    fn restore_os_default(&mut self) -> Result<(), I18nText> {
        self.service.restore_os_default();
        Ok(())
    }

    fn on_app_exit(&mut self) -> Result<(), I18nText> {
        self.service.disable();
        Ok(())
    }

    fn status(&self) -> ModuleStatus {
        if !self.service.is_running() {
            return ModuleStatus::default();
        }
        let st = self.service.state();
        let cfg = self.service.config();
        // Wallpaper name of the primary monitor (number 1), or the first one running.
        let main = st.running.iter().find(|r| r.monitor == 1 || r.monitor == 0).or(st.running.first());
        let summary = main.map(|r| text(library::find(self.service.paths(), &r.wallpaper_id).map(|w| w.title()).unwrap_or_else(|| r.wallpaper_id.clone())));
        let mut details = vec![(I18nText::new("wallpaper.status.running"), I18nText::new("wallpaper.val.n_monitors").arg("n", st.running.len()))];
        if let Some(r) = main {
            details.push((I18nText::new("wallpaper.status.player"), backend_label(&r.backend)));
        }
        details.push((
            I18nText::new("wallpaper.status.state"),
            I18nText::new(if st.manual_pause || st.running.iter().any(|r| r.paused) { "wallpaper.val.paused" } else { "wallpaper.val.playing" }),
        ));
        details.push((I18nText::new("wallpaper.status.volume"), I18nText::new("wallpaper.val.percent").arg("n", cfg.volume)));
        ModuleStatus { summary, details }
    }
}
