// Page order and the decoration parameters that change per page (taken from the v9 mockup).
import type { PageId } from "./types";

export const NAV_PAGES: PageId[] = ["home", "taskbar", "wallpaper", "cursor", "labels", "features"];

/** Large outlined text on the right edge — decorative text, kept in English in every language. */
export const BIGWORD: Record<PageId, string> = {
  home: "HOME",
  taskbar: "TASKBAR",
  wallpaper: "WALLPAPER",
  cursor: "CURSOR",
  labels: "LABELS",
  features: "FEATURES",
  settings: "SETTINGS",
  gallery: "GALLERY",
};

/** Position [top, height] (px on the 1280×760 stage) of the vertical blue bar on the left. */
export const RAIL: Record<PageId, [number, number]> = {
  home: [300, 160],
  taskbar: [200, 120],
  wallpaper: [260, 140],
  cursor: [230, 110],
  labels: [380, 90],
  features: [320, 120],
  settings: [280, 130],
  gallery: [280, 130],
};

const total = String(NAV_PAGES.length).padStart(2, "0");

export function pageNo(p: PageId): string {
  const i = NAV_PAGES.indexOf(p);
  return i < 0 ? `— / ${total}` : `${String(i + 1).padStart(2, "0")} / ${total}`;
}

/** Polka-dot pattern that drifts up/down per page. */
export function dotsShift(p: PageId): number {
  const i = Math.max(0, NAV_PAGES.indexOf(p));
  return (i % 3) * 40 - 20;
}
