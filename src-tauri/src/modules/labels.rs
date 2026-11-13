//! Tauri commands of the Desktop labels part. The `labels-status` event is emitted by `LabelsService` itself whenever the status changes.

use crate::state::AppState;
use sg_core::text::I18nText;
use sg_labels::LabelsStatus;
use tauri::State;

type Res<T> = Result<T, I18nText>;

#[tauri::command]
pub fn labels_status(state: State<'_, AppState>) -> LabelsStatus {
    state.services.labels.status()
}

/// Only when the user pressed "Restart Explorer" and confirmed (every File Explorer window will close).
#[tauri::command]
pub async fn explorer_restart(state: State<'_, AppState>) -> Res<()> {
    let svc = state.services.labels.clone();
    tauri::async_runtime::spawn_blocking(move || svc.restart_explorer())
        .await
        .map_err(|e| I18nText::new("labels.err.restart").arg("detail", e))?
}

/// Manual testing (debug builds only): simulates an Explorer restart by sending `TaskbarCreated` to our watcher window.
#[tauri::command]
pub fn labels_debug_taskbar_created(state: State<'_, AppState>) {
    if cfg!(debug_assertions) {
        state.services.explorer.debug_simulate_explorer_restart();
    }
}

#[cfg(test)]
mod tests {
    use sg_core::text::I18nText;
    use sg_labels::{LabelsStatus, Method};

    #[test]
    fn labels_status_json_camel_case() {
        let s = LabelsStatus {
            method: Method::Dll,
            mode: "custom",
            explorer_pid: None,
            items: 0,
            error: None,
            needs_explorer_restart: false,
            source: "none",
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, serde_json::json!({
            "method": "dll", "mode": "custom", "explorerPid": null, "items": 0,
            "error": null, "needsExplorerRestart": false, "source": "none"
        }));
        let f = LabelsStatus { method: Method::Fallback, error: Some(I18nText::new("labels.reason.blocked")), ..s };
        assert_eq!(serde_json::to_value(&f).unwrap()["method"], "fallback");
    }
}
