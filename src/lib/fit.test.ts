import { describe, expect, it } from "vitest";
import { fitStage } from "./fit";

describe("fitStage", () => {
  it("is 1:1 at the design size", () => {
    expect(fitStage(1280, 760)).toEqual({ zoom: 1, width: 1280, height: 760 });
  });

  it("fills the minimum window without grey bars", () => {
    const f = fitStage(1100, 650);
    expect(f.zoom).toBeCloseTo(650 / 760, 4);
    expect(f.height).toBeCloseTo(760, 6);
    expect(f.width).toBeCloseTo(1100 / (650 / 760), 3);
  });

  it("scales up on big screens", () => {
    const f = fitStage(2560, 1440);
    expect(f.zoom).toBeCloseTo(1440 / 760, 4);
    expect(f.height).toBeCloseTo(760, 6);
  });

  it("survives a zero-size window", () => {
    expect(fitStage(0, 0).zoom).toBeGreaterThan(0);
  });
});
