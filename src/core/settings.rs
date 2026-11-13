use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::core::i18n::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Куда попадает обычное удаление. Shift+Delete по-прежнему удаляет сразу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    /// Системная Корзина Windows. Значение по умолчанию, чтобы старое поведение не менялось.
    #[default]
    WindowsRecycle,
    /// Защищённая корзина приложения.
    SecureTrash,
    /// Спросить при каждом удалении.
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteRoute {
    pub windows: Vec<PathBuf>,
    pub secure: Vec<PathBuf>,
    pub ask: Vec<PathBuf>,
    pub permanent: bool,
}

/// Папка из списка захватывает путь только целиком по компонентам.
/// `C:\Work` включает `C:\Work\sub` и не включает `C:\Workplace`.
/// TODO(0.8): маски имён (`*.tmp`, `Work*`) не поддерживаются и здесь не добавляются.
pub fn path_is_within_folder(path: &Path, folder: &Path) -> bool {
    let folder_parts: Vec<_> = folder.components().collect();
    if folder_parts.is_empty() {
        return false;
    }
    let path_parts: Vec<_> = path.components().collect();
    if path_parts.len() < folder_parts.len() {
        return false;
    }
    path_parts
        .iter()
        .zip(folder_parts.iter())
        .all(|(path_part, folder_part)| {
            // Unicode-понижение, не ASCII-fold: иначе «Работа» и «РАБОТА» не совпадут.
            // Windows-API сравнения не используем — для имён достаточно to_lowercase.
            component_folded(*path_part) == component_folded(*folder_part)
        })
}

fn component_folded(component: std::path::Component<'_>) -> String {
    component.as_os_str().to_string_lossy().to_lowercase()
}

/// Где лежит сам удаляемый объект, без разрешения финальной ссылки.
///
/// Полная `fs::canonicalize` всего пути раскрыла бы symlink или junction,
/// и корзина выбиралась бы по цели ссылки, а не по папке, в которой ссылка стоит.
/// Родитель канонизируется: `..`, junction и symlink в предках по-прежнему не обходят список.
/// Восстановление здесь не участвует — там проверяется уже реальное место назначения.
/// Нет родителя (корень тома) или родитель не канонизируется — `None`, папка не применяется.
fn location_for_routing(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())?;
    let canonical_parent = fs::canonicalize(parent).ok()?;
    Some(canonical_parent.join(name))
}

/// Закладка «Корзина» открывает корзину приложения, когда удаление может в неё попасть.
/// В режиме «спрашивать» это она, а не системная корзина: иначе список приложения неоткуда открыть.
/// Системная корзина остаётся отдельным пунктом меню.
pub fn bookmark_opens_secure_trash(mode: DeleteMode) -> bool {
    matches!(mode, DeleteMode::SecureTrash | DeleteMode::Ask)
}

/// Папки из списка сравниваются по компонентам пути, не по сырой строке и не по маске.
pub fn route_delete(
    mode: DeleteMode,
    permanent: bool,
    paths: &[PathBuf],
    folders: &[PathBuf],
) -> DeleteRoute {
    if permanent {
        return DeleteRoute {
            windows: paths.to_vec(),
            secure: Vec::new(),
            ask: Vec::new(),
            permanent: true,
        };
    }
    let (forced, rest): (Vec<_>, Vec<_>) = paths.iter().cloned().partition(|path| {
        let Some(path) = location_for_routing(path) else {
            return false;
        };
        folders.iter().any(|folder| {
            // Папка из настройки канонизируется целиком: это корень правила, не удаляемый объект.
            fs::canonicalize(folder)
                .ok()
                .is_some_and(|folder| path_is_within_folder(&path, &folder))
        })
    });
    match mode {
        DeleteMode::SecureTrash => DeleteRoute {
            windows: Vec::new(),
            secure: paths.to_vec(),
            ask: Vec::new(),
            permanent: false,
        },
        DeleteMode::Ask => DeleteRoute {
            windows: Vec::new(),
            secure: forced,
            ask: rest,
            permanent: false,
        },
        DeleteMode::WindowsRecycle => DeleteRoute {
            windows: rest,
            secure: forced,
            ask: Vec::new(),
            permanent: false,
        },
    }
}

fn default_hash_max_bytes() -> u64 {
    crate::core::secure_trash::DEFAULT_HASH_MAX_BYTES
}

