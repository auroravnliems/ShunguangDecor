// Typed wrappers for each Tauri command (src-tauri/src/commands.rs).
// When run in a browser (`npm run dev`, no Tauri), it automatically uses the sample data in mock.ts so the UI can be viewed.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import type { Role, SchemeDetail, SchemeSummary } from "./cursor";
import type { EngineState, PropertiesView, WallpaperItem } from "./wallpaper";
import { mockInvoke } from "./mock";
import type {
  AppInfo,
  CaptureMode,
  FeatureImportReport,
  FeaturePriority,
  FeatureUsageReport,
  FeatureListQuery,
  FeaturePage,
  FeatureRow,
  FeatureState,
  FeatureStore,
  FeatureSubscription,
  I18nText,
  ImportSummary,
  LabelsStatus,
  LiveSource,
  MonitorView,
  ModuleId,
  ModuleInfo,
  ProfileList,
  RestoreResult,
  RuntimeStats,
  Settings,
  SuggestionRow,
  TaskbarRawState,
  TaskbarStatus,
  UndoEntry,
  WallpaperView,
  WidgetCatalogItem,
} from "./types";

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return isTauri() ? invoke<T>(cmd, args) : (mockInvoke(cmd, args) as Promise<T>);
}

export const getAppInfo = () => call<AppInfo>("get_app_info");
export const getSettings = () => call<Settings>("get_settings");
export const setLanguage = (lang: string) => call<Settings>("set_language", { lang });
export const setAutostart = (enabled: boolean) => call<Settings>("set_autostart", { enabled });
export const setReduceMotion = (enabled: boolean) => call<Settings>("set_reduce_motion", { enabled });
export const setFollowOsMotion = (enabled: boolean) => call<Settings>("set_follow_os_motion", { enabled });
export const listModules = () => call<ModuleInfo[]>("list_modules");
export const setModuleEnabled = (id: ModuleId, enabled: boolean) => call<ModuleInfo>("set_module_enabled", { id, enabled });
/** The active config of a module (`config/<module>.toml`, as JSON). */
export const getModuleConfig = (module: ModuleId) => call<Record<string, unknown>>("module_get_config", { module });
/** Saves and applies immediately (if the module is enabled). */
export const setModuleConfig = (module: ModuleId, config: Record<string, unknown>) =>
  call<void>("module_set_config", { module, config });
export const getRuntimeStats = () => call<RuntimeStats>("get_runtime_stats");
export const getCurrentWallpaper = () => call<WallpaperView | null>("get_current_wallpaper");

export const profileList = (module: ModuleId) => call<ProfileList>("profile_list", { module });
export const profileSave = (module: ModuleId, name: string, overwrite: boolean) =>
  call<void>("profile_save", { module, name, overwrite });
export const profileLoad = (module: ModuleId, name: string) => call<void>("profile_load", { module, name });
export const profileDelete = (module: ModuleId, name: string) => call<void>("profile_delete", { module, name });
export const profileExport = (module: ModuleId, name: string, dest: string) =>
  call<void>("profile_export", { module, name, dest });
export const profileImport = (module: ModuleId, src: string) => call<string>("profile_import", { module, src });

export const exportAll = (dest: string) => call<void>("export_all", { dest });
export const importAll = (src: string) => call<ImportSummary>("import_all", { src });
export const restoreAllOsDefaults = () => call<RestoreResult[]>("restore_all_os_defaults");
export const openDataDir = () => call<void>("open_data_dir");
export const openLink = (kind: "discord" | "github") => call<void>("open_link", { kind });
export const exportBugReport = (dest: string) => call<void>("export_bug_report", { dest });
export const takePendingIntent = () => call<string | null>("take_pending_intent");
export const ackTrayNotice = () => call<void>("ack_tray_notice");
export const logFrontend = (level: "info" | "warn" | "error", message: string) =>
  call<void>("log_frontend", { level, message }).catch(() => {});

