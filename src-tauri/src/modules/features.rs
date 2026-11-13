//! Tauri commands of the Hidden features part. Read commands need no admin; write commands go through the elevated helper process
//! (asking UAC itself if not yet held). Commands that can block (UAC, pipe) all run on their own thread, never occupying the UI thread.

use crate::state::AppState;
use sg_core::config::write_atomic;
use sg_core::ids::ModuleId;
use sg_core::text::I18nText;
use sg_features::{
    EnabledState, FeaturePage, FeatureRow, ImportReport, ListQuery, Priority, Store, Subscription, SuggestionRow, UndoEntry, UsageReport,
};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt as _;

type Res<T> = Result<T, I18nText>;

/// Import files larger than this are rejected (a real export file is only a few hundred KB).
const MAX_IMPORT_BYTES: u64 = 8 * 1024 * 1024;

/// A subscription row sent to the UI. `reporting_target` is a u64 (it can exceed 2^53), so it travels as a hex string so JS does not
/// round it. The `Subscription` type is shared with the helper-process protocol, so its serde is NOT changed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SubscriptionRow {
    pub id: u32,
    pub reporting_kind: u16,
    pub reporting_options: u16,
    pub reporting_target: String,
}

impl From<&Subscription> for SubscriptionRow {
    fn from(s: &Subscription) -> Self {
        Self {
            id: s.id,
            reporting_kind: s.reporting_kind,
            reporting_options: s.reporting_options,
            reporting_target: format!("0x{:016X}", s.reporting_target),
        }
    }
}

fn join_err(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("features.err.io").arg("detail", e)
}

fn io_err(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("features.err.io").arg("detail", e)
}

/// `ModuleHost::info()` is a cached copy: admin rights / the changed-configuration count are only refreshed when `refresh` is called.
/// Any command that can change those two must call this function (even on error). A refresh error is only logged and does not fail the command.
fn refresh_info(state: &AppState) {
    if let Err(e) = state.host.refresh(ModuleId::Features) {
        tracing::warn!(target: "sg::app", "features refresh failed: {}", e.to_text().key);
    }
}

#[tauri::command]
pub async fn features_search(state: State<'_, AppState>, query: String, limit: usize) -> Res<Vec<FeatureRow>> {
    let svc = state.services.features.clone();
    tauri::async_runtime::spawn_blocking(move || svc.search(&query, limit.min(200))).await.map_err(join_err)
}

/// One page of the list of all features (the "All" tab). An ntdll read error is returned to the UI; an incomplete list is never returned.
#[tauri::command]
pub async fn features_list(state: State<'_, AppState>, query: ListQuery) -> Res<FeaturePage<FeatureRow>> {
    let svc = state.services.features.clone();
    tauri::async_runtime::spawn_blocking(move || svc.list(&query)).await.map_err(join_err)?
}

/// Opens the browser to search the web by ID. The UI only sends the numeric ID; the URL is built by Rust (like `open_link`: no URL is accepted from outside).
#[tauri::command]
pub fn features_web_search(app: AppHandle, state: State<'_, AppState>, id: u32) -> Res<()> {
    let url = state.services.features.web_search_url(id);
    app.opener().open_url(url, None::<&str>).map_err(io_err)
}

#[tauri::command]
pub async fn features_get(state: State<'_, AppState>, id: u32) -> Res<FeatureRow> {
    let svc = state.services.features.clone();
    tauri::async_runtime::spawn_blocking(move || svc.get(id)).await.map_err(join_err)
}

#[tauri::command]
pub async fn features_suggestions(state: State<'_, AppState>) -> Res<Vec<SuggestionRow>> {
    let svc = state.services.features.clone();
    tauri::async_runtime::spawn_blocking(move || svc.suggestions()).await.map_err(join_err)
}

#[tauri::command]
pub async fn features_subscriptions(state: State<'_, AppState>) -> Res<Vec<SubscriptionRow>> {
    let svc = state.services.features.clone();
    let subs = tauri::async_runtime::spawn_blocking(move || svc.subscriptions()).await.map_err(join_err)?;
    Ok(subs.iter().map(SubscriptionRow::from).collect())
}

#[tauri::command]
pub fn features_undo_list(state: State<'_, AppState>) -> Vec<UndoEntry> {
    state.services.features.undo_list()
}

/// Writes the current configuration to a JSON file (the `.json` extension is forced); returns the real path written.
#[tauri::command]
pub async fn features_export(state: State<'_, AppState>, dest: PathBuf) -> Res<String> {
    let svc = state.services.features.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = force_json_ext(&dest);
        write_atomic(&path, svc.export_json()?.as_bytes()).map_err(io_err)?;
        Ok::<_, I18nText>(path.display().to_string())
    })
    .await
    .map_err(join_err)?
}

