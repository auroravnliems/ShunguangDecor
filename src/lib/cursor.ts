// Data types + configuration of the Cursor part (matches crates/sg-cursor, serde camelCase).

export const ROLES = [
  "Arrow",
  "Help",
  "AppStarting",
  "Wait",
  "Crosshair",
  "IBeam",
  "NWPen",
  "No",
  "SizeNS",
  "SizeWE",
  "SizeNWSE",
  "SizeNESW",
  "SizeAll",
  "UpArrow",
  "Hand",
  "Pin",
  "Person",
] as const;
export type Role = (typeof ROLES)[number];

export interface SchemeSummary {
  id: string;
  name: string;
  roleCount: number;
  importedUnix: number;
}

export interface RoleInfo {
  file: string;
  frames: number;
  /** How many milliseconds each frame is held. */
  delaysMs: number[];
  width: number;
  height: number;
  hotspot: [number, number];
}

export interface SchemeDetail {
  id: string;
  name: string;
  importedUnix: number;
  roles: Partial<Record<Role, RoleInfo>>;
  files: string[];
}

export type Scaling = "sharp" | "smooth";

export interface CursorConfig {
  scheme: string | null;
  size: number;
  scaling: Scaling;
}

export const MIN_SIZE = 32;
export const MAX_SIZE = 256;
export const SIZE_STEP = 8;

/** Raw data from `cursor.toml` (may have missing keys/wrong types) → a configuration with all keys. Matches `CursorConfig::from_table` in Rust. */
export function normalizeCursorConfig(raw: Record<string, unknown>): CursorConfig {
  const scheme = typeof raw.scheme === "string" && raw.scheme.trim() ? raw.scheme.trim() : null;
  const n = typeof raw.size === "number" && Number.isFinite(raw.size) ? raw.size : MIN_SIZE;
  const size = Math.min(MAX_SIZE, Math.max(MIN_SIZE, Math.floor((Math.max(0, n) + SIZE_STEP / 2) / SIZE_STEP) * SIZE_STEP));
  return { scheme, size, scaling: raw.scaling === "smooth" ? "smooth" : "sharp" };
}

/** The frame being shown after `elapsedMs` milliseconds of an infinitely looping animation. */
export function frameAt(delaysMs: number[], elapsedMs: number): number {
  const total = delaysMs.reduce((a, b) => a + Math.max(0, b), 0);
  if (delaysMs.length < 2 || total <= 0) return 0;
  let t = ((elapsedMs % total) + total) % total;
  for (let i = 0; i < delaysMs.length; i++) {
    t -= Math.max(0, delaysMs[i]);
    if (t < 0) return i;
  }
  return 0;
}

/** Preview image URL via `sg://` (`which` = "sheet" or a frame number). Change `v` to force a reload. */
export function cursorImageUrl(base: string, id: string, role: Role, which: "sheet" | number, v?: number | string): string {
  const file = which === "sheet" ? "sheet.png" : `${which}.png`;
  return `${base}/cursor/${id}/${role}/${file}${v !== undefined ? `?v=${v}` : ""}`;
}

/** Number of visible frames of a role (static = 1). */
export function isAnimated(info: RoleInfo | undefined): boolean {
  return !!info && info.frames > 1;
}

/** When the home page's "Import cursors" is clicked, open the Cursors page and show the file picker right away (the page reads then clears this flag). */
export const importIntent = { pending: false };
