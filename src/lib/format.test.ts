import { describe, expect, it } from "vitest";
import { formatMb, stamp } from "./format";

describe("format", () => {
  it("shows MB", () => {
    expect(formatMb(39_845_888)).toBe("38");
    expect(formatMb(null)).toBe("—");
    expect(formatMb(0)).toBe("0");
  });

  it("makes a file-name timestamp", () => {
    expect(stamp(new Date(2026, 10, 1, 9, 5))).toBe("20261101-0905");
  });
});
