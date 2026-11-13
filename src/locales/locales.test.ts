// Giữ 10 file ngôn ngữ đồng bộ với vi.json: cùng khoá, đủ dạng số nhiều, cùng {biến}, không chuỗi rỗng.
import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

const DIR = __dirname;
const CODES = ["vi", "en", "ru", "zh-CN", "ko", "ja", "de", "es", "pt-BR", "fr"];
const PLURAL = /_(zero|one|two|few|many|other)$/;

type Dict = Record<string, string>;
const load = (code: string): Dict => JSON.parse(readFileSync(path.join(DIR, `${code}.json`), "utf-8"));
const dicts = Object.fromEntries(CODES.map((c) => [c, load(c)])) as Record<string, Dict>;
const base = (k: string) => k.replace(PLURAL, "");
const baseKeys = (d: Dict) => new Set(Object.keys(d).map(base));
const pluralBases = (d: Dict) => new Set(Object.keys(d).filter((k) => PLURAL.test(k)).map(base));
const holes = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]);
/** Tập {biến} của một khoá gốc, gộp mọi dạng số nhiều của nó. */
const holesOf = (d: Dict, b: string) =>
  [...new Set(Object.keys(d).filter((k) => base(k) === b).flatMap((k) => holes(d[k])))].sort();

const vi = dicts.vi;

describe("locales", () => {
  it("has exactly the 10 locale files", () => {
    const files = readdirSync(DIR).filter((f) => f.endsWith(".json")).map((f) => f.slice(0, -5));
    expect(files.sort()).toEqual([...CODES].sort());
  });

  it("every locale has the same base keys as vi", () => {
    const want = baseKeys(vi);
    const problems: string[] = [];
    for (const code of CODES) {
      const have = baseKeys(dicts[code]);
      const missing = [...want].filter((k) => !have.has(k));
      const extra = [...have].filter((k) => !want.has(k));
      if (missing.length) problems.push(`${code}.json thiếu: ${missing.join(", ")}`);
      if (extra.length) problems.push(`${code}.json thừa: ${extra.join(", ")}`);
    }
    expect(problems).toEqual([]);
  });

  it("plural keys have _other, and _one/_few/_many where the language needs them", () => {
    const problems: string[] = [];
    for (const code of CODES) {
      const d = dicts[code];
      const cats = new Intl.PluralRules(code).resolvedOptions().pluralCategories;
      // Bắt buộc: _other ở mọi thứ tiếng; _one nếu thứ tiếng có dạng "một"; ru đủ cả _few/_many.
      const need = ["other", ...(cats.includes("one") ? ["one"] : []), ...(code === "ru" ? ["few", "many"] : [])];
      for (const b of pluralBases(vi)) {
        for (const c of need) if (!(`${b}_${c}` in d)) problems.push(`${code}.json thiếu ${b}_${c}`);
      }
      // Khoá số nhiều thì không được để thêm bản không hậu tố (sẽ không bao giờ được dùng).
      for (const b of pluralBases(vi)) if (b in d) problems.push(`${code}.json có ${b} không hậu tố`);
    }
    expect(problems).toEqual([]);
  });

  it("placeholders match vi", () => {
    const problems: string[] = [];
    for (const code of CODES) {
      const d = dicts[code];
      for (const b of baseKeys(vi)) {
        const want = holesOf(vi, b).join(",");
        const have = holesOf(d, b).join(",");
        if (baseKeys(d).has(b) && want !== have) problems.push(`${code}.json ${b}: {${have}} ≠ vi {${want}}`);
      }
    }
    expect(problems).toEqual([]);
  });

  it("no empty strings", () => {
    const problems = CODES.flatMap((code) =>
      Object.entries(dicts[code])
        .filter(([, v]) => typeof v !== "string" || v.trim() === "")
        .map(([k]) => `${code}.json ${k}`),
    );
    expect(problems).toEqual([]);
  });
});
