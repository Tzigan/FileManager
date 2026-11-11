//! Кэш лёгкого списка папки в памяти процесса.
//!
//! На диске не сериализуется и дескрипторы не держит: только уже прочитанные
//! `FileEntry`. Сортировка, фильтр и группы считаются заново поверх этих записей.
//! Задел `invalidate_dir` нужен будущему наблюдению за папкой, само наблюдение здесь не делается.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::core::fs::FileEntry;
use crate::core::ops::OpKind;

/// Сколько папок помнить, если в настройках ещё нет своего числа.
pub fn default_dir_cache_entries() -> usize {
    32
}

/// Обычный каталог можно брать из кэша. Корзина, ZIP и WSL читаются своим путём.
pub fn uses_dir_cache(secure_trash: bool, archive: bool, wsl: bool) -> bool {
    !secure_trash && !archive && !wsl
}

/// Папки, чей список меняет операция. Сам кэш по ним сбрасывается снаружи.
pub fn affected_dirs(kind: &OpKind) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match kind {
        OpKind::Copy { dest, .. } => dirs.push(dest.clone()),
        OpKind::Move { sources, dest } => {
            dirs.push(dest.clone());
            dirs.extend(parents(sources));
        }
        OpKind::Delete { sources } | OpKind::Recycle { sources } => {
            dirs.extend(parents(sources));
        }
        OpKind::Rename { from, to } => {
            if let Some(parent) = from.parent() {
                dirs.push(parent.to_path_buf());
            }
            if let Some(parent) = to.parent() {
                dirs.push(parent.to_path_buf());
            }
        }
        OpKind::CreateDir { path } | OpKind::CreateFile { path } => {
            if let Some(parent) = path.parent() {
                dirs.push(parent.to_path_buf());
            }
        }
    }
    dirs
}

fn parents(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect()
}

#[derive(Clone)]
struct Slot {
    entries: Vec<FileEntry>,
}

/// Словарь «канонический путь → лёгкий список» с вытеснением старых папок.
#[derive(Clone)]
pub struct DirCache {
    slots: HashMap<PathBuf, Slot>,
    /// Хвост — недавно использованный путь.
    recent: VecDeque<PathBuf>,
    /// Номер, выданный последнему чтению пути. Общий счётчик, чтобы номер папки A не совпал с папкой B.
    generation: HashMap<PathBuf, u64>,
    next_generation: u64,
    limit: usize,
}

impl DirCache {
    pub fn new(limit: usize) -> Self {
        Self {
            slots: HashMap::new(),
            recent: VecDeque::new(),
            generation: HashMap::new(),
            next_generation: 0,
            limit,
        }
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        self.evict();
    }

    /// Один и тот же каталог под разным написанием даёт один ключ.
    /// `None` — путь не канонизировался: такой список не кэшируем и не падаем.
    pub fn canonical_key(path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    pub fn get(&mut self, key: &Path) -> Option<Vec<FileEntry>> {
        let entries = self.slots.get(key)?.entries.clone();
        self.touch(key);
        Some(entries)
    }

    /// Начинает чтение. Предыдущий ответ с другим номером будет отвергнут.
    pub fn begin(&mut self, key: &Path) -> u64 {
        self.bump(key)
    }

    /// Пишет список только если это всё ещё актуальное чтение этого пути.
    pub fn store(&mut self, key: &Path, generation: u64, entries: Vec<FileEntry>) -> bool {
        if self.limit == 0 || self.generation.get(key).copied() != Some(generation) {
            return false;
        }
        self.slots.insert(
            key.to_path_buf(),
            Slot {
                entries,
            },
        );
        self.touch(key);
        self.evict();
        true
    }

    /// Точка сброса для обновления, файловой операции и будущего notify.
    pub fn invalidate_dir(&mut self, path: &Path) {
        // Сырой ключ тоже снимаем: так сбрасывается запись, если путь уже канонический.
        self.invalidate(path);
        if let Some(key) = Self::canonical_key(path) {
            self.invalidate(&key);
            if let Some(parent) = key.parent() {
                self.invalidate(&parent.to_path_buf());
            }
            return;
        }
        // Каталог уже удалён: канонизировать его нельзя, но родитель ещё жив.
        let Some(parent) = path.parent() else {
            return;
        };
        let Some(parent_key) = Self::canonical_key(parent) else {
            return;
        };
        self.invalidate(&parent_key);
        let name = path.file_name().map(|name| name.to_string_lossy().to_lowercase());
        let doomed: Vec<PathBuf> = self
            .slots
            .keys()
            .filter(|key| {
                key.parent() == Some(parent_key.as_path())
                    && key
                        .file_name()
                        .map(|item| item.to_string_lossy().to_lowercase())
                        == name
            })
            .cloned()
            .collect();
        for key in doomed {
            self.invalidate(&key);
        }
    }

    fn invalidate(&mut self, key: &Path) {
        self.slots.remove(key);
        self.recent.retain(|path| path != key);
        // Новый номер никто из уже запущенных чтений не знает.
        self.bump(key);
    }

    fn bump(&mut self, key: &Path) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.generation
            .insert(key.to_path_buf(), self.next_generation);
        self.next_generation
    }

    fn touch(&mut self, key: &Path) {
        self.recent.retain(|path| path != key);
        self.recent.push_back(key.to_path_buf());
    }