/// File list view: table with columns, compact list or icon grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    Table,
    List,
    Grid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub theme: Theme,
    /// Paths of open tabs, restored on startup.
    pub tabs: Vec<PathBuf>,
    /// Paths of pinned tabs (subset of `tabs`), restored on startup.
    pub pinned_tabs: Vec<PathBuf>,
    pub active_tab: usize,
    pub bookmarks: Vec<PathBuf>,
    pub show_hidden_files: bool,
    pub show_wsl_in_tree: bool,
    pub view_mode: ViewMode,
    /// Table-column visibility (name is always shown).
    #[serde(default = "default_true")]
    pub col_size: bool,
    #[serde(default = "default_true")]
    pub col_type: bool,
    #[serde(default = "default_true")]
    pub col_modified: bool,
    /// Table view: rows grouped by type (folders, then per-extension).
    pub group_by_type: bool,
    /// Optional user order of stable group keys (`folder`, `file`, `ext:txt`).
    pub group_order: Vec<String>,
    /// One running instance: new launches forward their path over the pipe.
    #[serde(default = "default_true")]
    pub single_instance: bool,
    /// Навигация по истории папок боковыми кнопками мыши.
    #[serde(default = "default_true")]
    pub enable_mouse_side_navigation: bool,
    /// После успешной установки updater сам запускает новую версию.
    #[serde(default = "default_true")]
    pub auto_restart_after_update: bool,
    /// Сколько секунд окно ещё остаётся открытым перед передачей установки updater.
    /// Пользователь успевает увидеть сообщение. Не задержка внутри updater.
    #[serde(default = "default_restart_delay_secs")]
    pub update_restart_delay_secs: u32,
    /// Режим обычного удаления. Не подменяет системную корзину, пока пользователь сам не выбрал.
    pub delete_mode: DeleteMode,
    /// Файлы больше порога не хешируются: удаление не должно вешать окно.
    #[serde(default = "default_hash_max_bytes")]
    pub secure_trash_hash_max_bytes: u64,
    /// Запасная корзина на системном диске, только если на своём томе нет места.
    pub secure_trash_allow_fallback: bool,
    /// Папки, для которых обычное удаление идёт в защищённую корзину даже в режиме Windows.
    pub secure_trash_folders: Vec<PathBuf>,
    /// Сколько последних папок держать в памяти, чтобы не читать их с диска снова.
    #[serde(default = "default_dir_cache_entries")]
    pub dir_cache_entries: usize,
    /// Следить за внешней правкой открытой папки. Выключено — только кнопка «Обновить».
    #[serde(default = "default_true")]
    pub dir_watch_enabled: bool,
    /// Пауза, после которой пачка внешних изменений становится одним чтением папки.
    #[serde(default = "default_dir_watch_debounce_ms")]
    pub dir_watch_debounce_ms: u64,
    /// Лимит одновременных поисков и ZIP-операций; файловые операции остаются последовательными.
    #[serde(default = "default_max_concurrent_bg_ops")]
    pub max_concurrent_bg_ops: usize,
}

fn default_max_concurrent_bg_ops() -> usize {
    crate::core::bg_ops::DEFAULT_CONCURRENT_BG_OPS
}

fn default_dir_watch_debounce_ms() -> u64 {
    crate::core::dir_watch::default_dir_watch_debounce_ms()
}

fn default_dir_cache_entries() -> usize {
    crate::core::dir_cache::default_dir_cache_entries()
}

fn default_true() -> bool {
    true
}

fn default_restart_delay_secs() -> u32 {
    3
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            theme: Theme::default(),
            tabs: Vec::new(),
            pinned_tabs: Vec::new(),
            active_tab: 0,
            bookmarks: Vec::new(),
            show_hidden_files: false,
            show_wsl_in_tree: true,
            view_mode: ViewMode::default(),
            col_size: true,
            col_type: true,
            col_modified: true,
            group_by_type: false,
            group_order: Vec::new(),
            single_instance: true,
            enable_mouse_side_navigation: true,
            auto_restart_after_update: true,
            update_restart_delay_secs: default_restart_delay_secs(),
            delete_mode: DeleteMode::WindowsRecycle,
            secure_trash_hash_max_bytes: default_hash_max_bytes(),
            secure_trash_allow_fallback: false,
            secure_trash_folders: Vec::new(),
            dir_cache_entries: default_dir_cache_entries(),
            dir_watch_enabled: true,
            dir_watch_debounce_ms: default_dir_watch_debounce_ms(),
            max_concurrent_bg_ops: default_max_concurrent_bg_ops(),
        }
    }
}

