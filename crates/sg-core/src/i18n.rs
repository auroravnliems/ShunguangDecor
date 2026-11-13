//! UI languages.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// The 10 UI languages, in spec order. The code is used as the file name `src/locales/<code>.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "vi")]
    Vi,
    #[serde(rename = "en")]
    En,
    #[serde(rename = "ru")]
    Ru,
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "ko")]
    Ko,
    #[serde(rename = "ja")]
    Ja,
    #[serde(rename = "de")]
    De,
    #[serde(rename = "es")]
    Es,
    #[serde(rename = "pt-BR")]
    PtBr,
    #[serde(rename = "fr")]
    Fr,
}

impl Lang {
    pub const ALL: [Lang; 10] = [
        Lang::Vi,
        Lang::En,
        Lang::Ru,
        Lang::ZhCn,
        Lang::Ko,
        Lang::Ja,
        Lang::De,
        Lang::Es,
        Lang::PtBr,
        Lang::Fr,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Lang::Vi => "vi",
            Lang::En => "en",
            Lang::Ru => "ru",
            Lang::ZhCn => "zh-CN",
            Lang::Ko => "ko",
            Lang::Ja => "ja",
            Lang::De => "de",
            Lang::Es => "es",
            Lang::PtBr => "pt-BR",
            Lang::Fr => "fr",
        }
    }

    /// Exactly one of the 10 codes, case-insensitive.
    pub fn from_code(s: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| l.code().eq_ignore_ascii_case(s.trim()))
    }
}

/// Picks the app language from the OS language (first launch). No match → English.
/// Accepts both Windows forms (`vi-VN`, `zh-Hant-TW`) and POSIX forms (`pt_BR.UTF-8`, `sr@latin`).
pub fn pick_language(os_locale: &str) -> Lang {
    let base = os_locale.split(['.', '@']).next().unwrap_or("");
    let primary = base.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "zh" => Lang::ZhCn,
        "pt" => Lang::PtBr,
        p => Lang::ALL.into_iter().find(|l| l.code() == p).unwrap_or(Lang::En),
    }
}

/// The OS language; an empty string if it cannot be read.
pub fn os_locale() -> String {
    sys_locale::get_locale().unwrap_or_default()
}

type Dict = HashMap<String, String>;

/// The core embeds the same translation files as the UI, used for the tray menu and dialogs.
fn raw(lang: Lang) -> &'static str {
    match lang {
        Lang::Vi => include_str!("../../../src/locales/vi.json"),
        Lang::En => include_str!("../../../src/locales/en.json"),
        Lang::Ru => include_str!("../../../src/locales/ru.json"),
        Lang::ZhCn => include_str!("../../../src/locales/zh-CN.json"),
        Lang::Ko => include_str!("../../../src/locales/ko.json"),
        Lang::Ja => include_str!("../../../src/locales/ja.json"),
        Lang::De => include_str!("../../../src/locales/de.json"),
        Lang::Es => include_str!("../../../src/locales/es.json"),
        Lang::PtBr => include_str!("../../../src/locales/pt-BR.json"),
        Lang::Fr => include_str!("../../../src/locales/fr.json"),
    }
}

fn dict(lang: Lang) -> Option<&'static Dict> {
    static DICTS: OnceLock<HashMap<Lang, Option<Dict>>> = OnceLock::new();
    DICTS
        .get_or_init(|| {
            Lang::ALL
                .into_iter()
                .map(|l| (l, serde_json::from_str::<Dict>(crate::config::strip_bom(raw(l))).ok()))
                .collect()
        })
        .get(&lang)
        .and_then(Option::as_ref)
}

/// Translates one key: selected language → English → the key itself. Substitutes `{name}` variables.
pub fn t(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    let s = dict(lang)
        .and_then(|d| d.get(key))
        .or_else(|| dict(Lang::En).and_then(|d| d.get(key)))
        .map(String::as_str)
        .unwrap_or(key);
    let mut out = s.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_serde_codes() {
        assert_eq!(serde_json::to_string(&Lang::ZhCn).unwrap(), "\"zh-CN\"");
        assert_eq!(serde_json::to_string(&Lang::PtBr).unwrap(), "\"pt-BR\"");
        assert_eq!(Lang::from_code("PT-br"), Some(Lang::PtBr));
        assert_eq!(Lang::from_code("xx"), None);
        let codes: Vec<&str> = Lang::ALL.iter().map(|l| l.code()).collect();
        assert_eq!(codes, ["vi", "en", "ru", "zh-CN", "ko", "ja", "de", "es", "pt-BR", "fr"]);
    }

    #[test]
    fn pick_language_cases() {
        let cases = [
            ("vi-VN", Lang::Vi),
            ("vi", Lang::Vi),
            ("en-GB", Lang::En),
            ("zh-Hant-TW", Lang::ZhCn),
            ("zh_CN", Lang::ZhCn),
            ("pt-PT", Lang::PtBr),
            ("pt_BR.UTF-8", Lang::PtBr),
            ("ja_JP", Lang::Ja),
            ("ko-KR", Lang::Ko),
            ("de-AT", Lang::De),
            ("es-419", Lang::Es),
            ("fr-CA", Lang::Fr),
            ("ru", Lang::Ru),
            ("nl-NL", Lang::En),
            ("", Lang::En),
            ("C", Lang::En),
            ("C.UTF-8", Lang::En),
            ("POSIX", Lang::En),
            ("sr@latin", Lang::En),
        ];
        for (input, want) in cases {
            assert_eq!(pick_language(input), want, "{input:?}");
        }
    }

    #[test]
    fn t_substitutes_and_falls_back() {
        assert_eq!(t(Lang::Vi, "tray.quit", &[]), "Thoát");
        assert_eq!(t(Lang::En, "tray.quit", &[]), "Quit");
        assert_eq!(t(Lang::Ru, "tray.quit", &[]), "Выход");
        // A key that exists nowhere → returns the key itself (the English fallback is in the same `or_else` chain).
        assert_eq!(t(Lang::Ru, "khong.co", &[]), "khong.co");
        assert_eq!(t(Lang::Vi, "khong.co", &[]), "khong.co");
        assert_eq!(t(Lang::Vi, "tray.faulted_suffix", &[("name", "Con trỏ")]), "Con trỏ — gặp lỗi, đã tắt");
        // A variable that is not passed is left untouched.
        assert!(t(Lang::Vi, "tray.faulted_suffix", &[]).contains("{name}"));
    }

    #[test]
    fn all_locale_files_parse() {
        for l in Lang::ALL {
            assert!(dict(l).is_some(), "src/locales/{}.json phải là JSON phẳng khoá → chuỗi", l.code());
        }
    }
}
