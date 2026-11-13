//! Live wallpaper. The engine is ported from TachyonWallpaper (idea originating from Lively Wallpaper, GPL-3.0)
//! as a library: no tray, no separate WebView2 control panel; every action goes through the app's Wallpaper page.
//!
//! The FFI part is ported unchanged from TachyonWallpaper, so it keeps the whole-function `unsafe` style; older clippy lints are relaxed here.
#![allow(unsafe_op_in_unsafe_fn, clippy::collapsible_if, clippy::too_many_arguments)]

#[macro_use]
pub mod util;

pub mod assets;
pub mod backup;
pub mod config;
pub mod frame;
pub mod library;
pub mod module;
pub mod properties;
pub mod service;
pub mod state;
pub mod thumbs;

pub mod engine;

pub mod job;
pub mod playback;
pub mod player;

pub use config::{Arrangement, Scaling, Target, TargetKind, VideoPlayer, WallpaperConfig, WallpaperPaths, plan_targets};
pub use engine::EngineHandle;

/// Finds `mpv.exe` (config → next to the exe → scoop → Program Files → PATH).
pub fn find_mpv(cfg: &WallpaperConfig) -> Option<std::path::PathBuf> {
    player::mpv::find_mpv(cfg)
}
pub use library::{Wallpaper, WallpaperType};
pub use module::WallpaperModule;
pub use service::WallpaperService;
pub use state::{EngineEvent, EngineState, RunningView};
