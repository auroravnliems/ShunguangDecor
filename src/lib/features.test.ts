import { describe, expect, it } from "vitest";
import type { I18nText } from "./types";
import {
  adminElevated,
  changedCount,
  clampVariant,
  EDITABLE_PRIORITIES,
  pageCount,
  parsePageInput,
  parseFeatureQuery,
  stateLabelKey,
  undoTargetCount,
} from "./features";

const d = (k: string, v: I18nText): [I18nText, I18nText] => [{ key: k }, v];

describe("undoTargetCount", () => {
  it("counts unique store+id+priority, not journal lines", () => {
    // Turning a suggestion card (2 stores) on, off, and on again = 6 journal lines but only 2 configurations.
    const e = (id: number, store: "runtime" | "boot", priority: "user" | "service" = "user") => ({ id, store, priority });
    const list = [e(1, "runtime"), e(1, "boot"), e(1, "runtime"), e(1, "boot"), e(1, "runtime"), e(1, "boot")];
    expect(undoTargetCount(list)).toBe(2);
    expect(undoTargetCount([e(1, "runtime"), e(1, "runtime", "service"), e(2, "runtime")])).toBe(3);
    expect(undoTargetCount([])).toBe(0);
  });
});

describe("stateLabelKey", () => {
  it("maps each state to a locale key", () => {
    expect(stateLabelKey("enabled")).toBe("features.state.enabled");
    expect(stateLabelKey("disabled")).toBe("features.state.disabled");
    expect(stateLabelKey("default")).toBe("features.state.default");
  });
});

describe("parseFeatureQuery", () => {
  it("number → id", () => expect(parseFeatureQuery("56192424")).toEqual({ kind: "id", id: 56192424 }));
  it("trims and accepts 0x hex", () => {
    expect(parseFeatureQuery("  42 ")).toEqual({ kind: "id", id: 42 });
    expect(parseFeatureQuery("0x10")).toEqual({ kind: "id", id: 16 });
  });
  it("text → name", () => expect(parseFeatureQuery("camera")).toEqual({ kind: "name", text: "camera" }));
  it("blank → empty", () => expect(parseFeatureQuery("   ")).toEqual({ kind: "empty" }));
  it("zero or beyond u32 is not an id", () => {
    expect(parseFeatureQuery("0").kind).toBe("name");
    expect(parseFeatureQuery("4294967296").kind).toBe("name");
  });
});

describe("adminElevated / changedCount", () => {
  it("reads the admin detail from Rust", () => {
    expect(adminElevated([d("features.status.admin", { key: "features.val.yes" })])).toBe(true);
    expect(adminElevated([d("features.status.admin", { key: "features.val.not_yet" })])).toBe(false);
    expect(adminElevated([])).toBe(false);
  });
  it("reads the changed count", () => {
    expect(changedCount([d("features.status.changed", { key: "features.val.n_features", args: { n: "3" } })])).toBe(3);
    expect(changedCount([])).toBe(0);
  });
});

describe("clampVariant / priorities", () => {
  it("clamps to 0..63", () => {
    expect(clampVariant(-4)).toBe(0);
    expect(clampVariant(99)).toBe(63);
    expect(clampVariant(7.6)).toBe(8);
    expect(clampVariant(Number.NaN)).toBe(0);
  });
  it("offers only writable priorities, User among them", () => {
    expect(EDITABLE_PRIORITIES).toContain("user");
    expect(EDITABLE_PRIORITIES).not.toContain("security");
    expect(EDITABLE_PRIORITIES).not.toContain("image_default");
  });
});

describe("phân trang tab Tất cả", () => {
  it("pageCount làm tròn lên và luôn ít nhất 1 trang", () => {
    expect(pageCount(0, 50)).toBe(1);
    expect(pageCount(1, 50)).toBe(1);
    expect(pageCount(50, 50)).toBe(1);
    expect(pageCount(51, 50)).toBe(2);
    expect(pageCount(4938, 50)).toBe(99);
    expect(pageCount(10, 0)).toBe(1);
  });
  it("parsePageInput: số 1-based trong khoảng → chỉ số 0-based, ngoài khoảng bị kẹp, rác → null", () => {
    expect(parsePageInput("1", 5)).toBe(0);
    expect(parsePageInput(" 3 ", 5)).toBe(2);
    expect(parsePageInput("99", 5)).toBe(4);
    expect(parsePageInput("0", 5)).toBe(0);
    expect(parsePageInput("-2", 5)).toBeNull();
    expect(parsePageInput("", 5)).toBeNull();
    expect(parsePageInput("abc", 5)).toBeNull();
    expect(parsePageInput("2.5", 5)).toBeNull();
  });
});