// Captures the real desktop for the preview frame (src-tauri/src/modules/capture.rs).
export const listMonitors = () => call<MonitorView[]>("list_monitors");
// Each command carries an increasing sequence number: IPC may reach the core out of order, and the core drops commands older than one already received.
let captureCall = 0;
/** `mode` "icons": the icon layer only (`sg://capture/<n>-icons.png`); default "full" (`sg://capture/<n>.jpg`). */
export const captureStart = (viewer: number, monitor: number, mode: CaptureMode = "full") =>
  call<void>("capture_start", { viewer, monitor, seq: ++captureCall, mode });
/** The video/GIF wallpaper currently running on that display (only when arranged per display); `null` = use the captured frame as before.
 *  Call again when `onWallpaperState` or `onModuleConfigChanged("wallpaper")` fires. */
export const captureLiveSource = (monitor: number) => call<LiveSource | null>("capture_live_source", { monitor });
export const captureStop = (viewer: number) => call<void>("capture_stop", { viewer, seq: ++captureCall });

// Wallpaper module (src-tauri/src/modules/wallpaper.rs).
export const wallpaperLibrary = () => call<WallpaperItem[]>("wallpaper_library");
export const wallpaperAdd = (source: string, kind?: string) => call<WallpaperItem>("wallpaper_add", { source, kind: kind ?? null });
export const wallpaperImport = (path: string) => call<WallpaperItem[]>("wallpaper_import", { path });
export const wallpaperDelete = (id: string) => call<void>("wallpaper_delete", { id });
export const wallpaperRename = (id: string, title: string) => call<void>("wallpaper_rename", { id, title });
export const wallpaperState = () => call<EngineState>("wallpaper_state");
export const wallpaperTogglePause = () => call<boolean>("wallpaper_toggle_pause");
export const wallpaperProperties = (id: string) => call<PropertiesView | null>("wallpaper_properties", { id });
export const wallpaperSetProperty = (id: string, name: string, value: unknown) =>
  call<void>("wallpaper_set_property", { id, name, value });
export const wallpaperResetProperties = (id: string) => call<void>("wallpaper_reset_properties", { id });
export const wallpaperFindMpv = () => call<string | null>("wallpaper_find_mpv");
export const wallpaperDebugTaskbarCreated = () => call<void>("wallpaper_debug_taskbar_created");

// Cursor module (src-tauri/src/modules/cursor.rs).
export const cursorList = () => call<SchemeSummary[]>("cursor_list");
export const cursorDetail = (id: string) => call<SchemeDetail>("cursor_detail", { id });
export const cursorImport = (path: string) => call<string[]>("cursor_import", { path });
export const cursorRename = (id: string, name: string) => call<void>("cursor_rename", { id, name });
export const cursorDelete = (id: string) => call<void>("cursor_delete", { id });
export const cursorSetRole = (id: string, role: Role, file: string | null) => call<void>("cursor_set_role", { id, role, file });

// Desktop labels module (src-tauri/src/modules/labels.rs).
export const labelsStatus = () => call<LabelsStatus>("labels_status");
/** Call only after the user confirms: every File Explorer window will close. */
export const explorerRestart = () => call<void>("explorer_restart");
export const labelsDebugTaskbarCreated = () => call<void>("labels_debug_taskbar_created");

// Hidden features module (src-tauri/src/modules/features.rs). Reading: no admin needed. Writing: through a helper process (UAC prompt).
export const featuresSearch = (query: string, limit: number) => call<FeatureRow[]>("features_search", { query, limit });
export const featuresList = (query: FeatureListQuery) => call<FeaturePage>("features_list", { query });
/** Sends only the numeric ID: the web search URL is built by Rust, the UI does not pass URLs. */
export const featuresWebSearch = (id: number) => call<void>("features_web_search", { id });
export const featuresGet = (id: number) => call<FeatureRow>("features_get", { id });
export const featuresSuggestions = () => call<SuggestionRow[]>("features_suggestions");
export const featuresSubscriptions = () => call<FeatureSubscription[]>("features_subscriptions");
export const featuresUndoList = () => call<UndoEntry[]>("features_undo_list");
/** Returns the real path that was written (the `.json` extension is enforced). */
export const featuresExport = (dest: string) => call<string>("features_export", { dest });
/** UAC prompt; `false` = the user clicked "No". Call again to ask for permission once more. */
export const featuresElevate = () => call<boolean>("features_elevate");
/** Call only after the session's warning dialog. Writes the undo journal before changing anything. */
export const featuresSet = (id: number, store: FeatureStore, state: FeatureState, variant: number, priority: FeaturePriority) =>
  call<void>("features_set", { id, store, state, variant, priority });
