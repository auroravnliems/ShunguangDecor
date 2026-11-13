// Hidden features page: pure functions (no Tauri access) + the session's "warning acknowledged" flag.
import type { FeaturePriority, FeatureState, I18nText, UndoEntry } from "./types";

export const STATE_KEYS: FeatureState[] = ["default", "disabled", "enabled"];

/** Priority tiers that can be written (matches `Priority::is_immutable` in sg-features); defaults to User like ViVeTool. */
export const EDITABLE_PRIORITIES: FeaturePriority[] = ["user", "service", "safeguard", "dynamic", "user_policy", "test"];
/** Priority tier names are kept as in ViVeTool (not translated). */
export const PRIORITY_LABEL: Partial<Record<FeaturePriority, string>> = {
  user: "User",
  service: "Service",
  safeguard: "Safeguard",
  dynamic: "Dynamic",
  user_policy: "UserPolicy",
  test: "Test",
};
export const DEFAULT_PRIORITY: FeaturePriority = "user";

export const MAX_VARIANT = 63;

export const stateLabelKey = (s: FeatureState) => `features.state.${s}`;
export const priorityLabelKey = (p: FeaturePriority) => `features.priority.${p}`;

export type FeatureQuery = { kind: "empty" } | { kind: "id"; id: number } | { kind: "name"; text: string };

/** Search box: all digits (or 0x…) within the u32 range and non-zero → ID; anything else → name. */
export function parseFeatureQuery(raw: string): FeatureQuery {
  const text = raw.trim();
  if (text === "") return { kind: "empty" };
  const isNum = /^\d+$/.test(text);
  const isHex = /^0x[0-9a-f]+$/i.test(text);
  if (isNum || isHex) {
    const id = Number(text);
    if (Number.isSafeInteger(id) && id > 0 && id <= 0xffffffff) return { kind: "id", id };
  }
  return { kind: "name", text };
}

export function clampVariant(n: number): number {
  if (!Number.isFinite(n)) return 0;
  return Math.min(MAX_VARIANT, Math.max(0, Math.round(n)));
}

const detail = (details: [I18nText, I18nText][], label: string) => details.find(([l]) => l.key === label)?.[1];

/** Whether admin rights are held — read from `info.status.details` (the only source is Rust's `is_elevated()`). */
export function adminElevated(details: [I18nText, I18nText][]): boolean {
  return detail(details, "features.status.admin")?.key === "features.val.yes";
}

/** Number of features changed, from the `features.status.changed` line. */
export function changedCount(details: [I18nText, I18nText][]): number {
  const n = Number(detail(details, "features.status.changed")?.args?.n);
  return Number.isFinite(n) ? n : 0;
}

/** Number of configurations an undo pass will touch: each (store, id, priority tier) counts as one (matches Rust's `plan_undo_all`,
 *  not the number of log lines: toggling the same card on/off/on again produces many lines but is only one configuration). */
export function undoTargetCount(entries: Pick<UndoEntry, "store" | "id" | "priority">[]): number {
  return new Set(entries.map((e) => `${e.store}|${e.id}|${e.priority}`)).size;
}

// The "first change" warning dialog: remembered in a module-level variable — survives page navigation, lost when the app quits, never saved to disk.
let warningAcknowledged = false;
export const isWarningAcknowledged = () => warningAcknowledged;
export const acknowledgeWarning = () => {
  warningAcknowledged = true;
};

// ---- "All" tab: pagination ----
export const ALL_PAGE_SIZE = 50;

/** Number of pages (always ≥ 1, even when there are no rows). */
export function pageCount(total: number, pageSize: number): number {
  if (pageSize <= 0) return 1;
  return Math.max(1, Math.ceil(total / pageSize));
}

/** Page-number input (the user counts from 1) → 0-based page index clamped into [0, pages-1]; not an integer → null. */
export function parsePageInput(raw: string, pages: number): number | null {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return null;
  return Math.min(Math.max(Number(text) - 1, 0), Math.max(pages - 1, 0));
}
