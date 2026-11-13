// Data types + configuration of the Wallpaper module (matches crates/sg-wallpaper, serde camelCase;
// the `config/wallpaper.toml` configuration uses snake_case like TOML keys).

export type Arrangement = "per_monitor" | "span" | "duplicate";
export type Scaling = "fill" | "fit" | "stretch";
export type VideoPlayer = "auto" | "media_foundation" | "mpv";

/** `[layout]` key for the Span / Duplicate arrangements (matches `LAYOUT_ALL` on the Rust side). */
export const LAYOUT_ALL = "all";

export interface WallpaperConfig {
  arrangement: Arrangement;
  /** Screen number ("1", "2"...) or "all" → wallpaper id. */
  layout: Record<string, string>;
  scaling: Scaling;
  volume: number;
  video_player: VideoPlayer;
  mpv_path: string;
  hardware_decoding: boolean;
  pause_when_covered: boolean;
  pause_when_app_focused: boolean;
  pause_per_monitor: boolean;
  pause_on_battery: boolean;
  pause_on_battery_saver: boolean;
  pause_on_remote_desktop: boolean;
  pause_apps: string[];
  poll_interval_ms: number;
  trim_memory: boolean;
}

export const DEFAULT_WALLPAPER_CONFIG: WallpaperConfig = {
  arrangement: "per_monitor",
  layout: {},
  scaling: "fill",
  volume: 0,
  video_player: "auto",
  mpv_path: "",
  hardware_decoding: true,
  pause_when_covered: true,
  pause_when_app_focused: false,
  pause_per_monitor: true,
  pause_on_battery: true,
  pause_on_battery_saver: true,
  pause_on_remote_desktop: true,
  pause_apps: [],
  poll_interval_ms: 750,
  trim_memory: true,
};

function pick<T extends string>(v: unknown, allowed: readonly T[], d: T): T {
  return typeof v === "string" && (allowed as readonly string[]).includes(v) ? (v as T) : d;
}
function bool(v: unknown, d: boolean): boolean {
  return typeof v === "boolean" ? v : d;
}
function num(v: unknown, d: number, min: number, max: number): number {
  return typeof v === "number" && Number.isFinite(v) ? Math.min(max, Math.max(min, Math.round(v))) : d;
}

/** Raw data (may have missing keys/wrong types) → a configuration with all keys, values clamped as on the Rust side. */
export function normalizeWallpaperConfig(raw: Record<string, unknown>): WallpaperConfig {
  const d = DEFAULT_WALLPAPER_CONFIG;
  const layout: Record<string, string> = {};
  if (raw.layout && typeof raw.layout === "object" && !Array.isArray(raw.layout)) {
    for (const [k, v] of Object.entries(raw.layout as Record<string, unknown>)) {
      if (typeof v === "string" && v.trim()) layout[k.trim()] = v.trim();
    }
  }
  const apps = Array.isArray(raw.pause_apps)
    ? raw.pause_apps.filter((a): a is string => typeof a === "string").map((a) => a.trim()).filter(Boolean)
    : [];
  return {
    arrangement: pick(raw.arrangement, ["per_monitor", "span", "duplicate"] as const, d.arrangement),
    layout,
    scaling: pick(raw.scaling, ["fill", "fit", "stretch"] as const, d.scaling),
    volume: num(raw.volume, d.volume, 0, 100),
    video_player: pick(raw.video_player, ["auto", "media_foundation", "mpv"] as const, d.video_player),
    mpv_path: typeof raw.mpv_path === "string" ? raw.mpv_path.trim() : "",
    hardware_decoding: bool(raw.hardware_decoding, d.hardware_decoding),
    pause_when_covered: bool(raw.pause_when_covered, d.pause_when_covered),
    pause_when_app_focused: bool(raw.pause_when_app_focused, d.pause_when_app_focused),
    pause_per_monitor: bool(raw.pause_per_monitor, d.pause_per_monitor),
    pause_on_battery: bool(raw.pause_on_battery, d.pause_on_battery),
    pause_on_battery_saver: bool(raw.pause_on_battery_saver, d.pause_on_battery_saver),
    pause_on_remote_desktop: bool(raw.pause_on_remote_desktop, d.pause_on_remote_desktop),
    pause_apps: apps,
    poll_interval_ms: num(raw.poll_interval_ms, d.poll_interval_ms, 250, 10_000),
    trim_memory: bool(raw.trim_memory, d.trim_memory),
  };
}

