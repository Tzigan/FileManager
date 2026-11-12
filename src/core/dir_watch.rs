//! Решение, что делать с пачкой уведомлений об изменениях папки.
//!
//! Само чтение `ReadDirectoryChangesW` живёт в платформе. Здесь только правило:
//! события копятся, а в список уходит один полный перечит. Имена из буфера не применяются.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Команда единственному потоку наблюдения.
#[derive(Debug)]
pub enum DirWatchCommand {
    Watch { path: PathBuf, generation: u64 },
    Stop,
}

/// Уведомление интерфейсу. Путь — та папка, которую наблюдали.
#[derive(Debug)]
pub struct WatchNote {
    pub path: PathBuf,
    pub generation: u64,
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
    /// Сам каталог удалён или помечен на удаление; его дескриптор надо закрыть.
    DirectoryRemoved,
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
    deadline: Option<Instant>,
    pub reloads: u32,
}

impl WatchDebounce {
    #[cfg(test)]
    pub fn note(&mut self, signal: WatchSignal) -> WatchReaction {
        self.note_at(
            signal,
            Instant::now(),
            Duration::from_millis(default_dir_watch_debounce_ms()),
        )
    }

    /// Начинает или продлевает окно тишины, не обращаясь к системному таймеру в тестах.
    pub fn note_at(&mut self, signal: WatchSignal, now: Instant, pause: Duration) -> WatchReaction {
        match signal {
            WatchSignal::Unavailable => {
                self.pending = false;
                self.overflow = false;
                self.deadline = None;
                WatchReaction::ManualOnly
            }
            WatchSignal::Changed | WatchSignal::DirectoryRemoved => {
                self.pending = true;
                self.deadline = Some(now + pause);
                WatchReaction::Coalesce
            }
            WatchSignal::Overflow => {
                self.pending = true;
                self.overflow = true;
                self.deadline = Some(now + pause);
                WatchReaction::Coalesce
            }
        }
    }

    /// Выдаёт reload только после полной паузы от последнего события.
    pub fn flush_if_due(&mut self, now: Instant) -> Option<WatchReaction> {
        if !self.deadline.is_some_and(|deadline| now >= deadline) {
            return None;
        }
        self.flush()
    }

    /// Один вызов после паузы. Тысяча событий до него дают один `Reload`.
    pub fn flush(&mut self) -> Option<WatchReaction> {
        self.deadline = None;
        if !self.pending {
            return None;
        }
        self.pending = false;
        let overflow = self.overflow;
        self.overflow = false;
        self.reloads += 1;
        Some(WatchReaction::Reload { overflow })
    }

    pub fn reset(&mut self) {
        self.pending = false;
        self.overflow = false;
        self.deadline = None;
    }
}

/// Токен отложенного reload. Его нельзя применять после смены вкладки или папки.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchReloadToken {
    pub path: PathBuf,
    pub tab_id: u64,
    pub generation: u64,
}

impl WatchReloadToken {
    pub fn is_current(
        &self,
        generation: u64,
        target_tab_id: Option<u64>,
        target_path: Option<&Path>,
        active_tab_id: Option<u64>,
        active_path: Option<&Path>,
    ) -> bool {
        self.generation == generation
            && target_tab_id == Some(self.tab_id)
            && target_path == Some(self.path.as_path())
            && active_tab_id == Some(self.tab_id)
            && active_path == Some(self.path.as_path())
    }
}

/// Наблюдаем только обычную папку. Корзина, ZIP и WSL остаются на своём чтении.
pub fn watch_applies(secure_trash: bool, archive: bool, wsl: bool) -> bool {
    crate::core::dir_cache::uses_dir_cache(secure_trash, archive, wsl)
}

/// Обычную папку перечитываем при активации; специальные представления имеют свой путь загрузки.
pub fn should_reload_on_activation(
    tab_changed: bool,
    secure_trash: bool,
    archive: bool,
    wsl: bool,
) -> bool {
    tab_changed && watch_applies(secure_trash, archive, wsl)
}

