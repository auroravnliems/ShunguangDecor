import { describe, expect, it } from "vitest";
import { aspectFor, fallbackVisible, liveLayerKind, monitorLabelParts, newViewerId, objectFitFor, previewMode } from "./preview";

describe("preview helpers", () => {
  it("gives the aspect ratio of a monitor in lowest terms", () => {
    expect(aspectFor({ width: 2560, height: 1440 })).toBe("16 / 9");
    expect(aspectFor({ width: 1920, height: 1200 })).toBe("8 / 5");
    expect(aspectFor({ width: 3440, height: 1440 })).toBe("43 / 18");
  });

  it("falls back to 16 / 9 for a missing or zero size", () => {
    expect(aspectFor(null)).toBe("16 / 9");
    expect(aspectFor({ width: 0, height: 0 })).toBe("16 / 9");
  });

  it("describes a monitor for the picker", () => {
    expect(monitorLabelParts({ number: 2, width: 1920, height: 1080, primary: true })).toEqual({ n: "2", size: "1920×1080", primary: true });
  });

  it("makes distinct positive viewer ids", () => {
    const a = newViewerId();
    const b = newViewerId();
    expect(a).toBeGreaterThan(0);
    expect(Number.isSafeInteger(a)).toBe(true);
    expect(a).not.toBe(b);
  });
});

describe("live layer helpers", () => {
  it("maps scaling to object-fit, unknown falls back to cover", () => {
    expect(objectFitFor("fill")).toBe("cover");
    expect(objectFitFor("fit")).toBe("contain");
    expect(objectFitFor("stretch")).toBe("fill");
    expect(objectFitFor("center")).toBe("none");
    expect(objectFitFor("???")).toBe("cover");
  });

  it("picks the live layer kind and drops gif under reduced motion", () => {
    expect(liveLayerKind(null)).toBeNull();
    expect(liveLayerKind({ kind: "video" })).toBe("video");
    expect(liveLayerKind({ kind: "gif" })).toBe("gif");
    expect(liveLayerKind({ kind: "video" }, true)).toBe("video");
    expect(liveLayerKind({ kind: "gif" }, true)).toBeNull();
  });

  it("falls back to capture for a url that failed to play, but retries another url", () => {
    const a = { kind: "video" as const, url: "sg://a?v=1" };
    const b = { kind: "video" as const, url: "sg://b?v=1" };
    expect(liveLayerKind(a, false, a.url)).toBeNull();
    expect(liveLayerKind(b, false, a.url)).toBe("video");
    expect(liveLayerKind(a, false, "")).toBe("video");
    expect(liveLayerKind({ kind: "gif", url: "x" }, false, "x")).toBeNull();
  });
});

describe("preview mode helpers", () => {
  it("waits for the live-source query before choosing a capture mode", () => {
    expect(previewMode(false, "video")).toBeNull();
    expect(previewMode(true, null)).toBe("full");
    expect(previewMode(true, "gif")).toBe("icons");
  });

  it("keeps the fallback visible in icons mode until the media is ready", () => {
    expect(fallbackVisible(false, "icons", true)).toBe(true);
    expect(fallbackVisible(true, "icons", true)).toBe(false);
    expect(fallbackVisible(false, "full", true)).toBe(false);
    expect(fallbackVisible(false, "full", false)).toBe(true);
    expect(fallbackVisible(false, null, false)).toBe(true);
  });
});
