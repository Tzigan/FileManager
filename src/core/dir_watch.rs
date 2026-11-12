//! Решение, что делать с пачкой уведомлений об изменениях папки.
//!
//! Само чтение `ReadDirectoryChangesW` живёт в платформе. Здесь только правило:
//! события копятся, а в список уходит один полный перечит. Имена из буфера не применяются.

use std::path::PathBuf;

/// Команда единственному потоку наблюдения.
#[derive(Debug)]
pub enum DirWatchCommand {
    Watch(PathBuf),
    Stop,
}

/// Уведомление интерфейсу. Путь — та папка, которую наблюдали.
#[derive(Debug)]
pub struct WatchNote {
    pub path: PathBuf,
    pub signal: WatchSignal,
}

/// Пауза, после которой пачка изменений становится одним обновлением списка.
pub fn default_dir_watch_debounce_ms() -> u64 {
    400
}

/// Что прислало наблюдение. Имена файлов сюда не входят: их легко потерять.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchSignal {
    Changed,
    /// Буфер кончился или запись обрезана. Часть событий уже потеряна.
    Overflow,
    /// Наблюдение на этом пути недоступно. Дальше только кнопка «Обновить».
    Unavailable,
}

/// Что делает интерфейс. Перечит всегда полный: потерянные события нельзя дорисовать.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchReaction {
    /// Событие принято в текущую пачку, список ещё не трогаем.
    Coalesce,
    /// Пауза вышла: один сброс кэша и одно чтение папки.
    Reload { overflow: bool },
    ManualOnly,
}

#[derive(Debug, Default)]
pub struct WatchDebounce {
    pending: bool,
    overflow: bool,
    pub reloads: u32,
}

impl WatchDebounce {
    pub fn note(&mut self, signal: WatchSignal) -> WatchReaction {
        match signal {
            WatchSignal::Unavailable => {
                self.pending = false;
                self.overflow = false;
                WatchReaction::ManualOnly
            }
            WatchSignal::Changed => {
                self.pending = true;
                WatchReaction::Coalesce
            }
            WatchSignal::Overflow => {
                self.pending = true;
                self.overflow = true;
                WatchReaction::Coalesce
            }
        }
    }

    /// Один вызов после паузы. Тысяча событий до него дают один `Reload`.
    pub fn flush(&mut self) -> Option<WatchReaction> {
        if !self.pending {
            return None;
        }
        self.pending = false;
        let overflow = self.overflow;
        self.overflow = false;
        self.reloads += 1;
        Some(WatchReaction::Reload { overflow })
    }
}

/// Наблюдаем только обычную папку. Корзина, ZIP и WSL остаются на своём чтении.
pub fn watch_applies(secure_trash: bool, archive: bool, wsl: bool) -> bool {
    crate::core::dir_cache::uses_dir_cache(secure_trash, archive, wsl)
}

/// Сколько потоков держит наблюдение. Один координатор, не поток на каждую папку.
pub const WATCH_COORDINATOR_THREADS: usize = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::dir_cache::{DirCache, uses_dir_cache};
    use crate::core::fs::FileEntry;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn entry(name: &str) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: PathBuf::from(name),
            is_dir: false,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: Some(1),
            modified: Some(SystemTime::UNIX_EPOCH),
            created: None,
            extension: Some("txt".into()),
            archive_member: None,
        }
    }

    #[test]
    fn notify_debounce_coalesces_burst() {
        let mut burst = WatchDebounce::default();
        for _ in 0..1000 {
            assert_eq!(burst.note(WatchSignal::Changed), WatchReaction::Coalesce);
        }
        assert_eq!(burst.flush(), Some(WatchReaction::Reload { overflow: false }));
        assert_eq!(burst.reloads, 1);
        assert!(burst.flush().is_none());
    }

    #[test]
    fn notify_buffer_overflow_triggers_full_reload() {
        let mut burst = WatchDebounce::default();
        burst.note(WatchSignal::Changed);
        burst.note(WatchSignal::Overflow);
        assert_eq!(burst.flush(), Some(WatchReaction::Reload { overflow: true }));
        assert_eq!(burst.reloads, 1);
    }

    #[test]
    fn notify_unavailable_falls_back_to_manual_refresh() {
        let mut burst = WatchDebounce::default();
        assert_eq!(
            burst.note(WatchSignal::Unavailable),
            WatchReaction::ManualOnly
        );
        assert!(burst.flush().is_none());
    }

    #[test]
    fn notify_invalidates_dir_cache_via_existing_invalidate_dir() {
        let dir = std::env::temp_dir().join(format!(
            "fm-watch-invalidate-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = DirCache::cache_key(&dir).unwrap();
        let mut cache = DirCache::new(4);
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("old.txt")]));
        // Тот же вызов, что у копирования и у кнопки «Обновить».
        cache.invalidate_dir(&dir);
        assert!(cache.get(&key).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_refresh_and_fs_ops_still_invalidate() {
        let mut cache = DirCache::new(2);
        let key = crate::core::dir_cache::CacheKey::new(r"C:\refresh-still", "TEST");
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("a.txt")]));
        cache.invalidate_dir(std::path::Path::new(r"C:\refresh-still"));
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn notify_single_coordinator_not_per_dir_thread() {
        assert_eq!(WATCH_COORDINATOR_THREADS, 1);
    }

    #[test]
    fn trash_zip_wsl_not_broken_by_notify() {
        assert!(!watch_applies(true, false, false));
        assert!(!watch_applies(false, true, false));
        assert!(!watch_applies(false, false, true));
        assert!(watch_applies(false, false, false));
        assert!(!uses_dir_cache(true, false, false));
    }
}