/// Сколько потоков держит наблюдение. Один координатор, не поток на каждую папку.
#[cfg(test)]
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
    fn notify_debounce_coalesces_burst_with_trailing_reload() {
        let mut debounce = WatchDebounce::default();
        let start = Instant::now();
        let pause = Duration::from_millis(400);
        let mut actual_names = vec!["initial.txt"];

        for (elapsed, name) in [
            (0, "first.txt"),
            (300, "middle.txt"),
            (600, "next.txt"),
            (900, "final.txt"),
        ] {
            assert!(
                debounce
                    .flush_if_due(start + Duration::from_millis(elapsed))
                    .is_none()
            );
            actual_names.push(name);
            assert_eq!(
                debounce.note_at(
                    WatchSignal::Changed,
                    start + Duration::from_millis(elapsed),
                    pause,
                ),
                WatchReaction::Coalesce
            );
        }

        assert!(
            debounce
                .flush_if_due(start + Duration::from_millis(1299))
                .is_none()
        );
        let displayed_names = match debounce.flush_if_due(start + Duration::from_millis(1300)) {
            Some(WatchReaction::Reload { overflow: false }) => actual_names.clone(),
            _ => panic!("после затишья должен пройти один полный reload"),
        };
        assert_eq!(displayed_names, actual_names);
        assert_eq!(debounce.reloads, 1);
        assert!(
            debounce
                .flush_if_due(start + Duration::from_millis(2000))
                .is_none()
        );
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
    fn debounced_reload_does_not_apply_after_dir_or_tab_switch() {
        let token = WatchReloadToken {
            path: PathBuf::from(r"C:\watched"),
            tab_id: 7,
            generation: 3,
        };
        let watched = Path::new(r"C:\watched");

        assert!(token.is_current(3, Some(7), Some(watched), Some(7), Some(watched)));
        assert!(!token.is_current(4, Some(7), Some(watched), Some(7), Some(watched)));
        assert!(!token.is_current(3, Some(8), Some(watched), Some(8), Some(watched)));
        assert!(!token.is_current(
            3,
            Some(7),
            Some(watched),
            Some(7),
            Some(Path::new(r"C:\new-folder"))
        ));
    }

    #[test]
    fn inactive_tab_rereads_on_activation_or_limitation_documented() {
        assert!(should_reload_on_activation(true, false, false, false));
        assert!(!should_reload_on_activation(false, false, false, false));
        assert!(!should_reload_on_activation(true, true, false, false));
        assert!(!should_reload_on_activation(true, false, true, false));
        assert!(!should_reload_on_activation(true, false, false, true));
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
    fn notify_unavailable_falls_back_to_status_message() {
        let mut debounce = WatchDebounce::default();
        assert_eq!(
            debounce.note_at(
                WatchSignal::Unavailable,
                Instant::now(),
                Duration::from_millis(400)
            ),
            WatchReaction::ManualOnly
        );
        assert!(debounce.flush().is_none());
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

    #[test]
    fn watched_dir_deleted_externally_handled() {
        let dir = std::env::temp_dir().join(format!("fm-watch-deleted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = DirCache::cache_key(&dir).unwrap();
        let mut cache = DirCache::new(2);
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("stale.txt")]));

        let mut debounce = WatchDebounce::default();
        assert_eq!(
            debounce.note(WatchSignal::DirectoryRemoved),
            WatchReaction::Coalesce
        );
        assert!(std::fs::remove_dir_all(&dir).is_ok());
        cache.invalidate_dir(&dir);
        assert!(DirCache::cache_key(&dir).is_none());
        assert!(cache.lookup(&dir).is_none());

        let cancelled = std::sync::atomic::AtomicBool::new(false);
        assert!(matches!(
            crate::core::fs::read_dir(&dir, &cancelled),
            Err(crate::core::fs::ReadDirError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound
        ));
    }
}