impl Settings {
    /// Сбрасывает поведение, сохраняя `tabs`, `pinned_tabs`, `active_tab`, `bookmarks`
    /// и `secure_trash_folders`. Эта граница фиксирована и не является переключателем.
    pub fn reset_behavior_preserving_user_data(&mut self) {
        let tabs = std::mem::take(&mut self.tabs);
        let pinned_tabs = std::mem::take(&mut self.pinned_tabs);
        let active_tab = self.active_tab;
        let bookmarks = std::mem::take(&mut self.bookmarks);
        let secure_trash_folders = std::mem::take(&mut self.secure_trash_folders);

        *self = Self::default();
        self.tabs = tabs;
        self.pinned_tabs = pinned_tabs;
        self.active_tab = active_tab;
        self.bookmarks = bookmarks;
        self.secure_trash_folders = secure_trash_folders;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsLoadIssue {
    Invalid,
    Unreadable,
}

impl SettingsLoadIssue {
    pub fn message_key(self) -> &'static str {
        match self {
            Self::Invalid => "settings.load_invalid",
            Self::Unreadable => "settings.load_failed",
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
    pub settings: Settings,
    pub load_issue: Option<SettingsLoadIssue>,
}

impl SettingsStore {
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join("settings.json");
        let mut load_issue = None;
        let settings = match fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<Settings>(&raw) {
                Ok(s) => {
                    tracing::info!(path = %path.display(), "settings loaded");
                    s
                }
                Err(e) => {
                    tracing::info!(path = %path.display(), error = %e, "settings parse failed, using defaults");
                    load_issue = Some(SettingsLoadIssue::Invalid);
                    Settings::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = %path.display(), "no settings file, using defaults");
                Settings::default()
            }
            Err(e) => {
                crate::core::logging::log_io_error("settings read", None, Some(&path), &e);
                load_issue = Some(SettingsLoadIssue::Unreadable);
                Settings::default()
            }
        };
        Self {
            path,
            settings,
            load_issue,
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let raw = serde_json::to_vec_pretty(&self.settings)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_path = parent.join(format!(
            ".settings.json.{}.{}.tmp",
            std::process::id(),
            stamp
        ));
        let mut temp = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("failed to create {}", temp_path.display()))?;
        if let Err(error) = temp
            .write_all(&raw)
            .and_then(|()| temp.sync_all())
        {
            drop(temp);
            let _ = fs::remove_file(&temp_path);
            return Err(error).with_context(|| {
                format!("failed to write temporary settings file {}", temp_path.display())
            });
        }
        drop(temp);
        if let Err(error) = crate::platform::replace_file(&temp_path, &self.path) {
            let _ = fs::remove_file(&temp_path);
            return Err(error).with_context(|| {
                format!("failed to replace settings file {}", self.path.display())
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_config() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("filemanager-settings-{n}"))
    }

    #[test]
    fn defaults_are_safe() {
        let s = Settings::default();
        assert!(s.tabs.is_empty() && s.active_tab == 0 && s.bookmarks.is_empty());
        assert!(!s.show_hidden_files);
        assert!(s.show_wsl_in_tree);
        assert!(s.enable_mouse_side_navigation);
        assert_eq!(
            s.max_concurrent_bg_ops,
            crate::core::bg_ops::DEFAULT_CONCURRENT_BG_OPS
        );
    }

    #[test]
    fn older_settings_without_background_limit_use_safe_default() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            settings.max_concurrent_bg_ops,
            crate::core::bg_ops::DEFAULT_CONCURRENT_BG_OPS
        );
    }

    #[test]
    fn settings_load_failure_reports_that_defaults_are_active() {
        let dir = temp_config();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), b"{ invalid json").unwrap();

        let store = SettingsStore::load(&dir);

