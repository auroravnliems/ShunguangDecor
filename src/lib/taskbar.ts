// Config of the YASB Taskbar module (matches crates/sg-taskbar/src/model.rs; TOML keys are snake_case) and pure helpers for
// the page: normalization, widget ordering, per-widget options.
import { normalizeHex } from "./color";

export type Edge = "top" | "bottom";
export type BarStyle = "docked" | "floating";
export type Blur = "none" | "blur" | "acrylic";
export type WinButtonPos = "left" | "center";
export type NativeMode = "keep" | "hide" | "reveal";
export type Corner = "top_left" | "top_right" | "bottom_left" | "bottom_right";
/** The core accepts the string "none" for a disabled hot corner (see `TaskbarConfig::from_table`). */
export type HotCorner = Corner | "none";
export type ZoneName = "left" | "center" | "right";
export const ZONE_NAMES: ZoneName[] = ["left", "center", "right"];

export const WIDGET_KINDS = [
  "windows_button",
  "search",
  "apps",
  "tray",
  "clock",
  "volume",
  "network",
  "battery",
  "notifications",
  "cpu",
  "memory",
  "active_window",
  "power_menu",
] as const;

/** One item of a zone: a widget name or a table `{ kind, ...options }` (exactly the form written in TOML). */
export type WidgetItem = string | ({ kind: string } & Record<string, unknown>);
export interface Zones {
  left: WidgetItem[];
  center: WidgetItem[];
  right: WidgetItem[];
}

export const HEIGHT = { min: 24, max: 64 };
export const MARGIN = { min: 0, max: 32 };
export const FONT_SIZE = { min: 9, max: 20 };
export const CORNERS: Corner[] = ["top_left", "top_right", "bottom_left", "bottom_right"];

export interface TaskbarConfig {
  edge: Edge;
  height: number;
  style: BarStyle;
  margin: number;
  background: string;
  opacity: number;
  blur: Blur;
  accent: string;
  font_family: string;
  font_size: number;
  text_color: string;
  windows_button: WinButtonPos;
  native: NativeMode;
  widgets: Zones;
  hide_on_fullscreen: boolean;
  hot_corner: HotCorner;
  all_monitors: boolean;
  auto_hide: boolean;
}

export function defaultTaskbarConfig(): TaskbarConfig {
  return {
    edge: "bottom",
    height: 36,
    style: "docked",
    margin: 8,
    background: "#1c1c1c",
    opacity: 85,
    blur: "blur",
    accent: "#4cc2ff",
    font_family: "Segoe UI Variable",
    font_size: 12,
    text_color: "#ffffff",
    windows_button: "left",
    native: "reveal",
    widgets: {
      left: ["windows_button", "search", "apps"],
      center: ["clock"],
      right: ["tray", "network", "volume", "battery", "notifications"],
    },
    hide_on_fullscreen: true,
    hot_corner: "none",
    all_monitors: true,
    auto_hide: false,
  };
}

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);

/** Like `read_int`: a number (floats are rounded) clamped into the range, wrong type → default. */
function num(v: unknown, min: number, max: number, d: number): number {
  if (typeof v !== "number" || !Number.isFinite(v)) return d;
  return Math.min(max, Math.max(min, Math.round(v)));
}
const pick = <T extends string>(v: unknown, all: readonly T[], d: T): T => (all.includes(v as T) ? (v as T) : d);
const color = (v: unknown, d: string): string => (typeof v === "string" && normalizeHex(v)) || d;
const bool = (v: unknown, d: boolean): boolean => (typeof v === "boolean" ? v : d);

export function kindOf(w: WidgetItem): string {
  return typeof w === "string" ? w : w.kind;
}

/** Raw data from `config/taskbar.toml` → a config with all keys, like `TaskbarConfig::from_table`. */
export function normalizeTaskbarConfig(raw: Record<string, unknown>): TaskbarConfig {
  const d = defaultTaskbarConfig();
  const c: TaskbarConfig = {
    edge: pick(raw.edge, ["top", "bottom"], d.edge),
    height: num(raw.height, HEIGHT.min, HEIGHT.max, d.height),
    style: pick(raw.style, ["docked", "floating"], d.style),
    margin: num(raw.margin, MARGIN.min, MARGIN.max, d.margin),
    background: color(raw.background, d.background),
    opacity: num(raw.opacity, 0, 100, d.opacity),
    blur: pick(raw.blur, ["none", "blur", "acrylic"], d.blur),
    accent: color(raw.accent, d.accent),
    font_family: typeof raw.font_family === "string" && raw.font_family.trim() ? raw.font_family.trim() : d.font_family,
    font_size: num(raw.font_size, FONT_SIZE.min, FONT_SIZE.max, d.font_size),
    text_color: color(raw.text_color, d.text_color),
    windows_button: pick(raw.windows_button, ["left", "center"], d.windows_button),
    native: pick(raw.native, ["keep", "hide", "reveal"], d.native),
    widgets: d.widgets,
    hide_on_fullscreen: bool(raw.hide_on_fullscreen, d.hide_on_fullscreen),
    hot_corner: pick(raw.hot_corner, CORNERS, "none"),
    all_monitors: bool(raw.all_monitors, d.all_monitors),
    auto_hide: bool(raw.auto_hide, d.auto_hide),
  };
  if (isObj(raw.widgets)) {
    const seen = new Set<string>();
    const widgets: Zones = { ...d.widgets };
    for (const zone of ZONE_NAMES) {
      const arr = raw.widgets[zone];
      if (!Array.isArray(arr)) continue; // not declared / wrong type: keep the zone's default
      const out: WidgetItem[] = [];
      for (const item of arr) {
        const kind = typeof item === "string" ? item.trim() : isObj(item) && typeof item.kind === "string" ? item.kind.trim() : "";
        if (!(WIDGET_KINDS as readonly string[]).includes(kind) || seen.has(kind)) continue;
        seen.add(kind);
        out.push(isObj(item) ? { ...item, kind } : kind);
      }
      widgets[zone] = out;
    }
    c.widgets = widgets;
  }
  return c;
}

