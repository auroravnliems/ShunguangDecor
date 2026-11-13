//! System tray menu: open the app, quick switches for the 6 modules, pause the wallpaper,
//! restore defaults, quit. Labels come from the same translation files as the UI.

use crate::state::{AppState, lock};
use crate::window;
use sg_core::i18n::{Lang, t};
use sg_core::ids::ModuleId;
use sg_core::module::{Availability, ModuleInfo};
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, Wry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    Toggle(ModuleId),
    WallpaperPause,
    Restore,
    Quit,
}

pub fn parse_menu_id(id: &str) -> Option<MenuAction> {
    match id {
        "open" => Some(MenuAction::Open),
        "wp-pause" => Some(MenuAction::WallpaperPause),
        "restore" => Some(MenuAction::Restore),
        "quit" => Some(MenuAction::Quit),
        _ => id.strip_prefix("mod:").and_then(ModuleId::parse).map(MenuAction::Toggle),
    }
}

/// A cheap copy (the menu items are Arcs inside): take it out of the lock first, then call into the menu.
#[derive(Clone)]
pub struct TrayHandles {
    open: MenuItem<Wry>,
    toggles: Vec<(ModuleId, CheckMenuItem<Wry>)>,
    wallpaper_pause: MenuItem<Wry>,
    /// The wallpaper is currently paused by the user (decides the item's label).
    wp_paused: Arc<AtomicBool>,
    restore: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

fn module_label(info: &ModuleInfo, lang: Lang) -> String {
    let name = t(lang, &format!("module.{}.name", info.id), &[]);
    if info.fault.is_some() { t(lang, "tray.faulted_suffix", &[("name", &name)]) } else { name }
}

fn usable(info: &ModuleInfo) -> bool {
    !matches!(info.availability, Availability::Unsupported { .. })
}

pub fn build(app: &AppHandle, lang: Lang, infos: &[ModuleInfo]) -> tauri::Result<TrayHandles> {
    let open = MenuItem::with_id(app, "open", t(lang, "tray.open", &[]), true, None::<&str>)?;
    let toggles = infos
        .iter()
        .map(|i| {
            CheckMenuItem::with_id(app, format!("mod:{}", i.id), module_label(i, lang), usable(i), i.enabled, None::<&str>)
                .map(|item| (i.id, item))
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    // Enabled when the Wallpaper module is on; the label changes with the paused state.
    let wp_on = infos.iter().any(|i| i.id == ModuleId::Wallpaper && i.enabled);
    let wallpaper_pause = MenuItem::with_id(app, "wp-pause", t(lang, "tray.wallpaper_pause", &[]), wp_on, None::<&str>)?;
    let restore = MenuItem::with_id(app, "restore", t(lang, "tray.restore", &[]), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", t(lang, "tray.quit", &[]), true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;

    let mut items: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = vec![&open, &sep1];
    for (_, item) in &toggles {
        items.push(item);
    }
    items.extend([&wallpaper_pause as &dyn tauri::menu::IsMenuItem<Wry>, &sep2, &restore, &quit]);
    let menu = Menu::with_items(app, &items)?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(t(lang, "tray.tooltip", &[]))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                window::open_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(TrayHandles { open, toggles, wallpaper_pause, wp_paused: Arc::new(AtomicBool::new(false)), restore, quit })
}

impl TrayHandles {
    /// Language changed → reset the label of every item.
    pub fn relabel(&self, lang: Lang, infos: &[ModuleInfo]) {
        let _ = self.open.set_text(t(lang, "tray.open", &[]));
        self.relabel_wallpaper(lang);
        let _ = self.restore.set_text(t(lang, "tray.restore", &[]));
        let _ = self.quit.set_text(t(lang, "tray.quit", &[]));
        for i in infos {
            self.sync(i, lang);
        }
    }

    fn relabel_wallpaper(&self, lang: Lang) {
        let key = if self.wp_paused.load(Ordering::SeqCst) { "tray.wallpaper_resume" } else { "tray.wallpaper_pause" };
        let _ = self.wallpaper_pause.set_text(t(lang, key, &[]));
    }

    /// Wallpaper paused/resumed → change the tray item's label.
    pub fn set_wallpaper_paused(&self, paused: bool, lang: Lang) {
        self.wp_paused.store(paused, Ordering::SeqCst);
        self.relabel_wallpaper(lang);
    }

    /// A module changed state → update the checkmark, label, and enabled/disabled state of the item.
    pub fn sync(&self, info: &ModuleInfo, lang: Lang) {
        if info.id == ModuleId::Wallpaper {
            let _ = self.wallpaper_pause.set_enabled(info.enabled);
            if !info.enabled {
                self.set_wallpaper_paused(false, lang);
            }
        }
        if let Some((_, item)) = self.toggles.iter().find(|(id, _)| *id == info.id) {
            let _ = item.set_text(module_label(info, lang));
            let _ = item.set_checked(info.enabled);
            let _ = item.set_enabled(usable(info));
        }
    }
}

/// Turning off the Hidden features module from the tray requires asking "undo?" in the UI, so the tray does not turn it off by itself (Ruling S32).
fn tray_toggle_needs_confirm(id: ModuleId, enabled: bool) -> bool {
    id == ModuleId::Features && enabled
}

fn on_menu_event(app: &AppHandle, e: MenuEvent) {
    let Some(action) = parse_menu_id(e.id.as_ref()) else { return };
    match action {
        MenuAction::Open => window::open_main(app),
        MenuAction::Toggle(id) => {
            let app = app.clone();
            // Turning a module on/off can take a few seconds: do not do it on the main thread.
            tauri::async_runtime::spawn_blocking(move || {
                let state = app.state::<AppState>();
                let enabled = state.host.info(id).enabled;
                if tray_toggle_needs_confirm(id, enabled) {
                    // Same as "confirm-restore": write the pending queue + notify the UI + open the window; the UI asks Yes/No/Cancel.
                    *lock(&state.pending_intent) = Some("confirm-disable-features".into());
                    window::emit_main(&app, "intent", "confirm-disable-features");
                    window::open_main(&app);
                } else if let Err(e) = crate::commands::toggle_module(&state, id, !enabled) {
                    tracing::warn!(target: "sg::app", "tray toggle {id} failed: {e}");
                }
                // CheckMenuItem flips its checkmark itself when clicked: sync it back to the real state.
                let info = state.host.info(id);
                if let Some(tray) = state.tray() {
                    tray.sync(&info, state.lang());
                }
            });
        }
        MenuAction::WallpaperPause => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let state = app.state::<AppState>();
                match state.services.wallpaper.toggle_pause() {
                    Ok(p) => {
                        if let Some(tray) = state.tray() {
                            tray.set_wallpaper_paused(p, state.lang());
                        }
                    }
                    Err(e) => tracing::warn!(target: "sg::app", "tray wallpaper pause failed: {}", e.key),
                }
            });
        }
        MenuAction::Restore => {
            // Always write to the pending queue: the UI picks it up right after loading, or when it hears the "intent" event
            // (the window is already open). Taking it removes it, so even if both paths run, the confirmation box opens only once.
            *lock(&app.state::<AppState>().pending_intent) = Some("confirm-restore".into());
            window::emit_main(app, "intent", "confirm-restore");
            window::open_main(app);
        }
        MenuAction::Quit => crate::quit(app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_turning_features_off_needs_confirm() {
        assert!(tray_toggle_needs_confirm(ModuleId::Features, true));
        assert!(!tray_toggle_needs_confirm(ModuleId::Features, false));
        assert!(!tray_toggle_needs_confirm(ModuleId::Cursor, true));
    }

    #[test]
    fn parse_menu_id_cases() {
        assert_eq!(parse_menu_id("mod:cursor"), Some(MenuAction::Toggle(ModuleId::Cursor)));
        assert_eq!(parse_menu_id("mod:abc"), None);
        assert_eq!(parse_menu_id("quit"), Some(MenuAction::Quit));
        assert_eq!(parse_menu_id("open"), Some(MenuAction::Open));
        assert_eq!(parse_menu_id("restore"), Some(MenuAction::Restore));
        assert_eq!(parse_menu_id(""), None);
    }
}
