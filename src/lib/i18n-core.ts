// Pure translation logic (no React) so it can be tested.
import type { I18nText } from "./types";

export type Dict = Record<string, string>;
export type Vars = Record<string, string | number>;

const PLURAL_SUFFIXES = ["zero", "one", "two", "few", "many", "other"] as const;

function lookup(dicts: Record<string, Dict>, lang: string, key: string): string | undefined {
  return dicts[lang]?.[key] ?? dicts.en?.[key];
}

function fill(s: string, vars?: Vars): string {
  if (!vars) return s;
  return s.replace(/\{(\w+)\}/g, (m, name: string) => (name in vars ? String(vars[name]) : m));
}

/**
 * Look up a key by language → English → the key itself.
 * With `vars.count`, picks the plural form `key_<one|few|many|other>` via `Intl.PluralRules`.
 */
export function translate(dicts: Record<string, Dict>, lang: string, key: string, vars?: Vars): string {
  if (vars && typeof vars.count === "number") {
    let cat: string = "other";
    try {
      cat = new Intl.PluralRules(lang).select(vars.count);
    } catch {
      /* unknown language code → use other */
    }
    const own = dicts[lang];
    // Prefer the language's own form; a language that only has _other (vi, ja…) uses _other.
    const s =
      own?.[`${key}_${cat}`] ??
      own?.[`${key}_other`] ??
      dicts.en?.[`${key}_${new Intl.PluralRules("en").select(vars.count)}`] ??
      dicts.en?.[`${key}_other`] ??
      lookup(dicts, lang, key);
    return fill(s ?? key, vars);
  }
  return fill(lookup(dicts, lang, key) ?? key, vars);
}

export function textOf(dicts: Record<string, Dict>, lang: string, t: I18nText): string {
  return translate(dicts, lang, t.key, t.args);
}

/** Strip the plural suffix so key sets can be compared across languages. */
export function baseKey(key: string): string {
  for (const s of PLURAL_SUFFIXES) {
    if (key.endsWith(`_${s}`)) return key.slice(0, -(s.length + 1));
  }
  return key;
}
