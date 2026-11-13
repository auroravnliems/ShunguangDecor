"use client";
// Translation in React. All 10 files are preloaded so a language switch takes effect immediately.
import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import de from "@/locales/de.json";
import en from "@/locales/en.json";
import es from "@/locales/es.json";
import fr from "@/locales/fr.json";
import ja from "@/locales/ja.json";
import ko from "@/locales/ko.json";
import ptBR from "@/locales/pt-BR.json";
import ru from "@/locales/ru.json";
import vi from "@/locales/vi.json";
import zhCN from "@/locales/zh-CN.json";
import { textOf, translate, type Dict, type Vars } from "./i18n-core";
import type { I18nText } from "./types";

export const DICTS: Record<string, Dict> = {
  vi,
  en,
  ru,
  "zh-CN": zhCN,
  ko,
  ja,
  de,
  es,
  "pt-BR": ptBR,
  fr,
} as Record<string, Dict>;

interface I18n {
  lang: string;
  t: (key: string, vars?: Vars) => string;
  tt: (text: I18nText) => string;
}

const Ctx = createContext<I18n>({
  lang: "en",
  t: (k) => k,
  tt: (x) => x.key,
});

export function I18nProvider({ lang, children }: { lang: string; children: ReactNode }) {
  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);
  const value = useMemo<I18n>(
    () => ({
      lang,
      t: (key, vars) => translate(DICTS, lang, key, vars),
      tt: (text) => textOf(DICTS, lang, text),
    }),
    [lang],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useT(): I18n {
  return useContext(Ctx);
}
