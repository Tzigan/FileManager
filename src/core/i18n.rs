use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Ru,
    En,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Language::Ru => "RU",
            Language::En => "EN",
        }
    }
}

/// String-table based localization stub.
///
/// Strings live in `i18n/<lang>.json`, embedded at compile time and parsed once
/// at startup. Missing keys fall back to English, then to the key itself.
pub struct I18n {
    lang: Language,
    ru: HashMap<String, String>,
    en: HashMap<String, String>,
}

impl I18n {
    pub fn new(lang: Language) -> Self {
        let ru = load_table(include_str!("../../i18n/ru.json"));
        let en = load_table(include_str!("../../i18n/en.json"));
        Self { lang, ru, en }
    }

    pub fn language(&self) -> Language {
        self.lang
    }

    pub fn set_language(&mut self, lang: Language) {
        self.lang = lang;
    }

    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        let table = match self.lang {
            Language::Ru => &self.ru,
            Language::En => &self.en,
        };
        table
            .get(key)
            .or_else(|| self.en.get(key))
            .map(String::as_str)
            .unwrap_or(key)
    }
}

fn load_table(raw: &str) -> HashMap<String, String> {
    serde_json::from_str(raw).unwrap_or_else(|e| {
        tracing::warn!("failed to parse embedded i18n table: {e}");
        HashMap::new()
    })
}
