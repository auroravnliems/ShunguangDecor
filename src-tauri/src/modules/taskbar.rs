//! Tauri commands of the YASB Taskbar part. The `taskbar-status` event is emitted by `TaskbarController` itself.

use crate::state::AppState;
use sg_core::text::I18nText;
use sg_taskbar::{RawState, TaskbarStatus, WidgetCatalogItem};
use std::path::PathBuf;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt as _;

type Res<T> = Result<T, I18nText>;

fn raw_io(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("taskbar.err.raw_io").arg("detail", e)
}

/// The 13 widgets that can be placed on the bar: the name/description text keys follow `taskbar.widget.<kind>.name|desc`.
#[tauri::command]
pub fn taskbar_widget_catalog() -> Vec<WidgetCatalogItem> {
    sg_taskbar::widget_catalog()
}

#[tauri::command]
pub fn taskbar_status(state: State<'_, AppState>) -> TaskbarStatus {
    state.services.taskbar.status()
}

/// `"generated"` | `"edited"` (the user edited by hand or imported a theme: the engine uses their files).
#[tauri::command]
pub fn taskbar_raw_state(state: State<'_, AppState>) -> RawState {
    state.services.taskbar.raw_state()
}

/// Opens the `taskbar-raw` folder (config.yaml + styles.css) with the system's opener.
#[tauri::command]
pub fn taskbar_open_raw(app: AppHandle, state: State<'_, AppState>) -> Res<()> {
    let dir = state.services.taskbar.raw_dir();
    std::fs::create_dir_all(&dir).map_err(raw_io)?;
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(raw_io)
}

/// `keep_user = true`: keep the user's files; `false`: go back to the app's controls (overwriting both files).
#[tauri::command]
pub async fn taskbar_raw_choose(state: State<'_, AppState>, keep_user: bool) -> Res<()> {
    let ctl = state.services.taskbar.clone();
    tauri::async_runtime::spawn_blocking(move || ctl.raw_choose(keep_user)).await.map_err(raw_io)?
}

/// Imports a YASB theme: a folder with `config.yaml` + `styles.css`. Written only if valid; if rejected, the old files are left untouched.
#[tauri::command]
pub async fn taskbar_import_yasb(state: State<'_, AppState>, dir: String) -> Res<()> {
    let ctl = state.services.taskbar.clone();
    tauri::async_runtime::spawn_blocking(move || ctl.import_yasb(&PathBuf::from(dir))).await.map_err(raw_io)?
}

/// System font families (names in the app's language) for the font picker.
#[tauri::command]
pub async fn taskbar_list_fonts(state: State<'_, AppState>) -> Res<Vec<String>> {
    let code = state.lang().code();
    tauri::async_runtime::spawn_blocking(move || sg_taskbar::fonts::list_system_fonts(code)).await.map_err(raw_io)
}

/// Manual testing (debug builds only): simulates an Explorer restart by sending `TaskbarCreated` to our watcher window.
#[tauri::command]
pub fn taskbar_debug_taskbar_created(state: State<'_, AppState>) {
    if cfg!(debug_assertions) {
        state.services.explorer.ensure_watching();
        state.services.explorer.debug_simulate_explorer_restart();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_json_is_camel_case() {
        let v = serde_json::to_value(sg_taskbar::widget_catalog()).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 13);
        assert_eq!(
            v[0],
            serde_json::json!({
                "kind": "windows_button",
                "nameKey": "taskbar.widget.windows_button.name",
                "descKey": "taskbar.widget.windows_button.desc",
                "singleton": true
            })
        );
    }

    #[test]
    fn raw_state_and_status_json() {
        assert_eq!(serde_json::to_value(sg_taskbar::RawState::Generated).unwrap(), "generated");
        assert_eq!(serde_json::to_value(sg_taskbar::RawState::EditedByUser).unwrap(), "edited");
        let s = sg_taskbar::TaskbarStatus {
            enabled: true,
            native_hidden: true,
            raw_state: sg_taskbar::RawState::EditedByUser,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::json!({ "enabled": true, "nativeHidden": true, "rawState": "edited", "error": null })
        );
    }
}
