use std::fs;
use std::path::{Path, PathBuf};

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

/// Папки из списка — префиксы путей, не маски. Это ограничение 0.8.
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
        folders
            .iter()
            .any(|folder| !folder.as_os_str().is_empty() && path.starts_with(folder))
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
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
    pub settings: Settings,
}

impl SettingsStore {
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join("settings.json");
        let settings = match fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<Settings>(&raw) {
                Ok(s) => {
                    tracing::info!(path = %path.display(), "settings loaded");
                    s
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), "settings parse failed, using defaults: {e}");
                    Settings::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = %path.display(), "no settings file, using defaults");
                Settings::default()
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), "settings read failed, using defaults: {e}");
                Settings::default()
            }
        };
        Self { path, settings }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(&self.settings)?;
        fs::write(&self.path, raw)?;
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
        let forced = PathBuf::from(r"D:\private\a.txt");
        let folders = vec![PathBuf::from(r"D:\private")];
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
        let permanent = route_delete(DeleteMode::SecureTrash, true, &[file.clone()], &folders);
        assert!(permanent.permanent);
        assert!(permanent.secure.is_empty());
    }
}
