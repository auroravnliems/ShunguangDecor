import { describe, expect, it } from "vitest";
import { BIGWORD, NAV_PAGES, RAIL, dotsShift, pageNo } from "./nav";

describe("nav", () => {
  it("has 6 menu pages in order", () => {
    expect(NAV_PAGES).toEqual(["home", "taskbar", "wallpaper", "cursor", "labels", "features"]);
  });

  it("formats the page number", () => {
    expect(pageNo("home")).toBe("01 / 06");
    expect(pageNo("features")).toBe("06 / 06");
    expect(pageNo("settings")).toBe("— / 06");
  });

  it("moves dots and rail per page", () => {
    expect(dotsShift("home")).toBe(-20);
    expect(dotsShift("taskbar")).toBe(20);
    expect(dotsShift("wallpaper")).toBe(60);
    expect(dotsShift("cursor")).toBe(-20);
    expect(dotsShift("settings")).toBe(-20);
    expect(RAIL.home).toEqual([300, 160]);
    expect(BIGWORD.labels).toBe("LABELS");
    expect(BIGWORD.settings).toBe("SETTINGS");
  });
});
