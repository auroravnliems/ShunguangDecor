import { describe, expect, it } from "vitest";
import { greetingKey, quoteKey } from "./greeting";

describe("greetingKey", () => {
  it("follows the hour of day", () => {
    const want: [number, string][] = [
      [4, "night"],
      [5, "morning"],
      [10, "morning"],
      [11, "noon"],
      [12, "noon"],
      [13, "afternoon"],
      [17, "afternoon"],
      [18, "evening"],
      [22, "evening"],
      [23, "night"],
      [0, "night"],
    ];
    for (const [h, k] of want) expect(greetingKey(h)).toBe(`home.greeting.${k}`);
  });
});

describe("quoteKey", () => {
  it("is stable within a day", () => {
    expect(quoteKey(new Date(2026, 10, 11, 8, 0))).toBe(quoteKey(new Date(2026, 10, 11, 23, 59)));
  });

  it("cycles through 5 quotes over 5 days", () => {
    const keys = new Set([11, 12, 13, 14, 15].map((d) => quoteKey(new Date(2026, 10, d, 12))));
    expect(keys).toEqual(new Set([1, 2, 3, 4, 5].map((i) => `home.quote.${i}`)));
  });
});