// ---------- layout ----------

/** `[layout]` key for the screen being edited: the screen number for "Per screen", "all" for Span/Duplicate. */
export function layoutKey(arrangement: Arrangement, monitor: number): string {
  return arrangement === "per_monitor" ? String(monitor) : LAYOUT_ALL;
}

/** Sets the wallpaper for one screen (or for "all"); returns a new `layout` without mutating the old one. */
export function withWallpaper(cfg: WallpaperConfig, monitor: number, id: string): Record<string, string> {
  return { ...cfg.layout, [layoutKey(cfg.arrangement, monitor)]: id };
}

/** Removes the wallpaper from one screen. */
export function withoutWallpaper(cfg: WallpaperConfig, monitor: number): Record<string, string> {
  const next = { ...cfg.layout };
  delete next[layoutKey(cfg.arrangement, monitor)];
  return next;
}

/** The `[layout]` keys currently using wallpaper `id` (to draw the "In use" label). */
export function usedBy(cfg: WallpaperConfig, id: string): string[] {
  return Object.entries(cfg.layout)
    .filter(([, v]) => v === id)
    .map(([k]) => k);
}

/** Changes the arrangement while keeping the most sensible wallpaper. */
export function layoutForArrangement(cfg: WallpaperConfig, next: Arrangement): Record<string, string> {
  if (next === cfg.arrangement) return cfg.layout;
  if (next === "per_monitor") {
    const all = cfg.layout[LAYOUT_ALL];
    const rest = Object.fromEntries(Object.entries(cfg.layout).filter(([k]) => k !== LAYOUT_ALL));
    return all && Object.keys(rest).length === 0 ? { "1": all } : rest;
  }
  const first = cfg.layout[LAYOUT_ALL] ?? cfg.layout["1"] ?? Object.values(cfg.layout)[0];
  return first ? { [LAYOUT_ALL]: first } : {};
}

// ---------- data from the core ----------

export interface WallpaperItem {
  id: string;
  title: string;
  kind: string;
  source: string;
  sourceExists: boolean;
  hasProperties: boolean;
  thumbUrl: string | null;
}

export interface RunningView {
  /** Screen number; 0 = span. */
  monitor: number;
  wallpaperId: string;
  backend: string;
  paused: boolean;
}

export interface EngineState {
  running: RunningView[];
  manualPause: boolean;
  error: string | null;
  errors: Record<string, string>;
}

export const EMPTY_ENGINE_STATE: EngineState = { running: [], manualPause: false, error: null, errors: {} };

export interface PropertiesView {
  controls: Record<string, LivelyControl>;
  values: Record<string, unknown>;
}

// ---------- Lively customization ----------

export interface LivelyControl {
  type?: string;
  text?: string;
  help?: string;
  value?: unknown;
  min?: number;
  max?: number;
  step?: number;
  items?: unknown[];
  folder?: string;
  filter?: string;
}

export type ControlKind = "slider" | "checkbox" | "dropdown" | "color" | "textbox" | "folderDropdown" | "button" | "label";

const KINDS: readonly ControlKind[] = ["slider", "checkbox", "dropdown", "color", "textbox", "folderDropdown", "button", "label"];

/** Control type to render; an unknown type is rendered as a label (so it does not break the page). */
export function controlKind(c: { type?: unknown } | null | undefined): ControlKind {
  const t = typeof c?.type === "string" ? c.type : "";
  return (KINDS as readonly string[]).includes(t) ? (t as ControlKind) : "label";
}

/** Whether any wallpaper is running on screen `monitor` (or spanned). */
export function runningOn(state: EngineState, monitor: number): RunningView | undefined {
  return state.running.find((r) => r.monitor === monitor) ?? state.running.find((r) => r.monitor === 0);
}

/** An error from the core may carry the code `source_missing: <path>`. */
export function parseSourceMissing(message: string | undefined): string | null {
  const m = message?.match(/^source_missing:\s*(.*)$/);
  return m ? m[1] : null;
}