        assert_eq!(store.load_issue, Some(SettingsLoadIssue::Invalid));
        assert_eq!(store.settings.language, Language::default());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_load_failure_message_mentions_user_data_not_loaded() {
        let keys = [
            SettingsLoadIssue::Invalid.message_key(),
            SettingsLoadIssue::Unreadable.message_key(),
        ];
        for json in [
            include_str!("../../i18n/en.json"),
            include_str!("../../i18n/ru.json"),
        ] {
            let labels: serde_json::Value = serde_json::from_str(json).unwrap();
            for key in keys {
                let message = labels.get(key).and_then(|value| value.as_str()).unwrap();
                assert!(
                    message.contains("bookmarks") || message.contains("закладки"),
                    "{key} must say bookmarks were not loaded"
                );
                assert!(
                    message.contains("tabs") || message.contains("вкладки"),
                    "{key} must say tabs were not loaded"
                );
                assert!(
                    message.contains("protected-folder") || message.contains("защищённой корзины"),
                    "{key} must say protected-folder rules were not loaded"
                );
                assert!(
                    message.contains("not loaded") || message.contains("не загружены"),
                    "{key} must explain that saved user data was not loaded"
                );
            }
        }
    }

    #[test]
    fn roundtrip_keeps_tabs_bookmarks() {
        let s = Settings {
            tabs: vec![PathBuf::from("C:\\a")],
            active_tab: 0,
            bookmarks: vec![PathBuf::from("D:\\b")],
            show_hidden_files: true,
            show_wsl_in_tree: false,
            ..Default::default()
        };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.tabs, s.tabs);
        assert_eq!(back.bookmarks, s.bookmarks);
        assert_eq!(back.show_hidden_files, s.show_hidden_files);
        assert_eq!(back.show_wsl_in_tree, s.show_wsl_in_tree);
        assert_eq!(back.language, s.language);
    }

    #[test]
    fn settings_store_persists_user_preferences_and_open_tabs() {
        let dir = temp_config();
        let mut store = SettingsStore::load(&dir);
        store.settings.tabs = vec![PathBuf::from("C:\\work"), PathBuf::from("D:\\data")];
        store.settings.active_tab = 1;
        store.settings.pinned_tabs = vec![PathBuf::from("C:\\work")];
        store.settings.bookmarks = vec![PathBuf::from("D:\\docs")];
        store.settings.show_hidden_files = true;
        store.settings.show_wsl_in_tree = false;
        store.settings.language = Language::En;
        store.settings.theme = Theme::Light;
        store.settings.view_mode = ViewMode::Grid;
        store.settings.col_size = false;
        store.settings.col_type = true;
        store.settings.col_modified = false;
        store.settings.group_by_type = true;
        store.settings.group_order = vec!["ext:txt".into(), "folder".into()];
        store.settings.single_instance = false;
        store.settings.enable_mouse_side_navigation = false;
        store.settings.auto_restart_after_update = false;
        store.settings.update_restart_delay_secs = 15;
        store.settings.max_concurrent_bg_ops = 4;
        store.save().unwrap();

        let loaded = SettingsStore::load(&dir);
        assert_eq!(loaded.settings.tabs, store.settings.tabs);
        assert_eq!(loaded.settings.active_tab, 1);
        assert_eq!(loaded.settings.pinned_tabs, vec![PathBuf::from("C:\\work")]);
        assert_eq!(loaded.settings.bookmarks, vec![PathBuf::from("D:\\docs")]);
        assert!(loaded.settings.show_hidden_files);
        assert!(!loaded.settings.show_wsl_in_tree);
        assert_eq!(loaded.settings.language, Language::En);
        assert_eq!(loaded.settings.theme, Theme::Light);
        assert_eq!(loaded.settings.view_mode, ViewMode::Grid);
        assert!(!loaded.settings.col_size);
        assert!(loaded.settings.col_type);
        assert!(!loaded.settings.col_modified);
        assert!(loaded.settings.group_by_type);
        assert_eq!(loaded.settings.group_order, vec!["ext:txt", "folder"]);
        assert!(!loaded.settings.single_instance);
        assert!(!loaded.settings.enable_mouse_side_navigation);
        assert!(!loaded.settings.auto_restart_after_update);
        assert_eq!(loaded.settings.update_restart_delay_secs, 15);
        assert_eq!(loaded.settings.max_concurrent_bg_ops, 4);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_settings_keep_existing_values_and_default_new_preferences() {
        let dir = temp_config();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"tabs":["C:\\work"],"active_tab":0,"bookmarks":["D:\\docs"]}"#,
        )
        .unwrap();

        let loaded = SettingsStore::load(&dir);
        assert_eq!(loaded.settings.tabs, vec![PathBuf::from("C:\\work")]);
        assert_eq!(loaded.settings.bookmarks, vec![PathBuf::from("D:\\docs")]);
        assert!(!loaded.settings.show_hidden_files);
        assert!(loaded.settings.show_wsl_in_tree);
        assert_eq!(loaded.settings.view_mode, ViewMode::Table);
        assert!(
            loaded.settings.col_size && loaded.settings.col_type && loaded.settings.col_modified
        );
        assert!(!loaded.settings.group_by_type);
        assert!(loaded.settings.group_order.is_empty());
        assert!(loaded.settings.single_instance);
        assert!(loaded.settings.enable_mouse_side_navigation);
        assert!(loaded.settings.auto_restart_after_update);
        assert_eq!(loaded.settings.update_restart_delay_secs, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_settings_fall_back_to_safe_defaults() {
        let dir = temp_config();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{ definitely not json").unwrap();

        let loaded = SettingsStore::load(&dir);
        assert!(loaded.settings.tabs.is_empty());
        assert_eq!(loaded.settings.active_tab, 0);
        assert!(loaded.settings.single_instance);
        assert_eq!(loaded.settings.view_mode, ViewMode::Table);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_reset_preserves_user_bookmarks_and_tabs() {
        let mut settings = Settings {
            tabs: vec![PathBuf::from("C:\\work"), PathBuf::from("D:\\data")],
            pinned_tabs: vec![PathBuf::from("C:\\work")],
            active_tab: 1,
            bookmarks: vec![PathBuf::from("D:\\docs")],
            show_hidden_files: true,
            ..Settings::default()
        };
        let tabs = settings.tabs.clone();
        let pinned_tabs = settings.pinned_tabs.clone();
        let active_tab = settings.active_tab;
        let bookmarks = settings.bookmarks.clone();

        settings.reset_behavior_preserving_user_data();

        assert_eq!(settings.tabs, tabs);
        assert_eq!(settings.pinned_tabs, pinned_tabs);
        assert_eq!(settings.active_tab, active_tab);
        assert_eq!(settings.bookmarks, bookmarks);
        assert!(!settings.show_hidden_files);
    }

    #[test]
    fn settings_reset_preserves_secure_trash_folders() {
        let folders = vec![PathBuf::from("C:\\private"), PathBuf::from("D:\\vault")];
        let mut settings = Settings {
            delete_mode: DeleteMode::SecureTrash,
            secure_trash_hash_max_bytes: 1,
            secure_trash_allow_fallback: true,
            secure_trash_folders: folders.clone(),
            ..Settings::default()
        };

        settings.reset_behavior_preserving_user_data();

        assert_eq!(settings.delete_mode, DeleteMode::WindowsRecycle);
        assert_eq!(settings.secure_trash_hash_max_bytes, default_hash_max_bytes());
        assert!(!settings.secure_trash_allow_fallback);
        assert_eq!(settings.secure_trash_folders, folders);
    }

    #[test]
    fn settings_reset_clears_behavior_params_only() {
        let mut settings = Settings {
            language: Language::En,
            theme: Theme::Dark,
            tabs: vec![PathBuf::from("C:\\work")],
            pinned_tabs: vec![PathBuf::from("C:\\work")],
            active_tab: 0,
            bookmarks: vec![PathBuf::from("D:\\docs")],
            show_hidden_files: true,
            show_wsl_in_tree: false,
            view_mode: ViewMode::Grid,
            col_size: false,
            col_type: false,
            col_modified: false,
            group_by_type: true,
            group_order: vec!["ext:txt".to_owned()],
            single_instance: false,
            enable_mouse_side_navigation: false,
            auto_restart_after_update: false,
            update_restart_delay_secs: 42,
            delete_mode: DeleteMode::Ask,
            secure_trash_hash_max_bytes: 1,
            secure_trash_allow_fallback: true,
            secure_trash_folders: vec![PathBuf::from("D:\\protected")],
            dir_cache_entries: 1,
            dir_watch_enabled: false,
            dir_watch_debounce_ms: 1,
            max_concurrent_bg_ops: 1,
        };
        let mut expected = Settings::default();
        expected.tabs = settings.tabs.clone();
        expected.pinned_tabs = settings.pinned_tabs.clone();
        expected.active_tab = settings.active_tab;
        expected.bookmarks = settings.bookmarks.clone();
        expected.secure_trash_folders = settings.secure_trash_folders.clone();

        settings.reset_behavior_preserving_user_data();

        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }

    #[test]
    fn settings_reset_does_not_touch_files_or_trash() {
        let dir = temp_config();
        fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("user-file.txt");
        fs::write(&marker, "keep").unwrap();
        let mut settings = Settings {
            secure_trash_folders: vec![dir.clone()],
            delete_mode: DeleteMode::SecureTrash,
            ..Settings::default()
        };

        settings.reset_behavior_preserving_user_data();

        assert_eq!(fs::read_to_string(&marker).unwrap(), "keep");
        assert_eq!(settings.secure_trash_folders, vec![dir.clone()]);
        assert_eq!(settings.delete_mode, DeleteMode::WindowsRecycle);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_reset_handles_settings_load_issue() {
        let dir = temp_config();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), "{ invalid json").unwrap();
        let mut store = SettingsStore::load(&dir);
        assert_eq!(store.load_issue, Some(SettingsLoadIssue::Invalid));

        store.settings.reset_behavior_preserving_user_data();

        assert_eq!(store.load_issue, Some(SettingsLoadIssue::Invalid));
        assert_eq!(
            serde_json::to_value(&store.settings).unwrap(),
            serde_json::to_value(Settings::default()).unwrap()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_reset_handles_unreadable_settings_load_issue() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = temp_config();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut locked_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0x0000_0002 | 0x0000_0004)
            .open(&path)
            .unwrap();
        locked_file.write_all(b"{ valid json }").unwrap();
        locked_file.flush().unwrap();

        let mut store = SettingsStore::load(&dir);
        assert_eq!(store.load_issue, Some(SettingsLoadIssue::Unreadable));
        store.settings.reset_behavior_preserving_user_data();

        assert_eq!(store.load_issue, Some(SettingsLoadIssue::Unreadable));
        assert_eq!(
            serde_json::to_value(&store.settings).unwrap(),
            serde_json::to_value(Settings::default()).unwrap()
        );
        drop(locked_file);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_structure_and_json_schema_unchanged() {
        let settings = serde_json::to_value(Settings::default()).unwrap();
        let mut actual: Vec<_> = settings.as_object().unwrap().keys().map(String::as_str).collect();
        let mut expected = vec![
            "active_tab",
            "auto_restart_after_update",
            "bookmarks",
            "col_modified",
            "col_size",
            "col_type",
            "delete_mode",
            "dir_cache_entries",
            "dir_watch_debounce_ms",
            "dir_watch_enabled",
            "enable_mouse_side_navigation",
            "group_by_type",
            "group_order",
            "language",
            "max_concurrent_bg_ops",
            "pinned_tabs",
            "secure_trash_allow_fallback",
            "secure_trash_folders",
            "secure_trash_hash_max_bytes",
            "show_hidden_files",
            "show_wsl_in_tree",
            "single_instance",
            "tabs",
            "theme",
            "update_restart_delay_secs",
            "view_mode",
        ];
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn settings_reset_boundary_not_configurable_flag() {
        let settings = serde_json::to_value(Settings::default()).unwrap();
        let fields = settings.as_object().unwrap();
        assert!(!fields.keys().any(|field| field.starts_with("reset_")));
    }

    #[test]
    fn settings_store_atomically_replaces_existing_file() {
        let dir = temp_config();
        let mut store = SettingsStore::load(&dir);
        store.settings.show_hidden_files = true;
        store.save().unwrap();
        store.settings.show_hidden_files = false;
        store.save().unwrap();

        let loaded = SettingsStore::load(&dir);
        assert!(!loaded.settings.show_hidden_files);
        assert_eq!(loaded.load_issue, None);
        assert!(
            fs::read_dir(&dir)
                .unwrap()
                .all(|entry| !entry.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_reset_save_failure_states_session_only() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = temp_config();
        let mut store = SettingsStore::load(&dir);
        store.settings.show_hidden_files = true;
        store.settings.tabs = vec![PathBuf::from("C:\\work")];
        store.save().unwrap();
        let settings_path = dir.join("settings.json");
        let previous_file = fs::read(&settings_path).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0x0000_0001 | 0x0000_0002)
            .open(&settings_path)
            .unwrap();

        store.settings.reset_behavior_preserving_user_data();
        let save_result = store.save();

        assert!(save_result.is_err());
        assert!(!store.settings.show_hidden_files);
        assert_eq!(store.settings.tabs, vec![PathBuf::from("C:\\work")]);
        drop(lock);
        assert_eq!(fs::read(&settings_path).unwrap(), previous_file);
        for json in [
            include_str!("../../i18n/en.json"),
            include_str!("../../i18n/ru.json"),
        ] {
            let labels: serde_json::Value = serde_json::from_str(json).unwrap();
            let message = labels["settings.reset_session_only"].as_str().unwrap();
            assert!(message.contains("restart") || message.contains("перезапуска"));
            assert!(message.contains("session") || message.contains("сессии"));
        }
        let _ = fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests_v06 {
    use super::*;

    #[test]
    fn view_mode_and_pinned_roundtrip() {
        let mut s = Settings::default();
        s.view_mode = ViewMode::Grid;
        s.pinned_tabs = vec![PathBuf::from("C:\\p")];
        let raw = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.view_mode, ViewMode::Grid);
        assert_eq!(back.pinned_tabs, vec![PathBuf::from("C:\\p")]);
    }

    #[test]
    fn old_settings_without_new_fields_default() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.view_mode, ViewMode::Table);
        assert!(s.pinned_tabs.is_empty());
        assert!(s.auto_restart_after_update);
        assert_eq!(s.update_restart_delay_secs, 3);
        assert_eq!(s.delete_mode, DeleteMode::WindowsRecycle);
        assert!(s.secure_trash_folders.is_empty());
    }

    #[test]
    fn delete_mode_routes_without_replacing_windows_recycle() {
        let file = PathBuf::from(r"C:\work\a.txt");
        let dir = std::env::temp_dir().join(format!("fm-route-{}", std::process::id()));
        let private_dir = dir.join("private");
        fs::create_dir_all(&private_dir).unwrap();
        let forced = private_dir.join("a.txt");
        fs::write(&forced, "a").unwrap();
        let folders = vec![private_dir];
        let windows = route_delete(DeleteMode::WindowsRecycle, false, &[file.clone()], &[]);
        assert_eq!(windows.windows, vec![file.clone()]);
        assert!(windows.secure.is_empty());
        let secure = route_delete(DeleteMode::SecureTrash, false, &[file.clone()], &[]);
        assert_eq!(secure.secure, vec![file.clone()]);
        let asked = route_delete(
            DeleteMode::Ask,
            false,
            &[file.clone(), forced.clone()],
            &folders,
        );
        assert_eq!(asked.ask, vec![file.clone()]);
        assert_eq!(asked.secure, vec![forced]);
        let _ = fs::remove_dir_all(dir);
        let permanent = route_delete(DeleteMode::SecureTrash, true, &[file.clone()], &folders);
        assert!(permanent.permanent);
        assert!(permanent.secure.is_empty());
    }

    #[test]
    fn folder_rule_matches_components_not_a_string_prefix() {
        let folder = PathBuf::from(r"C:\Work");
        assert!(path_is_within_folder(
            &PathBuf::from(r"C:\Work\sub"),
            &folder
        ));
        assert!(path_is_within_folder(
            &PathBuf::from(r"c:\work\sub\a.txt"),
            &folder
        ));
        assert!(!path_is_within_folder(
            &PathBuf::from(r"C:\Workplace"),
            &folder
        ));
        assert!(!path_is_within_folder(&PathBuf::from(r"C:\Work2"), &folder));
        assert!(path_is_within_folder(
            &PathBuf::from(r"C:\Работа\файл.txt"),
            &PathBuf::from(r"C:\РАБОТА")
        ));
        assert!(path_is_within_folder(
            &PathBuf::from(r"C:\Straße\a"),
            &PathBuf::from(r"C:\STRAßE")
        ));
        assert!(!path_is_within_folder(
            &PathBuf::from(r"C:\Работник\a"),
            &PathBuf::from(r"C:\Работа")
        ));
    }

    #[test]
    fn parent_segment_and_missing_path_do_not_match_a_protected_folder() {
        let root = std::env::temp_dir().join(format!(
            "fm-canon-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let work = root.join("Work");
        let other = root.join("Other");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&other).unwrap();
        let outside = other.join("x.txt");
        fs::write(&outside, "x").unwrap();
        let via_parent = work.join("..").join("Other").join("x.txt");
        let routed = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[via_parent.clone()],
            &[work.clone()],
        );
        assert!(routed.secure.is_empty());
        assert_eq!(routed.windows, vec![via_parent]);

        // Файла ещё нет, но родитель Work существует: место объекта известно.
        let missing = work.join("gone.txt");
        let missing_route = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[missing.clone()],
            &[work.clone()],
        );
        assert_eq!(missing_route.secure, vec![missing]);
        // Родителя нет — не угадываем и защищённую папку не применяем.
        let missing_parent = work.join("absent").join("gone.txt");
        let unknown = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[missing_parent.clone()],
            &[work.clone()],
        );
        assert!(unknown.secure.is_empty());

        let inside = work.join("a.txt");
        fs::write(&inside, "a").unwrap();
        let link = root.join("Alias");
        let linked = std::os::windows::fs::symlink_dir(&work, &link).is_ok()
            || std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &work.to_string_lossy(),
                ])
                .status()
                .is_ok_and(|status| status.success());
        assert!(linked, "нужна точка соединения или ссылка на Work");
        let through_link = link.join("a.txt");
        let link_route = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[through_link.clone()],
            &[work],
        );
        assert_eq!(link_route.secure, vec![through_link]);
        let _ = fs::remove_dir(&link);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reparse_object_is_classified_by_where_it_sits() {
        let root_only = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[PathBuf::from(r"C:\")],
            &[PathBuf::from(r"C:\Work")],
        );
        assert!(root_only.secure.is_empty());

        let root = std::env::temp_dir().join(format!(
            "fm-reparse-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let work = root.join("Work");
        let other = root.join("Other");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(work.join("keep.txt"), "a").unwrap();
        fs::write(other.join("keep.txt"), "b").unwrap();

        let link_outside = other.join("to-work");
        let link_inside = work.join("to-other");
        assert!(
            place_directory_reparse(&link_outside, &work),
            "не удалось создать ссылку или точку соединения"
        );
        assert!(
            place_directory_reparse(&link_inside, &other),
            "не удалось создать ссылку или точку соединения"
        );
        let junction_outside = other.join("junction-to-work");
        let junction_inside = work.join("junction-to-other");
        assert!(place_junction(&junction_outside, &work));
        assert!(place_junction(&junction_inside, &other));

        let outside_route = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[link_outside.clone()],
            &[work.clone()],
        );
        assert!(
            outside_route.secure.is_empty(),
            "ссылка в Other не должна считаться объектом внутри Work"
        );

        let inside_route = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[link_inside.clone()],
            &[work.clone()],
        );
        assert_eq!(inside_route.secure, vec![link_inside.clone()]);

        let junction_outside_route = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[junction_outside.clone()],
            &[work.clone()],
        );
        assert!(junction_outside_route.secure.is_empty());
        let junction_inside_route = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[junction_inside.clone()],
            &[work.clone()],
        );
        assert_eq!(junction_inside_route.secure, vec![junction_inside.clone()]);

        let plain = route_delete(
            DeleteMode::WindowsRecycle,
            false,
            &[work.join("keep.txt"), other.join("keep.txt")],
            &[work.clone()],
        );
        assert_eq!(plain.secure, vec![work.join("keep.txt")]);
        assert_eq!(plain.windows, vec![other.join("keep.txt")]);

        let _ = fs::remove_dir(&link_outside);
        let _ = fs::remove_dir(&link_inside);
        let _ = fs::remove_dir(&junction_outside);
        let _ = fs::remove_dir(&junction_inside);
        let _ = fs::remove_dir_all(root);
    }

    fn place_junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ])
            .status()
            .is_ok_and(|status| status.success())
    }

    fn place_directory_reparse(link: &Path, target: &Path) -> bool {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        // Без права на symbolic link точка соединения проверяет то же правило:
        // финальный reparse point не разрешается.
        std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ])
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn ask_mode_bookmark_opens_the_app_trash() {
        assert!(bookmark_opens_secure_trash(DeleteMode::Ask));
        assert!(bookmark_opens_secure_trash(DeleteMode::SecureTrash));
        assert!(!bookmark_opens_secure_trash(DeleteMode::WindowsRecycle));
    }

    #[test]
    fn ask_dialog_names_the_app_trash_and_the_windows_bin_separately() {
        let ru = include_str!("../../i18n/ru.json");
        let en = include_str!("../../i18n/en.json");
        assert!(ru.contains("Защищённая корзина приложения"));
        assert!(ru.contains("Стандартная Корзина Windows"));
        assert!(en.contains("Application secure trash"));
        assert!(en.contains("Windows Recycle Bin"));
        assert_ne!(
            "settings.delete_mode.secure",
            "settings.delete_mode.windows"
        );
    }
}