/** Deletes the configuration at exactly the given `priority` level (does not delete other levels, including the Windows Update Service level). */
export const featuresReset = (id: number, store: FeatureStore, priority: FeaturePriority) =>
  call<void>("features_reset", { id, store, priority });
export const featuresImport = (src: string) => call<FeatureImportReport>("features_import", { src });
/** Returns the number of configurations that were undone. */
export const featuresUndoAll = () => call<number>("features_undo_all");
/** `/fixpriority`: moves Service-level configurations to User (with an undo journal entry). Returns the number of configurations moved. */
export const featuresFixPriority = () => call<number>("features_fix_priority");
/** Reports feature usage to Windows; does not change configuration. */
export const featuresNotifyUsage = (report: FeatureUsageReport) => call<void>("features_notify_usage", { report });
/** Closes the helper process when leaving the page (undoes nothing). */
export const featuresRelease = () => call<void>("features_release");

// YASB Taskbar module (src-tauri/src/modules/taskbar.rs).
export const taskbarWidgetCatalog = () => call<WidgetCatalogItem[]>("taskbar_widget_catalog");
export const taskbarStatus = () => call<TaskbarStatus>("taskbar_status");
export const taskbarRawState = () => call<TaskbarRawState>("taskbar_raw_state");
/** Opens the folder containing the bar's config.yaml + styles.css. */
export const taskbarOpenRaw = () => call<void>("taskbar_open_raw");
/** `keepUser = true`: keep my files; `false`: go back to the app's controls (overwrites both files). */
export const taskbarRawChoose = (keepUser: boolean) => call<void>("taskbar_raw_choose", { keepUser });
/** Imports a YASB theme from a folder containing config.yaml + styles.css; invalid files are rejected and the old files are kept. */
export const taskbarImportYasb = (dir: string) => call<void>("taskbar_import_yasb", { dir });
/** The system's font families (sorted, deduplicated). */
export const taskbarListFonts = () => call<string[]>("taskbar_list_fonts");
export const taskbarDebugTaskbarCreated = () => call<void>("taskbar_debug_taskbar_created");

function on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (!isTauri()) return Promise.resolve(() => {});
  return listen<T>(event, (e) => cb(e.payload));
}

export const onModuleChanged = (cb: (m: ModuleInfo) => void) => on<ModuleInfo>("module-changed", cb);
export const onModuleFaulted = (cb: (f: { id: ModuleId; message: string }) => void) => on("module-faulted", cb);
export const onSettingsChanged = (cb: (s: Settings) => void) => on<Settings>("settings-changed", cb);
export const onModuleConfigChanged = (cb: (id: ModuleId) => void) => on<ModuleId>("module-config-changed", cb);
export const onCursorLibraryChanged = (cb: () => void) => on<null>("cursor-library-changed", () => cb());
/** `icons` = the icon-layer frame (PNG), otherwise the full frame (JPEG). */
export const onCaptureFrame = (cb: (f: { monitor: number; seq: number; icons: boolean }) => void) =>
  on<{ monitor: number; seq: number; icons: boolean }>("capture-frame", cb);
/** The Wallpaper module just applied a config (fill mode changed…): the preview frame should query `captureLiveSource` again. */
export const onCaptureLiveChanged = (cb: () => void) => on<null>("capture-live-changed", () => cb());
export const onWallpaperState = (cb: (s: EngineState) => void) => on<EngineState>("wallpaper-state", cb);
export const onWallpaperError = (cb: (e: { wallpaperId: string | null; message: string }) => void) =>
  on<{ wallpaperId: string | null; message: string }>("wallpaper-error", cb);
