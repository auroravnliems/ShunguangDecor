//! Where the 5 parts are assembled into the app, along with the services (library, file readers…) that each part's Tauri commands need (`Services`).
//!
//! Each part's own Tauri commands live in `modules/<part>.rs` and are registered in `lib.rs`.

pub mod capture;
pub mod cursor;
pub mod features;
pub mod labels;
pub mod taskbar;
pub mod wallpaper;

use sg_core::i18n::Lang;
use sg_core::ids::ModuleId;
use sg_core::module::{Module, ModuleCtx};
use sg_capture::CaptureService;
use sg_cursor::{CursorModule, CursorService};
use sg_explorer::{BridgeEvent, ExplorerBridge};
use sg_features::{FeaturesModule, FeaturesService};
use sg_labels::{Backgrounds, BgIdentity, LabelsModule, LabelsService};
use sg_taskbar::{Hooks, TaskbarController, TaskbarModule};
use sg_wallpaper::{WallpaperModule, WallpaperService};
use std::sync::Arc;

/// `shunguang_explorer.dll` embedded in the exe (`src-tauri/build.rs`); empty if the build has no DLL.
pub static EXPLORER_DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shunguang_explorer.dll"));

/// Services shared between a part (which runs on its own thread) and the Tauri commands.
pub struct Services {
    pub cursor: Arc<CursorService>,
    pub capture: Arc<CaptureService>,
    pub wallpaper: Arc<WallpaperService>,
    /// Bridge to the DLL inside Explorer (the desktop labels part).
    pub explorer: Arc<ExplorerBridge>,
    pub labels: Arc<LabelsService>,
    pub features: Arc<FeaturesService>,
    pub taskbar: TaskbarController,
}

/// The background right behind the file names for the Automatic mode of the desktop labels part: the animated wallpaper if one is running, otherwise
/// the Windows wallpaper image. The image is kept in memory only.
fn label_backgrounds(wallpaper: &Arc<WallpaperService>) -> Backgrounds {
    let wp = wallpaper.clone();
    let identity = Arc::new(move || {
        if wp.is_running() {
            let st = wp.state();
            if !st.running.is_empty() {
                let animated = !st.manual_pause && st.running.iter().any(|r| r.backend != "windows-wallpaper" && !r.paused);
                let key = st.running.iter().map(|r| format!("{}={}", r.monitor, r.wallpaper_id)).collect::<Vec<_>>().join(",");
                return BgIdentity { key: format!("wallpaper:{key}"), animated };
            }
        }
        let key = sg_core::desktop::current_wallpaper().map(|w| format!("os:{}:{}", w.path.display(), w.modified_unix)).unwrap_or_default();
        BgIdentity { key, animated: false }
    });
    Backgrounds {
        identity,
        capture: Arc::new(|m| sg_capture::capture_background(m, sg_labels::module::MEASURE_WIDTH)),
        monitors: Arc::new(sg_desktop::monitors),
    }
}

/// Creates all 5 parts in `ModuleId::ALL` order. `hooks` are the things the YASB Taskbar asks the app to do (turn a part off, open a window).
pub fn build(ctx: &ModuleCtx, lang: Arc<dyn Fn() -> Lang + Send + Sync>, mut hooks: Hooks) -> (Vec<Box<dyn Module>>, Services) {
    let cursor = CursorService::new(ctx.clone());
    let emit_ctx = ctx.clone();
    let capture = CaptureService::new(move |f| emit_ctx.emit("capture-frame", f));
    let wallpaper = WallpaperService::new(ctx.clone());
    // A web/stream/app wallpaper is running: the full preview frame captures every 100 ms instead of 500 ms.
    let probe = wallpaper.clone();
    capture.set_dynamic_probe(move |monitor| capture::is_dynamic_capture(&probe, monitor));
    let explorer = ExplorerBridge::new(EXPLORER_DLL, &ctx.paths);
    let labels = LabelsService::new(ctx.clone(), explorer.clone(), label_backgrounds(&wallpaper));
    let features = FeaturesService::new_real(&sg_features::undo::journal_path(&ctx.paths));
    let taskbar = TaskbarModule::new(ctx.clone(), lang);
    let taskbar_ctl = taskbar.controller();
    // Explorer restarted (TaskbarCreated): the Taskbar part re-applies the native taskbar mode and re-registers the AppBar.
    let ctl = taskbar_ctl.clone();
    explorer.subscribe(move |ev| {
        if matches!(ev, BridgeEvent::ExplorerRestarted) {
            ctl.on_explorer_restarted();
        }
    });
    // The Explorer watcher only runs when some part needs it (the Taskbar part needs it even without loading the DLL).
    let bridge = explorer.clone();
    hooks.watch_explorer = Arc::new(move || bridge.ensure_watching());
    taskbar_ctl.set_hooks(hooks);
    // Preview frame approach (b): the live frame of the running animated wallpaper.
    let provider = wallpaper.clone();
    sg_capture::set_wallpaper_frame_provider(move |m| provider.engine()?.snapshot(m.number, false));
    let provider = wallpaper.clone();
    sg_capture::set_window_frame_provider(move |m| provider.engine()?.snapshot(m.number, true));
    let mut taskbar = Some(taskbar);
    let modules = ModuleId::ALL
        .iter()
        .map(|&id| match id {
            ModuleId::Cursor => Box::new(CursorModule::new(cursor.clone())) as Box<dyn Module>,
            ModuleId::Wallpaper => Box::new(WallpaperModule::new(wallpaper.clone())) as Box<dyn Module>,
            ModuleId::Labels => Box::new(LabelsModule::new(labels.clone())) as Box<dyn Module>,
            ModuleId::Features => Box::new(FeaturesModule::new(features.clone())) as Box<dyn Module>,
            ModuleId::Taskbar => Box::new(taskbar.take().expect("Taskbar chỉ có một lần trong ModuleId::ALL")) as Box<dyn Module>,
        })
        .collect();
    (modules, Services { cursor, capture, wallpaper, explorer, labels, features, taskbar: taskbar_ctl })
}
