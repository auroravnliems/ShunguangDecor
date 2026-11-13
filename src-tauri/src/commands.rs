//! Tauri commands for the UI. Every returned error is an `I18nText` so the UI translates it itself.

use crate::state::{AppState, lock};
use crate::window;
use serde::Serialize;
use sg_core::archive::{ArchiveKind, ArchiveWriter, Limits, Manifest, read_archive};
use sg_core::config::{self, write_atomic};
use sg_core::desktop;
use sg_core::i18n::{Lang, t};
use sg_core::ids::ModuleId;
use sg_core::module::ModuleInfo;
use sg_core::profiles::{ProfileMeta, validate_name};
use sg_core::text::I18nText;
use std::path::PathBuf;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;

pub const DISCORD_URL: &str = "https://discord.gg/k55b6F3h2z";
/// Source code repository (the UI hides the link when `None`).
pub const GITHUB_URL: Option<&str> = Some("https://github.com/auroravnliems/ShunguangDecor");
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

type Res<T> = Result<T, I18nText>;

fn io_err(e: impl std::fmt::Display) -> I18nText {
    I18nText::new("err.io").arg("detail", e)
}

fn parse_module(s: &str) -> Res<ModuleId> {
    ModuleId::parse(s).ok_or_else(|| I18nText::new("err.module.unknown").arg("id", s))
}

/// A write path always carries the app's own extension: even if the UI is tricked into passing a strange path, it
/// can only produce `x.dll.sgdecor` and cannot overwrite a file of another type.
fn with_ext(dest: PathBuf, ext: &str) -> PathBuf {
    if dest.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
        return dest;
    }
    let mut s = dest.into_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    protocol_base: &'static str,
    portable: bool,
    github_url: Option<&'static str>,
    discord_url: &'static str,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    language: String,
    autostart: bool,
    reduce_motion: bool,
    follow_os_motion: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStats {
    ram_bytes: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperView {
    name: String,
    url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    files: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    id: ModuleId,
    error: Option<I18nText>,
}

pub fn protocol_base() -> &'static str {
    "http://sg.localhost"
}

#[tauri::command]
pub fn get_app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: VERSION,
        protocol_base: protocol_base(),
        portable: state.paths.portable,
        github_url: GITHUB_URL,
        discord_url: DISCORD_URL,
    }
}

