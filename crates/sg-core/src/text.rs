//! Translatable text passed from the core to the UI: a key plus arguments. The UI translates it into the selected language.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct I18nText {
    pub key: String,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
}

impl I18nText {
    pub fn new(key: impl Into<String>) -> Self {
        I18nText { key: key.into(), args: BTreeMap::new() }
    }

    pub fn arg(mut self, name: impl Into<String>, value: impl ToString) -> Self {
        self.args.insert(name.into(), value.to_string());
        self
    }

    /// Translate right on the Rust side (tray menu, log).
    pub fn render(&self, lang: crate::i18n::Lang) -> String {
        let args: Vec<(&str, &str)> = self.args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        crate::i18n::t(lang, &self.key, &args)
    }
}

impl fmt::Display for I18nText {
    /// Log form: key plus arguments, untranslated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.key)?;
        for (k, v) in &self.args {
            write!(f, " {k}={v}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i18n_text_serializes() {
        let t = I18nText::new("err.x").arg("n", "1");
        assert_eq!(serde_json::to_string(&t).unwrap(), r#"{"key":"err.x","args":{"n":"1"}}"#);
        let back: I18nText = serde_json::from_str(r#"{"key":"a"}"#).unwrap();
        assert_eq!(back, I18nText::new("a"));
    }
}