/**
 * The bar's "auto-hide" is ignored by the engine when the original taskbar is in "reveal" mode (by default at the bottom edge) and the bar is also at the
 * bottom edge: both compete for one edge. The page does not know the original taskbar's real edge, so it assumes the bottom edge.
 */
export function autoHideBlocked(c: Pick<TaskbarConfig, "native" | "edge">): boolean {
  return c.native === "reveal" && c.edge === "bottom";
}

/** The zones the bar actually uses: a "centered" Windows button is moved to the start of the center zone (like the config generator). */
export function effectiveZones(c: TaskbarConfig): Zones {
  const z: Zones = { left: [...c.widgets.left], center: [...c.widgets.center], right: [...c.widgets.right] };
  if (c.windows_button === "center") {
    const wb = [...z.left, ...z.center, ...z.right].find((w) => kindOf(w) === "windows_button");
    if (wb !== undefined) {
      z.left = z.left.filter((w) => kindOf(w) !== "windows_button");
      z.center = z.center.filter((w) => kindOf(w) !== "windows_button");
      z.center.unshift(wb);
    }
  }
  return z;
}

export interface WidgetSlot {
  zone: ZoneName;
  index: number;
}

/** Moves a widget from `from` to `to`. `to.index` is the insertion SLOT in the displayed list (0 = before the first item,
 *  n = after the last item); an invalid source returns an unchanged copy. Does not mutate the input. */
export function moveWidget(z: Zones, m: { from: WidgetSlot; to: WidgetSlot }): Zones {
  const out: Zones = { left: [...z.left], center: [...z.center], right: [...z.right] };
  const src = out[m.from.zone];
  if (!Number.isInteger(m.from.index) || m.from.index < 0 || m.from.index >= src.length) return out;
  const [item] = src.splice(m.from.index, 1);
  let at = m.to.index;
  if (m.to.zone === m.from.zone && at > m.from.index) at -= 1;
  const dst = out[m.to.zone];
  dst.splice(Math.min(dst.length, Math.max(0, at)), 0, item);
  return out;
}

/** Appends to the end of a zone; a type that already exists somewhere is not added. */
export function addWidget(z: Zones, zone: ZoneName, kind: string): Zones {
  if (ZONE_NAMES.some((n) => z[n].some((w) => kindOf(w) === kind))) return z;
  return { ...z, [zone]: [...z[zone], kind] };
}

export function removeWidget(z: Zones, zone: ZoneName, index: number): Zones {
  if (index < 0 || index >= z[zone].length) return z;
  return { ...z, [zone]: z[zone].filter((_, i) => i !== index) };
}

/** Writes a widget's own options; empty goes back to the plain name string form. */
export function setWidgetOptions(z: Zones, zone: ZoneName, index: number, options: Record<string, unknown>): Zones {
  const cur = z[zone][index];
  if (cur === undefined) return z;
  const kind = kindOf(cur);
  const item: WidgetItem = Object.keys(options).length === 0 ? kind : { ...options, kind };
  return { ...z, [zone]: z[zone].map((w, i) => (i === index ? item : w)) };
}

export function optionsOf(w: WidgetItem): Record<string, unknown> {
  if (typeof w === "string") return {};
  const { kind: _kind, ...rest } = w;
  return rest;
}

/** Clock: the generator reads only the `label` key (a format string), so 12/24-hour + seconds are encoded in it. */
export interface ClockOptions {
  hour12: boolean;
  seconds: boolean;
}
export function clockOptions(o: Record<string, unknown>): ClockOptions {
  const l = typeof o.label === "string" ? o.label : "";
  return { hour12: /%[Ilp]/.test(l), seconds: /%S/.test(l) };
}
export function clockLabel(o: ClockOptions): string {
  return `{${o.hour12 ? "%I" : "%H"}:%M${o.seconds ? ":%S" : ""}${o.hour12 ? " %p" : ""}}<span>{alarm}</span>`;
}

/** Built-in fonts used when the font-listing command returns empty (running outside the app). */
export const FALLBACK_FONTS = ["Segoe UI Variable", "Segoe UI", "Arial", "Calibri", "Consolas"];
