// Pure functions for the real-desktop preview pane (components/DesktopPreview.tsx).
import type { MonitorView } from "./types";

function gcd(a: number, b: number): number {
  return b === 0 ? a : gcd(b, a % b);
}

/** A monitor's frame ratio as a CSS `aspect-ratio` ("16 / 9"); 16 / 9 when data is missing. */
export function aspectFor(m: { width: number; height: number } | null | undefined): string {
  if (!m || m.width <= 0 || m.height <= 0) return "16 / 9";
  const g = gcd(m.width, m.height);
  return `${m.width / g} / ${m.height / g}`;
}

export function monitorLabelParts(m: MonitorView): { n: string; size: string; primary: boolean } {
  return { n: String(m.number), size: `${m.width}×${m.height}`, primary: m.primary };
}

let counter = 0;
/** Viewer identifier: random at mount time, safe within JS's integer range. */
export function newViewerId(): number {
  counter += 1;
  return Math.floor(Math.random() * 2 ** 40) * 64 + (counter % 64) + 1;
}

/** Maps the wallpaper configuration's `scaling` to CSS `object-fit`; an unknown value → `cover`. */
export function objectFitFor(scaling: string): "cover" | "contain" | "fill" | "none" {
  switch (scaling) {
    case "fit":
      return "contain";
    case "stretch":
      return "fill";
    case "center":
      return "none";
    default:
      return "cover";
  }
}

/**
 * Which kind of live-playback layer to build: `null` = use the old capture path.
 * Reduced motion: video is still built (shows only the first frame, no autoplay) while a GIF cannot be stopped → fall back to the capture path.
 */
export function liveLayerKind(
  source: { kind: "video" | "gif"; url?: string } | null | undefined,
  reducedMotion = false,
  failedUrl = "",
): "video" | "gif" | null {
  if (!source) return null;
  // This URL could not be played in the WebView (codec/CSP): use the capture path. Other URLs are retried normally.
  if (failedUrl && source.url === failedUrl) return null;
  if (source.kind === "gif" && reducedMotion) return null;
  return source.kind;
}

/** Capture mode from the source query result: `null` = query not finished yet (`captureStart` must not be called yet, to avoid a redundant JPEG capture). */
export function previewMode(resolved: boolean, layerKind: "video" | "gif" | null): "full" | "icons" | null {
  if (!resolved) return null;
  return layerKind ? "icons" : "full";
}

/** Whether to show the current wallpaper image (fallback): in icons mode it is shown until the video layer is ready (the transparent PNG icon image cannot cover it). */
export function fallbackVisible(showMedia: boolean, mode: "full" | "icons" | null, live: boolean): boolean {
  return !showMedia && (mode === "icons" || !live);
}
