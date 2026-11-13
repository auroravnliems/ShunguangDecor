import { describe, expect, it } from "vitest";
import { needsDisableGuard } from "./moduleGuard";

describe("needsDisableGuard", () => {
  const guards = { features: () => null };
  it("asks only when turning a registered module off", () => {
    expect(needsDisableGuard(guards, "features", false)).toBe(true);
    expect(needsDisableGuard(guards, "features", true)).toBe(false);
    expect(needsDisableGuard(guards, "cursor", false)).toBe(false);
  });
});
