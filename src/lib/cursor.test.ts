import { describe, expect, it } from "vitest";
import { cursorImageUrl, frameAt, normalizeCursorConfig, ROLES } from "./cursor";

describe("normalizeCursorConfig", () => {
  it("điền mặc định", () => {
    expect(normalizeCursorConfig({})).toEqual({ scheme: null, size: 32, scaling: "sharp" });
  });
  it("kẹp và làm tròn cỡ", () => {
    expect(normalizeCursorConfig({ size: 999 }).size).toBe(256);
    expect(normalizeCursorConfig({ size: "x" }).size).toBe(32);
    expect(normalizeCursorConfig({ size: 37 }).size).toBe(40);
    expect(normalizeCursorConfig({ size: -3 }).size).toBe(32);
    expect(normalizeCursorConfig({ size: 64 }).size).toBe(64);
  });
  it("đọc bộ và kiểu phóng, bỏ giá trị sai kiểu", () => {
    expect(normalizeCursorConfig({ scheme: " abc ", scaling: "smooth" })).toEqual({ scheme: "abc", size: 32, scaling: "smooth" });
    expect(normalizeCursorConfig({ scheme: 5, scaling: 7 })).toEqual({ scheme: null, size: 32, scaling: "sharp" });
  });
});

describe("frameAt", () => {
  it("chọn khung theo thời gian, lặp vòng", () => {
    expect(frameAt([100, 200, 100], 250)).toBe(1);
    expect(frameAt([100, 200, 100], 0)).toBe(0);
    expect(frameAt([100, 200, 100], 99)).toBe(0);
    expect(frameAt([100, 200, 100], 100)).toBe(1);
    expect(frameAt([100, 200, 100], 350)).toBe(2);
    expect(frameAt([100, 200, 100], 400)).toBe(0);
    expect(frameAt([100, 200, 100], 1050)).toBe(1);
  });
  it("tĩnh hoặc dữ liệu xấu thì khung 0", () => {
    expect(frameAt([], 50)).toBe(0);
    expect(frameAt([16], 5000)).toBe(0);
    expect(frameAt([0, 0], 5)).toBe(0);
  });
});

describe("cursorImageUrl", () => {
  it("ghép đường dẫn sg://", () => {
    expect(cursorImageUrl("http://sg.localhost", "a-1", "Hand", "sheet", 3)).toBe("http://sg.localhost/cursor/a-1/Hand/sheet.png?v=3");
    expect(cursorImageUrl("http://sg.localhost", "a-1", "Hand", 2)).toBe("http://sg.localhost/cursor/a-1/Hand/2.png");
    expect(ROLES.length).toBe(17);
  });
});