    fn evict(&mut self) {
        while self.slots.len() > self.limit {
            let Some(oldest) = self.recent.pop_front() else {
                break;
            };
            self.slots.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use super::*;
    use crate::core::fs::{self, SortKey};

    fn entry(name: &str, hidden: bool) -> FileEntry {
        FileEntry {
            name: name.to_owned(),
            path: PathBuf::from(name),
            is_dir: false,
            is_symlink: false,
            hidden,
            system: false,
            readonly: false,
            size: Some(1),
            modified: Some(SystemTime::UNIX_EPOCH),
            created: None,
            extension: Some("txt".into()),
            archive_member: None,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fm-dir-cache-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn repeat_open_same_dir_does_not_reread_from_disk() {
        let dir = scratch("repeat");
        let key = DirCache::canonical_key(&dir).unwrap();
        let mut cache = DirCache::new(8);
        let mut reads = 0;
        let load = |cache: &mut DirCache, reads: &mut usize| {
            if let Some(hit) = cache.get(&key) {
                return hit;
            }
            *reads += 1;
            let entries = vec![entry("a.txt", false)];
            let generation = cache.begin(&key);
            assert!(cache.store(&key, generation, entries.clone()));
            entries
        };
        assert_eq!(load(&mut cache, &mut reads).len(), 1);
        assert_eq!(load(&mut cache, &mut reads).len(), 1);
        assert_eq!(reads, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sort_filter_group_recompute_over_cache_without_disk_reread() {
        let mut cache = DirCache::new(4);
        let key = PathBuf::from(r"C:\cache-sort");
        let stored = vec![entry("b.txt", false), entry("a.txt", true)];
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, stored.clone()));
        let mut view = cache.get(&key).unwrap();
        view = fs::filter_hidden(view, false);
        fs::sort_entries_by(&mut view, SortKey::Name, false);
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].name, "b.txt");
        let again = cache.get(&key).unwrap();
        assert_eq!(again.len(), 2);
        assert_eq!(again[0].name, "b.txt");
    }

    #[test]
    fn manual_refresh_bypasses_cache_and_updates_entry() {
        let mut cache = DirCache::new(4);
        let key = PathBuf::from(r"C:\cache-refresh");
        let first = cache.begin(&key);
        assert!(cache.store(&key, first, vec![entry("old.txt", false)]));
        cache.invalidate_dir(&key);
        assert!(cache.get(&key).is_none());
        let second = cache.begin(&key);
        assert!(!cache.store(&key, first, vec![entry("stale.txt", false)]));
        assert!(cache.store(&key, second, vec![entry("new.txt", false)]));
        assert_eq!(cache.get(&key).unwrap()[0].name, "new.txt");
    }

    #[test]
    fn fs_operation_invalidates_cache_for_affected_dir() {
        let dir = scratch("ops");
        let key = DirCache::canonical_key(&dir).unwrap();
        let mut cache = DirCache::new(4);
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("keep.txt", false)]));
        let kind = OpKind::CreateFile {
            path: dir.join("created.txt"),
        };
        for path in affected_dirs(&kind) {
            cache.invalidate_dir(&path);
        }
        assert!(cache.get(&key).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_async_read_does_not_write_wrong_path_to_cache() {
        let mut cache = DirCache::new(4);
        let folder_a = PathBuf::from(r"C:\folder-a");
        let folder_b = PathBuf::from(r"C:\folder-b");
        let first = cache.begin(&folder_a);
        let second = cache.begin(&folder_b);
        assert!(!cache.store(&folder_b, first, vec![entry("from-a.txt", false)]));
        assert!(cache.store(&folder_b, second, vec![entry("from-b.txt", false)]));
        assert!(cache.get(&folder_a).is_none());
        assert_eq!(cache.get(&folder_b).unwrap()[0].name, "from-b.txt");
    }

    #[test]
    fn cache_memory_limit_evicts_old_dirs() {
        let mut cache = DirCache::new(2);
        for name in ["one", "two", "three"] {
            let key = PathBuf::from(name);
            let generation = cache.begin(&key);
            assert!(cache.store(&key, generation, vec![entry(name, false)]));
        }
        assert!(cache.get(Path::new("one")).is_none());
        assert!(cache.get(Path::new("two")).is_some());
        assert!(cache.get(Path::new("three")).is_some());
    }

    #[test]
    fn secure_trash_tab_is_not_served_from_dir_cache() {
        assert!(!uses_dir_cache(true, false, false));
        assert!(!uses_dir_cache(false, true, false));
        assert!(!uses_dir_cache(false, false, true));
        assert!(uses_dir_cache(false, false, false));
    }

    #[test]
    fn invalidate_dir_hook_exists_for_notify() {
        let mut cache = DirCache::new(2);
        let key = PathBuf::from(r"C:\watched");
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("a.txt", false)]));
        cache.invalidate_dir(&key);
        assert!(cache.get(&key).is_none());
        assert!(!cache.store(&key, generation, vec![entry("late.txt", false)]));
    }

    #[test]
    fn canonical_key_dedupes_same_dir_under_different_spellings() {
        let dir = scratch("canon");
        let nested = dir.join("Sub");
        std::fs::create_dir(&nested).unwrap();
        let direct = DirCache::canonical_key(&nested).unwrap();
        let dotted = nested.join("..").join("Sub");
        let trailed = PathBuf::from(format!("{}\\", nested.display()));
        assert_eq!(DirCache::canonical_key(&dotted).unwrap(), direct);
        assert_eq!(DirCache::canonical_key(&trailed).unwrap(), direct);
        let upper = PathBuf::from(nested.display().to_string().to_uppercase());
        if let Some(upper_key) = DirCache::canonical_key(&upper) {
            assert_eq!(upper_key, direct);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
