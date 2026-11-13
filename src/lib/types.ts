// Data types matching the Rust core (serde camelCase). Change one side and change the other too.

export type ModuleId = "taskbar" | "wallpaper" | "cursor" | "labels" | "features";
export const MODULE_IDS: ModuleId[] = ["taskbar", "wallpaper", "cursor", "labels", "features"];

export type PageId = "home" | ModuleId | "settings" | "gallery";

/** Text to translate, sent up by Rust: key + variables. */
export interface I18nText {
  key: string;
  args?: Record<string, string>;
}

export type Availability =
  | { kind: "available" }
  | { kind: "partial"; reason: I18nText }
  | { kind: "unsupported"; reason: I18nText };

export interface ModuleStatus {
  summary: I18nText | null;
  details: [I18nText, I18nText][];
}

export interface ModuleInfo {
  id: ModuleId;
  number: number;
  availability: Availability;
  enabled: boolean;
  fault: string | null;
  status: ModuleStatus;
}

export interface AppInfo {
  version: string;
  protocolBase: string;
  portable: boolean;
  githubUrl: string | null;
  discordUrl: string;
}

export interface Settings {
  language: string;
  autostart: boolean;
  reduceMotion: boolean;
  /** Follow the operating system's disable-animations setting (off by default). */
  followOsMotion: boolean;
}

export interface RuntimeStats {
  ramBytes: number | null;
}

export interface WallpaperView {
  name: string;
  url: string;
}

export interface ProfileMeta {
  name: string;
  modifiedUnix: number;
}

export interface ProfileList {
  profiles: ProfileMeta[];
  active: string | null;
}

export interface ImportSummary {
  files: number;
}

export interface RestoreResult {
  id: ModuleId;
  error: I18nText | null;
}

/** Preview frame mode: `full` = the whole desktop (JPEG), `icons` = the icon layer only (PNG with a transparent background). */
export type CaptureMode = "full" | "icons";

/** A video/GIF wallpaper running on a monitor (command `capture_live_source`), so that same file can be played on the underlying layer. */
export interface LiveSource {
  kind: "video" | "gif";
  /** `sg://wallpaper/media/<id>?v=…` — use directly as `src`. */
  url: string;
  /** The wallpaper configuration's scaling mode. */
  scaling: "fill" | "fit" | "stretch";
  /** The corresponding CSS `object-fit` value (`cover` | `contain` | `fill`). */
  objectFit: "cover" | "contain" | "fill";
  /** Currently paused (the user pressed pause or an auto-pause rule applies). */
  paused: boolean;
}

/** One monitor of the machine (command `list_monitors`). */
export interface MonitorView {
  number: number;
  width: number;
  height: number;
  primary: boolean;
}

/** One widget in the Taskbar page's catalog (command `taskbar_widget_catalog`). */
export interface WidgetCatalogItem {
  kind: string;
  nameKey: string;
  descKey: string;
  /** Each type can only be placed once on the bar. */
  singleton: boolean;
}

/** `generated`: file written by the app; `edited`: the user edited it by hand or imported a YASB theme (the engine uses their file). */
export type TaskbarRawState = "generated" | "edited";

/** State of the YASB Taskbar part (command `taskbar_status`, event `taskbar-status`). */
export interface TaskbarStatus {
  enabled: boolean;
  /** The original Windows taskbar is currently hidden by this part. */
  nativeHidden: boolean;
  rawState: TaskbarRawState;
  error: I18nText | null;
}

/** State of the Desktop labels part (command `labels_status`, event `labels-status`). */
export interface LabelsStatus {
  /** `dll` = the DLL inside Explorer; `fallback` = a single shared color (no DLL); `off` = disabled. */
  method: "dll" | "fallback" | "off";
  mode: "auto" | "custom";
  explorerPid: number | null;
  /** Number of file names currently colored individually (Auto via the DLL). */
  items: number;
  /** Why it is in fallback mode / the most recent error. */
  error: I18nText | null;
  needsExplorerRestart: boolean;
  source: "static" | "animated" | "none";
}

// ---- Hidden features part (src-tauri/src/modules/features.rs) ----
// Note: FeatureConfig/UndoEntry/Subscription cross IPC in snake_case (exactly as crates/sg-features' serde does);
// FeatureRow/SuggestionRow/ImportReport are camelCase like the other commands.
export type FeatureStore = "runtime" | "boot";
export type FeatureState = "default" | "disabled" | "enabled";
export type FeaturePriority =
  | "image_default"
  | "ekb"
  | "safeguard"
  | "image_default_edition_override"
  | "service"
  | "dynamic"
  | "user"
  | "security"
  | "user_policy"
  | "test"
  | "image_override";
export type FeaturePayloadKind = "none" | "resident" | "external";

export interface FeatureConfig {
  id: number;
  priority: FeaturePriority;
  state: FeatureState;
  variant: number;
  variant_payload_kind: FeaturePayloadKind;
  variant_payload: number;
  /** Experimental configuration (Wexp). */
  wexp: boolean;
}

export interface FeatureRow {
  id: number;
  name: string | null;
  runtime: FeatureConfig | null;
  boot: FeatureConfig | null;
  changedByApp: boolean;
}

export interface SuggestionRow {
  id: number;
  nameKey: string;
  descKey: string;
  row: FeatureRow;
}

export interface UndoEntry {
  unix: number;
  store: FeatureStore;
  /** The original configuration before the change; null = there was no configuration then. */
  original: FeatureConfig | null;
  id: number;
  priority: FeaturePriority;
}

export interface FeatureSubscription {
  id: number;
  reporting_kind: number;
  reporting_options: number;
  /** A `0x…` hex string (a u64 can exceed 2^53, so it does not travel as a number through JS). */
  reporting_target: string;
}

/** Feature usage report (snake_case, like the crate's serde). */
export interface FeatureUsageReport {
  id: number;
  reporting_kind: number;
  reporting_options: number;
}

export interface FeatureImportReport {
  applied: number;
  skipped: number;
  warnings: string[];
}

/** Request for one page of the "All" tab (camelCase over IPC). `page` counts from 0. */
export interface FeatureListQuery {
  page: number;
  pageSize: number;
  configuredOnly: boolean;
  hideUnnamed: boolean;
  text: string;
  sort: "name" | "id";
}

/** One page of results: `total` is the number of rows after filtering, `page` is the actual page returned (clamped). */
export interface FeaturePage {
  total: number;
  page: number;
  pageSize: number;
  rows: FeatureRow[];
}
