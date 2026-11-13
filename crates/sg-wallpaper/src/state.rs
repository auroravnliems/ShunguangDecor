//! Types shared between the engine (which runs on its own thread) and the rest of the app.

use serde::Serialize;
use std::collections::BTreeMap;

/// A wallpaper that is currently running.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunningView {
    /// Monitor number (1, 2…); 0 = spanned across all monitors.
    pub monitor: u32,
    pub wallpaper_id: String,
    /// `media-foundation`, `mpv`, `webview2`, `windows-wallpaper` (images), `app`.
    pub backend: String,
    pub paused: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct EngineState {
    pub running: Vec<RunningView>,
    /// The user pressed pause (kept in memory only).
    pub manual_pause: bool,
    /// The most recent error (shown on the wallpaper page).
    pub error: Option<String>,
    /// Errors per wallpaper (id → reason); cleared once the wallpaper runs successfully.
    pub errors: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    State(EngineState),
    Error { wallpaper_id: Option<String>, message: String },
}
