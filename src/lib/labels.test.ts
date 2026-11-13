import { describe, expect, it } from "vitest";
import { DEFAULT_LABELS_CONFIG, OFF_STATUS, clampOutline, normalizeLabelsConfig, statusKind } from "./labels";

describe("normalizeLabelsConfig", () => {
  it("matches the Rust defaults for an empty file", () => {
    expect(normalizeLabelsConfig({})).toEqual(DEFAULT_LABELS_CONFIG);
    expect(DEFAULT_LABELS_CONFIG).toEqual({
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
    });
  });

  it("clamps numbers and ignores wrong types", () => {
    const c = normalizeLabelsConfig({
      mode: "custom",
      outline: "yes",
      custom: { color: "#FF69B4", outline_color: "abc", outline_width: 9, shadow_alpha: 300, shadow_dx: -20, shadow_dy: 3.6, backdrop_color: 5, backdrop_alpha: -1 },
    });
    expect(c.mode).toBe("custom");
    expect(c.outline).toBe(true);
    expect(c.custom.color).toBe("#ff69b4");
    expect(c.custom.outline_color).toBe("#aabbcc");
    expect(c.custom.outline_width).toBe(4);
    expect(c.custom.shadow_alpha).toBe(100);
    expect(c.custom.shadow_dx).toBe(-6);
    expect(c.custom.shadow_dy).toBe(4);
    expect(c.custom.backdrop_color).toBe("#000000");
    expect(c.custom.backdrop_alpha).toBe(0);
    expect(normalizeLabelsConfig({ mode: "rainbow", custom: [1] }).mode).toBe("auto");
  });

  it("clampOutline", () => {
    expect(clampOutline(9)).toBe(4);
    expect(clampOutline(-3)).toBe(0);
    expect(clampOutline(2.4)).toBe(2);
    expect(clampOutline("x")).toBe(1);
  });
});

describe("statusKind", () => {
  it("orders restart > fallback > dll > off", () => {
    expect(statusKind(OFF_STATUS)).toBe("off");
    expect(statusKind({ ...OFF_STATUS, method: "dll" })).toBe("dll");
    expect(statusKind({ ...OFF_STATUS, method: "fallback" })).toBe("fallback");
    expect(statusKind({ ...OFF_STATUS, method: "fallback", needsExplorerRestart: true })).toBe("restart");
  });
});
