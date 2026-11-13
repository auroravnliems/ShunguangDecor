// The 10 languages. Native names are not translated.
export interface LangInfo {
  code: string;
  short: string;
  native: string;
}

export const LANGS: LangInfo[] = [
  { code: "vi", short: "VI", native: "Tiếng Việt" },
  { code: "en", short: "EN", native: "English" },
  { code: "ru", short: "RU", native: "Русский" },
  { code: "zh-CN", short: "中", native: "简体中文" },
  { code: "ko", short: "한", native: "한국어" },
  { code: "ja", short: "日", native: "日本語" },
  { code: "de", short: "DE", native: "Deutsch" },
  { code: "es", short: "ES", native: "Español" },
  { code: "pt-BR", short: "PT", native: "Português (Brasil)" },
  { code: "fr", short: "FR", native: "Français" },
];

export function langInfo(code: string): LangInfo {
  return LANGS.find((l) => l.code === code) ?? LANGS[1];
}