export const onWallpaperLibraryChanged = (cb: () => void) => on<null>("wallpaper-library-changed", () => cb());
export const onLabelsStatus = (cb: (s: LabelsStatus) => void) => on<LabelsStatus>("labels-status", cb);
export const onTaskbarStatus = (cb: (s: TaskbarStatus) => void) => on<TaskbarStatus>("taskbar-status", cb);
export const onIntent = (cb: (intent: string) => void) => on<string>("intent", cb);
export const onShowTrayNotice = (cb: () => void) => on<null>("show-tray-notice", () => cb());

export async function minimizeWindow(): Promise<void> {
  if (isTauri()) await getCurrentWindow().minimize();
}

export async function closeWindow(): Promise<void> {
  if (isTauri()) await getCurrentWindow().close();
}

/** Save-file dialog. Returns `null` when the user cancels. */
export async function pickSavePath(defaultName: string, ext: string, filterName: string): Promise<string | null> {
  if (!isTauri()) return null;
  return save({ defaultPath: defaultName, filters: [{ name: filterName, extensions: [ext] }] });
}

/** Open-a-file dialog. Returns `null` when the user cancels. */
export async function pickOpenPath(ext: string, filterName: string): Promise<string | null> {
  if (!isTauri()) return null;
  const r = await open({ multiple: false, directory: false, filters: [{ name: filterName, extensions: [ext] }] });
  return typeof r === "string" ? r : null;
}

/** Picker for a cursor scheme file (.zip or install.inf). */
export async function pickCursorFile(filterName: string): Promise<string | null> {
  if (!isTauri()) return null;
  const r = await open({ multiple: false, directory: false, filters: [{ name: filterName, extensions: ["zip", "inf"] }] });
  return typeof r === "string" ? r : null;
}

/** Picker for one wallpaper file (video, image, GIF, HTML, exe). */
export async function pickWallpaperFile(filterName: string): Promise<string | null> {
  if (!isTauri()) return null;
  const exts = ["mp4", "m4v", "webm", "mkv", "mov", "avi", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "3gp", "ogv", "gif", "jpg", "jpeg", "png", "bmp", "jfif", "webp", "tif", "tiff", "heic", "html", "htm", "exe", "zip"];
  const r = await open({ multiple: false, directory: false, filters: [{ name: filterName, extensions: exts }] });
  return typeof r === "string" ? r : null;
}

/** Picker for a Lively wallpaper .zip file. */
export async function pickZipFile(filterName: string): Promise<string | null> {
  if (!isTauri()) return null;
  const r = await open({ multiple: false, directory: false, filters: [{ name: filterName, extensions: ["zip"] }] });
  return typeof r === "string" ? r : null;
}

/** Folder picker. */
export async function pickFolder(): Promise<string | null> {
  if (!isTauri()) return null;
  const r = await open({ multiple: false, directory: true });
  return typeof r === "string" ? r : null;
}

/** Drag-and-drop of files/folders onto the window. `onOver(true)` while dragging across it, `onDrop` on drop. */
export async function onFileDrop(onDrop: (paths: string[]) => void, onOver: (over: boolean) => void): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return getCurrentWebview().onDragDropEvent((e) => {
    const p = e.payload;
    if (p.type === "enter" || p.type === "over") onOver(true);
    else if (p.type === "leave") onOver(false);
    else if (p.type === "drop") {
      onOver(false);
      onDrop(p.paths);
    }
  });
}

/** Errors from Rust are `I18nText`; other errors are converted to a string. */
export function errorText(e: unknown, tt: (t: I18nText) => string): string {
  if (e && typeof e === "object" && "key" in e && typeof (e as I18nText).key === "string") return tt(e as I18nText);
  if (e instanceof Error) return e.message;
  return String(e);
}
