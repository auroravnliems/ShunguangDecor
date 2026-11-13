import { describe, expect, it } from "vitest";
import { hexToHsv, hsvToHex, normalizeHex } from "./color";

function channels(hex: string): number[] {
  return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
}

describe("color", () => {
  it("converts HSV to hex", () => {
    expect(hsvToHex(0, 1, 1)).toBe("#ff0000");
    expect(hsvToHex(120, 1, 1)).toBe("#00ff00");
    expect(hsvToHex(0, 0, 1)).toBe("#ffffff");
    expect(hsvToHex(0, 0, 0)).toBe("#000000");
    const got = channels(hsvToHex(210, 0.75, 0.91));
    const want = channels("#3a91e8"); // rgb(58,145,232)
    got.forEach((c, i) => expect(Math.abs(c - want[i])).toBeLessThanOrEqual(1));
  });

  it("round-trips the brand blue", () => {
    const hsv = hexToHsv("#3b7be9");
    expect(hsv).not.toBeNull();
    expect(hsvToHex(hsv!.h, hsv!.s, hsv!.v)).toBe("#3b7be9");
  });

  it("normalizes user input", () => {
    expect(normalizeHex("abc")).toBe("#aabbcc");
    expect(normalizeHex("#AABBCC")).toBe("#aabbcc");
    expect(normalizeHex(" #3b7be9 ")).toBe("#3b7be9");
    expect(normalizeHex("#GG0000")).toBeNull();
    expect(normalizeHex("#12345")).toBeNull();
    expect(hexToHsv("nope")).toBeNull();
  });
});
