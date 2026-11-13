import { describe, expect, it } from "vitest";
import {
  addWidget,
  autoHideBlocked,
  clockOptions,
  defaultTaskbarConfig,
  effectiveZones,
  kindOf,
  moveWidget,
  normalizeTaskbarConfig,
  removeWidget,
  setWidgetOptions,
  type Zones,
} from "./taskbar";

const kinds = (z: Zones) => ({ left: z.left.map(kindOf), center: z.center.map(kindOf), right: z.right.map(kindOf) });

describe("normalizeTaskbarConfig", () => {
  it("empty → defaults of crates/sg-taskbar (TaskbarConfig::default)", () => {
    const c = normalizeTaskbarConfig({});
    expect(c).toEqual(defaultTaskbarConfig());
    expect(c).toMatchObject({
      edge: "bottom", height: 36, style: "docked", margin: 8, background: "#1c1c1c", opacity: 85, blur: "blur",
      accent: "#4cc2ff", font_family: "Segoe UI Variable", font_size: 12, text_color: "#ffffff",
      windows_button: "left", native: "reveal", hide_on_fullscreen: true, hot_corner: "none", all_monitors: true, auto_hide: false,
    });
    expect(kinds(c.widgets)).toEqual({
      left: ["windows_button", "search", "apps"],
      center: ["clock"],
      right: ["tray", "network", "volume", "battery", "notifications"],
    });
  });

  it("clamps numbers, fixes colours, ignores bad enums and unknown widgets", () => {
    const c = normalizeTaskbarConfig({
      height: 5000, margin: 99, font_size: 1, opacity: 250.4, background: "#ABC", accent: "#zzz", edge: "left", blur: "acrylic",
      hot_corner: "top_left", font_family: "  ", auto_hide: "yes",
      widgets: { left: ["clock", "does_not_exist", 7, { kind: "cpu", x: 1 }], right: ["clock", "tray"] },
    });
    expect(c).toMatchObject({ height: 64, margin: 32, font_size: 9, opacity: 100, background: "#aabbcc", accent: "#4cc2ff", edge: "bottom", blur: "acrylic", hot_corner: "top_left", font_family: "Segoe UI Variable", auto_hide: false });
    expect(kinds(c.widgets)).toEqual({ left: ["clock", "cpu"], center: ["clock"], right: ["tray"] });
    expect(c.widgets.left[1]).toEqual({ kind: "cpu", x: 1 });
  });

  it("hot_corner none / empty / junk", () => {
    expect(normalizeTaskbarConfig({ hot_corner: "none" }).hot_corner).toBe("none");
    expect(normalizeTaskbarConfig({ hot_corner: "" }).hot_corner).toBe("none");
    expect(normalizeTaskbarConfig({ hot_corner: "middle" }).hot_corner).toBe("none");
  });
});

describe("widget helpers", () => {
  const z = (): Zones => ({ left: ["a", "b", "c"], center: ["d"], right: [] });
  it("moveWidget reorders inside a zone and across zones", () => {
    const m = moveWidget(z(), { from: { zone: "left", index: 0 }, to: { zone: "right", index: 0 } });
    expect(m).toEqual({ left: ["b", "c"], center: ["d"], right: ["a"] });
    expect(moveWidget(z(), { from: { zone: "left", index: 0 }, to: { zone: "left", index: 2 } }).left).toEqual(["b", "a", "c"]);
    expect(moveWidget(z(), { from: { zone: "left", index: 2 }, to: { zone: "left", index: 0 } }).left).toEqual(["c", "a", "b"]);
    expect(moveWidget(z(), { from: { zone: "left", index: 0 }, to: { zone: "center", index: 1 } }).center).toEqual(["d", "a"]);
  });
  it("moveWidget with bad source returns the same zones and does not mutate", () => {
    const src = z();
    expect(moveWidget(src, { from: { zone: "right", index: 3 }, to: { zone: "left", index: 0 } })).toEqual(z());
    moveWidget(src, { from: { zone: "left", index: 0 }, to: { zone: "right", index: 0 } });
    expect(src).toEqual(z());
  });
  it("addWidget appends once, never duplicates", () => {
    const a = addWidget(z(), "center", "e");
    expect(a.center).toEqual(["d", "e"]);
    expect(addWidget(a, "right", "e")).toEqual(a);
    expect(addWidget(z(), "right", "a").right).toEqual([]);
  });
  it("removeWidget", () => {
    expect(removeWidget(z(), "left", 1).left).toEqual(["a", "c"]);
    expect(removeWidget(z(), "left", 9)).toEqual(z());
  });
  it("setWidgetOptions keeps kind, collapses to string when empty", () => {
    const a = setWidgetOptions(z(), "left", 0, { label: true });
    expect(a.left[0]).toEqual({ kind: "a", label: true });
    expect(setWidgetOptions(a, "left", 0, {}).left[0]).toBe("a");
  });
});

describe("effectiveZones / clock options", () => {
  it("windows button centre moves it to the first spot of the centre zone", () => {
    const c = defaultTaskbarConfig();
    c.windows_button = "center";
    expect(kinds(effectiveZones(c))).toMatchObject({ left: ["search", "apps"], center: ["windows_button", "clock"] });
  });
  it("clock options round trip through the label format", () => {
    expect(clockOptions({})).toEqual({ hour12: false, seconds: false });
    const o = clockOptions({ label: "{%I:%M:%S %p}<span>{alarm}</span>" });
    expect(o).toEqual({ hour12: true, seconds: true });
  });
});

describe("autoHideBlocked", () => {
  it("chỉ chặn khi taskbar gốc ở chế độ reveal và thanh ở cạnh dưới", () => {
    expect(autoHideBlocked({ native: "reveal", edge: "bottom" })).toBe(true);
    expect(autoHideBlocked({ native: "reveal", edge: "top" })).toBe(false);
    expect(autoHideBlocked({ native: "keep", edge: "bottom" })).toBe(false);
    expect(autoHideBlocked({ native: "hide", edge: "bottom" })).toBe(false);
  });
});
