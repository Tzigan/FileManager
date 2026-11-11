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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_known_key() {
        let i = I18n::new(Language::En);
        assert_ne!(i.t("menu.file"), "menu.file");
    }

    #[test]
    fn missing_key_returns_key() {
        let i = I18n::new(Language::Ru);
        assert_eq!(i.t("no.such.key"), "no.such.key");
    }

    #[test]
    fn switching_language_changes_existing_translation() {
        let mut i = I18n::new(Language::Ru);
        let russian = i.t("menu.file").to_owned();
        i.set_language(Language::En);
        let english = i.t("menu.file").to_owned();
        assert_ne!(russian, english);
        assert_eq!(i.language(), Language::En);
        assert_eq!(Language::Ru.label(), "RU");
        assert_eq!(Language::En.label(), "EN");
    }

    #[test]
    fn new_folder_context_actions_are_localized_in_both_languages() {
        let ru = I18n::new(Language::Ru);
        assert_eq!(ru.t("ctx.open_terminal"), "Открыть терминал здесь");
        assert_eq!(ru.t("ctx.open_new_tab"), "Открыть в новой вкладке");
        assert_eq!(ru.t("ctx.add_bookmark"), "Добавить в закладки");
        assert_eq!(ru.t("tree.expand"), "Развернуть");
        assert_eq!(ru.t("tree.collapse"), "Свернуть");
        assert_eq!(ru.t("menu.settings"), "Настройки");
        assert_eq!(ru.t("settings.category.general"), "Общие");
        assert_eq!(
            ru.t("settings.single_instance"),
            "Один экземпляр приложения"
        );
        assert_eq!(
            ru.t("settings.single_instance_hint"),
            "Повторный запуск откроет папку в уже запущенном окне. Выключите настройку, чтобы разрешить несколько окон."
        );

        let en = I18n::new(Language::En);
        assert_eq!(en.t("ctx.open_terminal"), "Open Terminal Here");
        assert_eq!(en.t("ctx.open_new_tab"), "Open in new tab");
        assert_eq!(en.t("ctx.add_bookmark"), "Add to bookmarks");
        assert_eq!(en.t("tree.expand"), "Expand");
        assert_eq!(en.t("tree.collapse"), "Collapse");
        assert_eq!(en.t("menu.settings"), "Settings");
        assert_eq!(en.t("settings.category.general"), "General");
        assert_eq!(en.t("settings.single_instance"), "Single instance");
        assert_eq!(
            en.t("settings.single_instance_hint"),
            "New launches open folders in the existing window. Turn this off to allow multiple windows."
        );
        assert_eq!(
            ru.t("settings.auto_restart_after_update"),
            "Перезапускать после обновления"
        );
        assert_eq!(
            ru.t("update.restarting"),
            "Обновление установлено. Перезапуск..."
        );
        assert_eq!(
            en.t("settings.auto_restart_after_update"),
            "Restart after update"
        );
        assert_eq!(en.t("update.restarting"), "Update installed. Restarting...");
        assert_eq!(ru.t("zip.extract_to"), "Извлечь в...");
        assert_eq!(en.t("zip.extract_to"), "Extract to...");
        assert_eq!(
            ru.t("update.busy_ops"),
            "Сначала дождитесь окончания копирования или распаковки либо отмените операцию. Установка не начата."
        );
    }

    #[test]
    fn russian_table_falls_back_to_english_for_a_missing_translation() {
        let i = I18n {
            lang: Language::Ru,
            ru: HashMap::new(),
            en: HashMap::from([("fallback".to_owned(), "English text".to_owned())]),
        };
        assert_eq!(i.t("fallback"), "English text");
    }
}
