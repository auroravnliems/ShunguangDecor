//! Tauri commands of the Cursor part and preview images via `sg://`.

use crate::state::AppState;
use sg_core::text::I18nText;
use sg_cursor::{Role, SchemeDetail, SchemeSummary};
use std::path::PathBuf;
use tauri::State;

type Res<T> = Result<T, I18nText>;

fn parse_role(s: &str) -> Res<Role> {
    Role::parse(s).ok_or_else(|| I18nText::new("cursor.err.role_unknown"))
}

#[tauri::command]
pub fn cursor_list(state: State<'_, AppState>) -> Vec<SchemeSummary> {
    state.services.cursor.library().list()
}

#[tauri::command]
pub async fn cursor_detail(state: State<'_, AppState>, id: String) -> Res<SchemeDetail> {
    let svc = state.services.cursor.clone();
    tauri::async_runtime::spawn_blocking(move || svc.library().get(&id))
        .await
        .map_err(|e| I18nText::new("cursor.err.io").arg("detail", e))?
}

/// Import can take a few seconds (extraction + checking each file): runs off the UI thread.
#[tauri::command]
pub async fn cursor_import(state: State<'_, AppState>, path: PathBuf) -> Res<Vec<String>> {
    let svc = state.services.cursor.clone();
    tauri::async_runtime::spawn_blocking(move || svc.import(&path))
        .await
        .map_err(|e| I18nText::new("cursor.err.io").arg("detail", e))?
}

#[tauri::command]
pub fn cursor_rename(state: State<'_, AppState>, id: String, name: String) -> Res<()> {
    state.services.cursor.rename(&id, &name)
}

#[tauri::command]
pub fn cursor_delete(state: State<'_, AppState>, id: String) -> Res<()> {
    state.services.cursor.delete(&id)
}

#[tauri::command]
pub async fn cursor_set_role(state: State<'_, AppState>, id: String, role: String, file: Option<String>) -> Res<()> {
    let role = parse_role(&role)?;
    let svc = state.services.cursor.clone();
    tauri::async_runtime::spawn_blocking(move || svc.set_role(&id, role, file))
        .await
        .map_err(|e| I18nText::new("cursor.err.io").arg("detail", e))?
}

/// Preview image: the whole frame strip or a single frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Sheet,
    Index(usize),
}

/// `/cursor/<id>/<Role>/sheet.png` or `/cursor/<id>/<Role>/<n>.png`. Unknown id/role → `None`.
pub fn parse_route(path: &str) -> Option<(String, Role, Frame)> {
    let mut parts = path.strip_prefix("/cursor/")?.split('/');
    let (id, role, file) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !sg_cursor::library::valid_id(id) {
        return None;
    }
    let role = Role::parse(role)?;
    let stem = file.strip_suffix(".png")?;
    let frame = if stem == "sheet" { Frame::Sheet } else { Frame::Index(stem.parse().ok()?) };
    Some((id.to_string(), role, frame))
}

/// PNG bytes for an `sg://` path; `None` → 404.
pub fn serve(state: &AppState, path: &str) -> Option<Vec<u8>> {
    let (id, role, frame) = parse_route(path)?;
    let anim = state.services.cursor.library().read_frames(&id, role).ok()?;
    match frame {
        Frame::Sheet => sg_cursor::preview::sheet_png(&anim),
        Frame::Index(n) => sg_cursor::preview::frame_png(anim.frames.get(n)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_parsing() {
        assert_eq!(parse_route("/cursor/abc-1/Hand/sheet.png"), Some(("abc-1".into(), Role::Hand, Frame::Sheet)));
        assert_eq!(parse_route("/cursor/abc-1/ibeam/3.png"), Some(("abc-1".into(), Role::IBeam, Frame::Index(3))));
        assert_eq!(parse_route("/cursor/../x/Hand/0.png"), None);
        assert_eq!(parse_route("/cursor/abc/Nope/0.png"), None);
        assert_eq!(parse_route("/cursor/ABC/Hand/0.png"), None);
        assert_eq!(parse_route("/cursor/abc/Hand/0.jpg"), None);
        assert_eq!(parse_route("/cursor/abc/Hand/-1.png"), None);
        assert_eq!(parse_route("/cursor/abc/Hand/0.png/x"), None);
        assert_eq!(parse_route("/wallpaper/current"), None);
    }
}
