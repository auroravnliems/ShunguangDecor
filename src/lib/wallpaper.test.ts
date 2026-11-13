import { describe, expect, it } from "vitest";
import {
  DEFAULT_WALLPAPER_CONFIG,
  EMPTY_ENGINE_STATE,
  controlKind,
  layoutForArrangement,
  layoutKey,
  normalizeWallpaperConfig,
  parseSourceMissing,
  runningOn,
  usedBy,
  withWallpaper,
  withoutWallpaper,
} from "./wallpaper";

describe("normalizeWallpaperConfig", () => {
  it("matches the defaults for an empty file", () => {
    expect(normalizeWallpaperConfig({})).toEqual(DEFAULT_WALLPAPER_CONFIG);
  });

  it("clamps and ignores wrong types", () => {
    const c = normalizeWallpaperConfig({ volume: 900, poll_interval_ms: 3, arrangement: 7, scaling: "zoom", pause_apps: [" obs64.exe ", 5, ""] });
    expect(c.volume).toBe(100);
    expect(c.poll_interval_ms).toBe(250);
    expect(c.arrangement).toBe("per_monitor");
    expect(c.scaling).toBe("fill");
    expect(c.pause_apps).toEqual(["obs64.exe"]);
    expect(normalizeWallpaperConfig({ volume: "x" }).volume).toBe(0);
  });

  it("keeps only string ids in the layout", () => {
    const c = normalizeWallpaperConfig({ layout: { "1": "a", "2": 5, "3": " " } });
    expect(c.layout).toEqual({ "1": "a" });
  });
});

describe("controlKind", () => {
  it("knows the Lively control types", () => {
    expect(controlKind({ type: "slider" })).toBe("slider");
    expect(controlKind({ type: "folderDropdown" })).toBe("folderDropdown");
    expect(controlKind({ type: "checkbox" })).toBe("checkbox");
  });
  it("draws unknown types as a label", () => {
    expect(controlKind({ type: "hologram" })).toBe("label");
    expect(controlKind({})).toBe("label");
    expect(controlKind(null)).toBe("label");
  });
});

describe("layout helpers", () => {
  const per = normalizeWallpaperConfig({ layout: { "1": "a", "2": "b" } });
  it("uses the monitor number per screen and 'all' otherwise", () => {
    expect(layoutKey("per_monitor", 2)).toBe("2");
    expect(layoutKey("span", 2)).toBe("all");
  });
  it("sets and clears without mutating", () => {
    expect(withWallpaper(per, 1, "z")).toEqual({ "1": "z", "2": "b" });
    expect(per.layout["1"]).toBe("a");
    expect(withoutWallpaper(per, 2)).toEqual({ "1": "a" });
    expect(usedBy(per, "b")).toEqual(["2"]);
  });
  it("converts the layout when the arrangement changes", () => {
    expect(layoutForArrangement(per, "span")).toEqual({ all: "a" });
    const span = normalizeWallpaperConfig({ arrangement: "span", layout: { all: "s" } });
    expect(layoutForArrangement(span, "per_monitor")).toEqual({ "1": "s" });
    expect(layoutForArrangement(per, "per_monitor")).toBe(per.layout);
  });
});

describe("engine state helpers", () => {
  it("finds the wallpaper on a screen, falling back to span", () => {
    const s = { ...EMPTY_ENGINE_STATE, running: [{ monitor: 0, wallpaperId: "s", backend: "mpv", paused: false }] };
    expect(runningOn(s, 2)?.wallpaperId).toBe("s");
    expect(runningOn(EMPTY_ENGINE_STATE, 1)).toBeUndefined();
  });
  it("reads the source_missing code", () => {
    expect(parseSourceMissing("source_missing: C:\\a.mp4")).toBe("C:\\a.mp4");
    expect(parseSourceMissing("other")).toBeNull();
    expect(parseSourceMissing(undefined)).toBeNull();
  });
});