fn settings_of(app: &AppHandle, state: &AppState) -> Settings {
    let c = state.config();
    Settings {
        language: c.language.unwrap_or(Lang::En).code().to_string(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        reduce_motion: c.reduce_motion,
        follow_os_motion: c.follow_os_motion,
    }
}

fn settings_changed(app: &AppHandle, state: &AppState) -> Settings {
    let s = settings_of(app, state);
    window::emit_main(app, "settings-changed", s.clone());
    s
}

#[tauri::command]
pub fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Settings {
    settings_of(&app, &state)
}

#[tauri::command]
pub async fn set_language(app: AppHandle, state: State<'_, AppState>, lang: String) -> Res<Settings> {
    let l = Lang::from_code(&lang).ok_or_else(|| I18nText::new("err.settings.bad_language").arg("code", &lang))?;
    state.update_config(|c| c.language = Some(l));
    if let Some(tray) = state.tray() {
        tray.relabel(l, &state.host.infos());
    }
    Ok(settings_changed(&app, &state))
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Res<Settings> {
    let al = app.autolaunch();
    let r = if enabled { al.enable() } else { al.disable() };
    r.map_err(|e| I18nText::new("err.settings.autostart").arg("detail", e))?;
    state.update_config(|c| c.autostart_initialized = true);
    Ok(settings_changed(&app, &state))
}

#[tauri::command]
pub fn set_reduce_motion(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Settings {
    state.update_config(|c| c.reduce_motion = enabled);
    settings_changed(&app, &state)
}

#[tauri::command]
pub fn set_follow_os_motion(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> Settings {
    state.update_config(|c| c.follow_os_motion = enabled);
    settings_changed(&app, &state)
}

#[tauri::command]
pub fn list_modules(state: State<'_, AppState>) -> Vec<ModuleInfo> {
    state.host.infos()
}

/// Enables/disables a part and remembers the choice (shared by the UI and the tray menu).
pub fn toggle_module(state: &AppState, id: ModuleId, enabled: bool) -> Res<ModuleInfo> {
    let cfg = state.module_config(id);
    match state.host.set_enabled(id, enabled, &cfg) {
        Ok(info) => {
            state.update_config(|c| c.modules_enabled.insert(id, info.enabled));
            Ok(info)
        }
        Err(e) => {
            if enabled {
                state.update_config(|c| c.modules_enabled.insert(id, false));
            }
            Err(e.to_text())
        }
    }
}

#[tauri::command]
pub async fn set_module_enabled(state: State<'_, AppState>, id: String, enabled: bool) -> Res<ModuleInfo> {
    toggle_module(&state, parse_module(&id)?, enabled)
}

/// The configuration currently in use for a part, as JSON for the UI (missing/corrupt file → `{}`).
#[tauri::command]
pub fn module_get_config(state: State<'_, AppState>, module: String) -> Res<serde_json::Value> {
    let id = parse_module(&module)?;
    serde_json::to_value(state.module_config(id)).map_err(io_err)
}

/// Saves a part's configuration and applies it immediately if the part is enabled (there is no "Apply" button).
#[tauri::command]
pub async fn module_set_config(app: AppHandle, state: State<'_, AppState>, module: String, config: serde_json::Value) -> Res<()> {
    let id = parse_module(&module)?;
    let table: toml::Table =
        serde_json::from_value(config).map_err(|e| I18nText::new("err.config.invalid").arg("detail", e))?;
    let text = toml::to_string_pretty(&table).map_err(io_err)?;
    write_atomic(&state.paths.module_config_file(id), text.as_bytes()).map_err(io_err)?;
    if state.host.info(id).enabled {
        state.host.apply(id, &table).map_err(|e| e.to_text())?;
        live_changed(&app, id);
    }
    Ok(())
}

/// The Wallpaper part just applied a new configuration (fill mode, arrangement…): the preview pane re-queries the live playback source.
/// A dedicated event, not `module-config-changed` (the UI would reload the configuration and could overwrite a change still waiting to be saved).
fn live_changed(app: &AppHandle, id: ModuleId) {
    if id == ModuleId::Wallpaper {
        window::emit_main(app, "capture-live-changed", ());
    }
}

/// Tells the UI to re-read a part's configuration (after loading a preset, importing a file…).
pub fn config_changed(app: &AppHandle, id: ModuleId) {
    window::emit_main(app, "module-config-changed", id);
}

#[tauri::command]
pub fn get_runtime_stats() -> RuntimeStats {
    RuntimeStats { ram_bytes: desktop::process_memory_bytes() }
}

#[tauri::command]
pub fn get_current_wallpaper(state: State<'_, AppState>) -> Option<WallpaperView> {
    // If a live wallpaper is running on the primary monitor, use it (its thumbnail); otherwise the static wallpaper as before.
    if let Some((name, url)) = crate::modules::wallpaper::running_main_thumb(&state) {
        return Some(WallpaperView { name, url });
    }
    desktop::current_wallpaper().map(|w| WallpaperView {
        name: w.name,
        url: format!("{}/wallpaper/current?v={}", protocol_base(), w.modified_unix),
    })
}

// ---------- Presets ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileListView {
    profiles: Vec<ProfileMeta>,
    /// The currently selected preset (if its file still exists).
    active: Option<String>,
}

#[tauri::command]
pub fn profile_list(state: State<'_, AppState>, module: String) -> Res<ProfileListView> {
    let id = parse_module(&module)?;
    let profiles = state.profiles.list(id);
    let active = state.config().active_profiles.get(&id).filter(|a| profiles.iter().any(|p| &p.name == *a)).cloned();
    Ok(ProfileListView { profiles, active })
}

#[tauri::command]
pub async fn profile_save(state: State<'_, AppState>, module: String, name: String, overwrite: bool) -> Res<()> {
    let id = parse_module(&module)?;
    let clean = validate_name(&name)?;
    state.profiles.save(id, &clean, &state.module_config_text(id), overwrite)?;
    state.update_config(|c| c.active_profiles.insert(id, clean));
    Ok(())
}

#[tauri::command]
pub async fn profile_load(app: AppHandle, state: State<'_, AppState>, module: String, name: String) -> Res<()> {
    let id = parse_module(&module)?;
    let text = state.profiles.load(id, &name)?;
    let table: toml::Table = text.parse().map_err(|e| I18nText::new("err.profile.bad_toml").arg("detail", e))?;
    write_atomic(&state.paths.module_config_file(id), text.as_bytes()).map_err(io_err)?;
    state.update_config(|c| c.active_profiles.insert(id, name.trim().to_string()));
    config_changed(&app, id);
    if state.host.info(id).enabled {
        state.host.apply(id, &table).map_err(|e| e.to_text())?;
        live_changed(&app, id);
    }
    Ok(())
}

#[tauri::command]
pub async fn profile_delete(state: State<'_, AppState>, module: String, name: String) -> Res<()> {
    let id = parse_module(&module)?;
    state.profiles.delete(id, &name)?;
    state.update_config(|c| {
        if c.active_profiles.get(&id).is_some_and(|a| a == name.trim()) {
            c.active_profiles.remove(&id);
        }
    });
    Ok(())
}

fn manifest(kind: ArchiveKind, module: Option<ModuleId>, profile_name: Option<String>) -> Manifest {
    Manifest {
        format: sg_core::archive::FORMAT_VERSION,
        app_version: VERSION.to_string(),
        kind,
        module,
        profile_name,
        created_unix: now_unix(),
    }
}

/// A Wallpaper preset carries the library folders of the wallpapers in use (over the limit, the assets are dropped).
fn wallpaper_assets_into(state: &AppState, w: &mut ArchiveWriter, config_text: &str) -> std::io::Result<()> {
    let paths = state.services.wallpaper.paths();
    let assets = sg_wallpaper::assets::collect_export_assets(paths, config_text, &Limits::default());
    for warn in &assets.warnings {
        tracing::warn!(target: "sg::wallpaper", "export without assets: {warn}");
    }
    sg_wallpaper::assets::write_export_assets(w, &assets)
}

#[tauri::command]
pub async fn profile_export(state: State<'_, AppState>, module: String, name: String, dest: PathBuf) -> Res<()> {
    let dest = with_ext(dest, "sgdecor");
    let id = parse_module(&module)?;
    let text = state.profiles.load(id, &name)?;
    let mut w = ArchiveWriter::create(&dest).map_err(io_err)?;
    w.add_bytes(&format!("modules/{id}/config.toml"), text.as_bytes()).map_err(io_err)?;
    if id == ModuleId::Wallpaper {
        wallpaper_assets_into(&state, &mut w, &text).map_err(io_err)?;
    }
    w.finish(&manifest(ArchiveKind::Module, Some(id), Some(name.trim().to_string()))).map_err(io_err)
}

#[tauri::command]
pub async fn profile_import(state: State<'_, AppState>, module: String, src: PathBuf) -> Res<String> {
    let id = parse_module(&module)?;
    let staging = sg_core::transfer::staging_dir(&state.paths);
    let staged = read_archive(&src, &staging, Limits::default())?;
    let result = (|| {
        let m = &staged.manifest;
        if m.kind != ArchiveKind::Module {
            return Err(I18nText::new("err.archive.is_full_export"));
        }
        if m.module != Some(id) {
            let other = m.module.map(|o| t(state.lang(), &format!("module.{o}.name"), &[])).unwrap_or_default();
            return Err(I18nText::new("err.archive.wrong_module").arg("module", other));
        }
        let text = std::fs::read_to_string(staged.root.join(format!("modules/{id}/config.toml")))
            .map_err(|_| I18nText::new("err.archive.no_config"))?;
        let name = state.profiles.unique_name(id, m.profile_name.as_deref().unwrap_or("Imported"));
        state.profiles.save(id, &name, config::strip_bom(&text), false)?;
        if id == ModuleId::Wallpaper {
            let mut imp = sg_wallpaper::assets::AssetImporter::new(state.services.wallpaper.paths());
            for rel in staged.files.iter().filter(|f| f.starts_with("modules/wallpaper/assets/")) {
                imp.accept(rel, &staged.root.join(rel)).map_err(io_err)?;
            }
        }
        Ok(name)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[tauri::command]
pub async fn export_all(state: State<'_, AppState>, dest: PathBuf) -> Res<()> {
    let dest = with_ext(dest, "sgdecor");
    let paths = state.services.wallpaper.paths().clone();
    let cfg_text = state.module_config_text(ModuleId::Wallpaper);
    sg_core::transfer::export_all_with(&state.paths, &state.config(), &dest, VERSION, |w| {
        let assets = sg_wallpaper::assets::collect_export_assets(&paths, &cfg_text, &Limits::default());
        for warn in &assets.warnings {
            tracing::warn!(target: "sg::wallpaper", "export without assets: {warn}");
        }
        sg_wallpaper::assets::write_export_assets(w, &assets)
    })
    .map_err(io_err)
}

#[tauri::command]
pub async fn import_all(app: AppHandle, state: State<'_, AppState>, src: PathBuf) -> Res<ImportSummary> {
    let mut importer = sg_wallpaper::assets::AssetImporter::new(state.services.wallpaper.paths());
    let out = sg_core::transfer::import_all_with(&state.paths, &src, now_unix(), |rel, from| importer.accept(rel, from))?;
    tracing::info!(target: "sg::app", "imported {} files, backup at {}", out.files, out.backup.display());
    if let Some(inc) = out.settings {
        state.update_config(|c| {
            if inc.language.is_some() {
                c.language = inc.language;
            }
            c.reduce_motion = inc.reduce_motion;
            c.follow_os_motion = inc.follow_os_motion;
            c.modules_enabled = inc.modules_enabled;
        });
    }
    // Re-apply the enabled/disabled state according to the configuration just imported.
    let wanted = state.config().modules_enabled;
    for id in ModuleId::ALL {
        let want = wanted.get(&id).copied().unwrap_or(false);
        if state.host.info(id).enabled != want {
            let _ = toggle_module(&state, id, want);
        }
    }
    if let Some(tray) = state.tray() {
        tray.relabel(state.lang(), &state.host.infos());
    }
    for id in ModuleId::ALL {
        config_changed(&app, id);
    }
    settings_changed(&app, &state);
    Ok(ImportSummary { files: out.files })
}

#[tauri::command]
pub async fn restore_all_os_defaults(state: State<'_, AppState>) -> Res<Vec<RestoreResult>> {
    let results = state.host.restore_all();
    state.update_config(|c| {
        for id in ModuleId::ALL {
            c.modules_enabled.insert(id, false);
        }
    });
    Ok(results
        .into_iter()
        .map(|(id, r)| RestoreResult { id, error: r.err().map(|e| e.to_text()) })
        .collect())
}

#[tauri::command]
pub fn open_data_dir(app: AppHandle, state: State<'_, AppState>) -> Res<()> {
    let dir = state.paths.roaming.clone();
    std::fs::create_dir_all(&dir).map_err(io_err)?;
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(io_err)
}

#[tauri::command]
pub fn open_link(app: AppHandle, kind: String) -> Res<()> {
    let url = match kind.as_str() {
        "discord" => DISCORD_URL,
        "github" => GITHUB_URL.ok_or_else(|| I18nText::new("err.link.unavailable"))?,
        _ => return Err(I18nText::new("err.link.unavailable")),
    };
    app.opener().open_url(url, None::<&str>).map_err(io_err)
}

#[tauri::command]
pub async fn export_bug_report(state: State<'_, AppState>, dest: PathBuf) -> Res<()> {
    let dest = with_ext(dest, "zip");
    let info = sg_core::sysinfo::collect(VERSION, tauri::webview_version().ok(), state.paths.portable);
    sg_core::report::write_bug_report(&dest, &state.paths, &info, dirs::home_dir().as_deref()).map_err(io_err)
}

#[tauri::command]
pub fn take_pending_intent(state: State<'_, AppState>) -> Option<String> {
    lock(&state.pending_intent).take()
}

#[tauri::command]
pub fn ack_tray_notice(state: State<'_, AppState>) {
    state.update_config(|c| c.tray_notice_shown = true);
}

#[tauri::command]
pub fn log_frontend(level: String, message: String) {
    let msg: String = message.chars().take(4000).collect();
    match level.as_str() {
        "error" => tracing::error!(target: "sg::ui", "{msg}"),
        "warn" => tracing::warn!(target: "sg::ui", "{msg}"),
        _ => tracing::info!(target: "sg::ui", "{msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::with_ext;
    use std::path::PathBuf;

    #[test]
    fn with_ext_adds_missing_extension_only() {
        assert_eq!(with_ext(PathBuf::from("a/b.sgdecor"), "sgdecor"), PathBuf::from("a/b.sgdecor"));
        assert_eq!(with_ext(PathBuf::from("a/b.SGDECOR"), "sgdecor"), PathBuf::from("a/b.SGDECOR"));
        assert_eq!(with_ext(PathBuf::from("a/b"), "zip"), PathBuf::from("a/b.zip"));
        assert_eq!(with_ext(PathBuf::from("a/x.dll"), "zip"), PathBuf::from("a/x.dll.zip"));
    }
}
