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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub theme: Theme,
    /// Paths of open tabs, restored on startup.
    pub tabs: Vec<PathBuf>,
    pub active_tab: usize,
    pub bookmarks: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            theme: Theme::default(),
            tabs: Vec::new(),
            active_tab: 0,
            bookmarks: Vec::new(),
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
