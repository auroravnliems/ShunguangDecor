// Desktop labels part configuration (matches crates/sg-labels/src/config.rs; TOML keys are snake_case, `custom` is a sub-table).
import { normalizeHex } from "./color";
import type { LabelsStatus } from "./types";

export type LabelsMode = "auto" | "custom";

export interface LabelsCustom {
  color: string;
  outline_color: string;
  /** 0–4 px (0 = no outline). */
  outline_width: number;
  shadow_color: string;
  /** 0–100 % (0 = no shadow). */
  shadow_alpha: number;
  /** -6…6 px. */
  shadow_dx: number;
  shadow_dy: number;
  backdrop_color: string;
  /** 0–100 % (0 = no translucent background). */
  backdrop_alpha: number;
}

export interface LabelsConfig {
  mode: LabelsMode;
  /** Auto: adds a thin outline/shadow so text stays readable on a speckled background. */
  outline: boolean;
  custom: LabelsCustom;
}

export const MAX_OUTLINE = 4;
export const MAX_SHADOW_OFFSET = 6;

export const DEFAULT_LABELS_CONFIG: LabelsConfig = {
  mode: "auto",
  outline: true,
  custom: {
    color: "#ffffff",
    outline_color: "#000000",
    outline_width: 1,
    shadow_color: "#000000",
    shadow_alpha: 0,
    shadow_dx: 1,
    shadow_dy: 1,
    backdrop_color: "#000000",
    backdrop_alpha: 0,
  },
};

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? Math.round(v) : null;
}

function clamp(v: unknown, min: number, max: number, d: number): number {
  const n = num(v);
  return n === null ? d : Math.min(max, Math.max(min, n));
}

/** Outline thickness 0–4 (same as Rust: an odd value → default). */
export function clampOutline(v: unknown): number {
  return clamp(v, 0, MAX_OUTLINE, DEFAULT_LABELS_CONFIG.custom.outline_width);
}

function color(v: unknown, d: string): string {
  return (typeof v === "string" && normalizeHex(v)) || d;
}

/** Raw data from `config/labels.toml` (may have missing keys / wrong types) → a configuration with every key. */
export function normalizeLabelsConfig(raw: Record<string, unknown>): LabelsConfig {
  const d = DEFAULT_LABELS_CONFIG;
  const c = raw.custom && typeof raw.custom === "object" && !Array.isArray(raw.custom) ? (raw.custom as Record<string, unknown>) : {};
  return {
    mode: raw.mode === "custom" ? "custom" : "auto",
    outline: typeof raw.outline === "boolean" ? raw.outline : d.outline,
    custom: {
      color: color(c.color, d.custom.color),
      outline_color: color(c.outline_color, d.custom.outline_color),
      outline_width: clampOutline(c.outline_width),
      shadow_color: color(c.shadow_color, d.custom.shadow_color),
      shadow_alpha: clamp(c.shadow_alpha, 0, 100, d.custom.shadow_alpha),
      shadow_dx: clamp(c.shadow_dx, -MAX_SHADOW_OFFSET, MAX_SHADOW_OFFSET, d.custom.shadow_dx),
      shadow_dy: clamp(c.shadow_dy, -MAX_SHADOW_OFFSET, MAX_SHADOW_OFFSET, d.custom.shadow_dy),
      backdrop_color: color(c.backdrop_color, d.custom.backdrop_color),
      backdrop_alpha: clamp(c.backdrop_alpha, 0, 100, d.custom.backdrop_alpha),
    },
  };
}

export const OFF_STATUS: LabelsStatus = {
  method: "off",
  mode: "auto",
  explorerPid: null,
  items: 0,
  error: null,
  needsExplorerRestart: false,
  source: "none",
};

/** Status strip on the page: Explorer restart needed > fallback > DLL > off. */
export type StatusKind = "restart" | "fallback" | "dll" | "off";

export function statusKind(s: LabelsStatus): StatusKind {
  if (s.needsExplorerRestart) return "restart";
  if (s.method === "fallback") return "fallback";
  if (s.method === "dll") return "dll";
  return "off";
}
