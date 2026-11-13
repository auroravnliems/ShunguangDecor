import { describe, expect, it } from "vitest";
import { textOf, translate } from "./i18n-core";

const dicts = {
  vi: { "a.hello": "Xin chào {name}", "a.files_other": "{count} tệp" },
  en: { "a.hello": "Hello {name}", "a.only_en": "Only EN", "a.files_one": "{count} file", "a.files_other": "{count} files" },
  ru: {
    "a.files_one": "{count} файл",
    "a.files_few": "{count} файла",
    "a.files_many": "{count} файлов",
    "a.files_other": "{count} файла",
  },
};

describe("translate", () => {
  it("falls back lang → en → key", () => {
    expect(translate(dicts, "vi", "a.hello", { name: "An" })).toBe("Xin chào An");
    expect(translate(dicts, "vi", "a.only_en")).toBe("Only EN");
    expect(translate(dicts, "vi", "khong.co")).toBe("khong.co");
    expect(translate(dicts, "de", "a.hello", { name: "Jo" })).toBe("Hello Jo");
  });

  it("keeps unknown placeholders", () => {
    expect(translate(dicts, "en", "a.hello")).toBe("Hello {name}");
  });

  it("picks Russian plural forms", () => {
    expect(translate(dicts, "ru", "a.files", { count: 1 })).toBe("1 файл");
    expect(translate(dicts, "ru", "a.files", { count: 2 })).toBe("2 файла");
    expect(translate(dicts, "ru", "a.files", { count: 5 })).toBe("5 файлов");
    expect(translate(dicts, "ru", "a.files", { count: 21 })).toBe("21 файл");
  });

  it("uses _other when the language has no _one", () => {
    expect(translate(dicts, "vi", "a.files", { count: 1 })).toBe("1 tệp");
    expect(translate(dicts, "en", "a.files", { count: 1 })).toBe("1 file");
    expect(translate(dicts, "en", "a.files", { count: 3 })).toBe("3 files");
  });

  it("translates I18nText from Rust", () => {
    expect(textOf(dicts, "vi", { key: "a.hello", args: { name: "Bé" } })).toBe("Xin chào Bé");
  });
});