fn force_json_ext(p: &Path) -> PathBuf {
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
        return p.to_path_buf();
    }
    let mut s = p.as_os_str().to_owned();
    s.push(".json");
    PathBuf::from(s)
}

/// Requests admin rights (UAC dialog). `false` = the user clicked "No".
#[tauri::command]
pub async fn features_elevate(state: State<'_, AppState>) -> Res<bool> {
    let svc = state.services.features.clone();
    let r = tauri::async_runtime::spawn_blocking(move || svc.elevate()).await;
    refresh_info(&state);
    r.map_err(join_err)?
}

#[tauri::command]
pub async fn features_set(
    app: State<'_, AppState>,
    id: u32,
    store: Store,
    state: EnabledState,
    variant: u8,
    priority: Priority,
) -> Res<()> {
    let svc = app.services.features.clone();
    let r = tauri::async_runtime::spawn_blocking(move || svc.set(id, store, state, variant, priority)).await;
    refresh_info(&app);
    r.map_err(join_err)?
}

#[tauri::command]
pub async fn features_reset(state: State<'_, AppState>, id: u32, store: Store, priority: Priority) -> Res<()> {
    let svc = state.services.features.clone();
    let r = tauri::async_runtime::spawn_blocking(move || svc.reset(id, store, priority)).await;
    refresh_info(&state);
    r.map_err(join_err)?
}

/// Import from a file: the app reads the file and sends the DATA to the helper process (never the path).
#[tauri::command]
pub async fn features_import(state: State<'_, AppState>, src: PathBuf) -> Res<ImportReport> {
    let svc = state.services.features.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        let len = std::fs::metadata(&src).map_err(io_err)?.len();
        if len > MAX_IMPORT_BYTES {
            return Err(I18nText::new("features.err.file_too_big"));
        }
        let text = std::fs::read_to_string(&src).map_err(io_err)?;
        svc.import_text(&text)
    })
    .await;
    refresh_info(&state);
    r.map_err(join_err)?
}

#[tauri::command]
pub async fn features_undo_all(state: State<'_, AppState>) -> Res<u32> {
    let svc = state.services.features.clone();
    let r = tauri::async_runtime::spawn_blocking(move || svc.undo_all()).await;
    refresh_info(&state);
    r.map_err(join_err)?
}

/// ViVeTool's `/fixpriority`: moves Service-tier configurations to User. Returns the number of configurations moved (0 = nothing needed).
#[tauri::command]
pub async fn features_fix_priority(state: State<'_, AppState>) -> Res<u32> {
    let svc = state.services.features.clone();
    let r = tauri::async_runtime::spawn_blocking(move || svc.fix_priority()).await;
    refresh_info(&state);
    r.map_err(join_err)?
}

/// Reports one use of a feature to Windows (`RtlNotifyFeatureUsage`); does not change configuration.
#[tauri::command]
pub async fn features_notify_usage(state: State<'_, AppState>, report: UsageReport) -> Res<()> {
    let svc = state.services.features.clone();
    // A failed call drops the client, so `is_elevated()` may change: refresh even on error.
    let r = tauri::async_runtime::spawn_blocking(move || svc.notify_usage(report)).await;
    refresh_info(&state);
    r.map_err(join_err)?
}

/// Closes the helper process when leaving the page (undoes nothing).
#[tauri::command]
pub async fn features_release(state: State<'_, AppState>) -> Res<()> {
    let svc = state.services.features.clone();
    let r = tauri::async_runtime::spawn_blocking(move || svc.release()).await;
    refresh_info(&state);
    r.map_err(join_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_target_is_sent_as_hex_string() {
        let s = Subscription { id: 7, reporting_kind: 3, reporting_options: 1, reporting_target: 1_119_440_463_460_059_321 };
        let row = SubscriptionRow::from(&s);
        assert_eq!(row.reporting_target, "0x0F890D2BA3BC48B9");
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["reporting_target"], "0x0F890D2BA3BC48B9");
        assert_eq!(json["id"], 7);
    }

    #[test]
    fn export_extension_is_forced_to_json() {
        assert_eq!(force_json_ext(Path::new("a/b")), PathBuf::from("a/b.json"));
        assert_eq!(force_json_ext(Path::new("a/b.txt")), PathBuf::from("a/b.txt.json"));
        assert_eq!(force_json_ext(Path::new("a/b.JSON")), PathBuf::from("a/b.JSON"));
    }
}
