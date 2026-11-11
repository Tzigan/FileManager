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
            // Сам каталог тоже: иначе его подпапки остаются в кэше по старому пути.
            dirs.extend(sources.iter().cloned());
            dirs.extend(parents(sources));
        }
        OpKind::Delete { sources } | OpKind::Recycle { sources } => {
            dirs.extend(sources.iter().cloned());
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

/// Удалённый каталог собираем из живого родителя и последнего имени.
fn raw_subtree_root(path: &Path) -> Option<PathBuf> {
    let parent = DirCache::canonical_key(path.parent()?)?;
    let name = path.file_name()?;
    Some(parent.join(name))
}

/// `C:\Work` накрывает `C:\Work` и `C:\Work\sub`, но не `C:\Workplace`.
/// `Path::starts_with` режет по компонентам, поэтому сырой префикс строки не нужен.
fn path_is_inside(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

fn compose_cache_key(path: Option<PathBuf>, volume: Option<String>) -> Option<CacheKey> {
    let path = path?;
    let volume = volume.filter(|id| !id.is_empty())?;
    Some(CacheKey::new(path, volume))
}

fn parents(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect()
}

/// Ключ списка: путь и серийный номер тома. Одна буква диска на другой флешке — другой ключ.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub path: PathBuf,
    pub volume: String,
}

impl CacheKey {
    pub fn new(path: impl Into<PathBuf>, volume: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            volume: volume.into(),
        }
    }
}

#[derive(Clone)]
struct Slot {
    entries: Vec<FileEntry>,
}

/// Словарь «канонический путь → лёгкий список» с вытеснением старых папок.
#[derive(Clone)]
pub struct DirCache {
    slots: HashMap<CacheKey, Slot>,
    /// Хвост — недавно использованный путь.
    recent: VecDeque<CacheKey>,
    /// Номер, выданный последнему чтению пути. Общий счётчик, чтобы номер папки A не совпал с папкой B.
    generation: HashMap<CacheKey, u64>,
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

    /// Один и тот же каталог под разным написанием даёт один путь.
    /// `None` — путь не канонизировался: такой список не кэшируем и не падаем.
    pub fn canonical_key(path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    /// Ключ для живой папки. Ошибка канонизации или тома — `None`, без паники: папка просто не кэшируется.
    pub fn cache_key(path: &Path) -> Option<CacheKey> {
        let path = Self::canonical_key(path)?;
        compose_cache_key(Some(path.clone()), crate::platform::volume_id(&path))
    }

    /// Заход в уже удалённую папку не канонизируется и поэтому не достаёт старую запись.
    pub fn lookup(&mut self, path: &Path) -> Option<Vec<FileEntry>> {
        let key = Self::cache_key(path)?;
        self.get(&key)
    }

    pub fn get(&mut self, key: &CacheKey) -> Option<Vec<FileEntry>> {
        let entries = self.slots.get(key)?.entries.clone();
        self.touch(key);
        Some(entries)
    }

    /// Начинает чтение. Предыдущий ответ с другим номером будет отвергнут.
    pub fn begin(&mut self, key: &CacheKey) -> u64 {
        self.bump(key)
    }

    /// Пишет список только если это всё ещё актуальное чтение этого пути и тома.
    pub fn store(&mut self, key: &CacheKey, generation: u64, entries: Vec<FileEntry>) -> bool {
        if self.limit == 0 || self.generation.get(key).copied() != Some(generation) {
            return false;
        }
        self.slots.insert(key.clone(), Slot { entries });
        self.touch(key);
        self.evict();
        true
    }

    /// Сбрасывает путь, все закэшированные подпапки и список непосредственного родителя.
    /// Корень тома целиком не вычищается: иначе удаление одной папки сотрёт кэш всего диска.
    pub fn invalidate_dir(&mut self, path: &Path) {
        // Совпадает и с тестовым ключом, и с уже канонической записью.
        self.drop_subtree(path);
        let root = Self::canonical_key(path).or_else(|| raw_subtree_root(path));
        let Some(root) = root else {
            return;
        };
        self.drop_subtree(&root);
        // Родитель — только его собственный список, без чужих папок на том же диске.
        if let Some(parent) = root.parent().filter(|parent| *parent != root) {
            self.drop_exact(parent);
        }
    }

    fn drop_subtree(&mut self, root: &Path) {
        let doomed: Vec<CacheKey> = self
            .generation
            .keys()
            .chain(self.slots.keys())
            .filter(|key| path_is_inside(root, &key.path))
            .cloned()
            .collect();
        for key in doomed {
            self.invalidate_key(&key);
        }
    }

    fn drop_exact(&mut self, path: &Path) {
        let doomed: Vec<CacheKey> = self
            .generation
            .keys()
            .chain(self.slots.keys())
            .filter(|key| key.path == path)
            .cloned()
            .collect();
        for key in doomed {
            self.invalidate_key(&key);
        }
    }

    fn invalidate_key(&mut self, key: &CacheKey) {
        self.slots.remove(key);
        self.recent.retain(|path| path != key);
        self.bump(key);
    }

    fn bump(&mut self, key: &CacheKey) -> u64 {
        // saturating_add не возвращает номер к нулю, пока жив этот кэш.
        // Переполнение u64 в одной сессии недостижимо.
        self.next_generation = self.next_generation.saturating_add(1);
        self.generation.insert(key.clone(), self.next_generation);
        self.next_generation
    }

    fn touch(&mut self, key: &CacheKey) {
        self.recent.retain(|path| path != key);
        self.recent.push_back(key.clone());
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

    fn k(path: impl Into<PathBuf>) -> CacheKey {
        CacheKey::new(path, "TEST")
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
        let key = DirCache::cache_key(&dir).unwrap();
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
        let key = k(r"C:\cache-sort");
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
        let key = k(r"C:\cache-refresh");
        let first = cache.begin(&key);
        assert!(cache.store(&key, first, vec![entry("old.txt", false)]));
        cache.invalidate_dir(Path::new(r"C:\cache-refresh"));
        assert!(cache.get(&key).is_none());
        let second = cache.begin(&key);
        assert!(!cache.store(&key, first, vec![entry("stale.txt", false)]));
        assert!(cache.store(&key, second, vec![entry("new.txt", false)]));
        assert_eq!(cache.get(&key).unwrap()[0].name, "new.txt");
    }

    #[test]
    fn fs_operation_invalidates_cache_for_affected_dir() {
        let dir = scratch("ops");
        let key = DirCache::cache_key(&dir).unwrap();
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
        let folder_a = k(r"C:\folder-a");
        let folder_b = k(r"C:\folder-b");
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
            let key = k(name);
            let generation = cache.begin(&key);
            assert!(cache.store(&key, generation, vec![entry(name, false)]));
        }
        assert!(cache.get(&k("one")).is_none());
        assert!(cache.get(&k("two")).is_some());
        assert!(cache.get(&k("three")).is_some());
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
        let key = k(r"C:\watched");
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("a.txt", false)]));
        cache.invalidate_dir(Path::new(r"C:\watched"));
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

    fn seed(cache: &mut DirCache, dir: &std::path::Path) {
        let key = DirCache::cache_key(dir).unwrap();
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("kept.txt", false)]));
    }

    #[test]
    fn move_invalidates_both_source_and_dest_dirs() {
        let source = scratch("move-src");
        let dest = scratch("move-dst");
        let mut cache = DirCache::new(8);
        seed(&mut cache, &source);
        seed(&mut cache, &dest);
        let kind = OpKind::Move {
            sources: vec![source.join("file.txt")],
            dest: dest.clone(),
        };
        for path in affected_dirs(&kind) {
            cache.invalidate_dir(&path);
        }
        assert!(cache.get(&DirCache::cache_key(&source).unwrap()).is_none());
        assert!(cache.get(&DirCache::cache_key(&dest).unwrap()).is_none());
        let _ = std::fs::remove_dir_all(source);
        let _ = std::fs::remove_dir_all(dest);
    }

    #[test]
    fn copy_invalidates_dest_not_source() {
        let source = scratch("copy-src");
        let dest = scratch("copy-dst");
        let mut cache = DirCache::new(8);
        seed(&mut cache, &source);
        seed(&mut cache, &dest);
        let kind = OpKind::Copy {
            sources: vec![source.join("file.txt")],
            dest: dest.clone(),
        };
        let touched = affected_dirs(&kind);
        assert!(touched.iter().all(|path| path != &source));
        for path in touched {
            cache.invalidate_dir(&path);
        }
        assert!(cache.get(&DirCache::cache_key(&source).unwrap()).is_some());
        assert!(cache.get(&DirCache::cache_key(&dest).unwrap()).is_none());
        let _ = std::fs::remove_dir_all(source);
        let _ = std::fs::remove_dir_all(dest);
    }

    #[test]
    fn rename_invalidates_parent_dir() {
        let parent = scratch("rename");
        let other = scratch("rename-other");
        let mut cache = DirCache::new(8);
        seed(&mut cache, &parent);
        seed(&mut cache, &other);
        let kind = OpKind::Rename {
            from: parent.join("old.txt"),
            to: other.join("new.txt"),
        };
        for path in affected_dirs(&kind) {
            cache.invalidate_dir(&path);
        }
        assert!(cache.get(&DirCache::cache_key(&parent).unwrap()).is_none());
        assert!(cache.get(&DirCache::cache_key(&other).unwrap()).is_none());
        let _ = std::fs::remove_dir_all(parent);
        let _ = std::fs::remove_dir_all(other);
    }

    #[test]
    fn create_invalidates_parent_dir() {
        let parent = scratch("create");
        let mut cache = DirCache::new(4);
        seed(&mut cache, &parent);
        let kind = OpKind::CreateFile {
            path: parent.join("fresh.txt"),
        };
        for path in affected_dirs(&kind) {
            cache.invalidate_dir(&path);
        }
        assert!(cache.get(&DirCache::cache_key(&parent).unwrap()).is_none());
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn delete_invalidates_parent_dir() {
        let parent = scratch("delete");
        let mut cache = DirCache::new(4);
        seed(&mut cache, &parent);
        let kind = OpKind::Delete {
            sources: vec![parent.join("gone.txt")],
        };
        for path in affected_dirs(&kind) {
            cache.invalidate_dir(&path);
        }
        assert!(cache.get(&DirCache::cache_key(&parent).unwrap()).is_none());
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn extract_zip_or_restore_trash_invalidates_target_dir_cache() {
        let parent = scratch("target");
        let mut cache = DirCache::new(4);
        seed(&mut cache, &parent);
        // И извлечение, и восстановление пишут файл в папку. Сбрасывается она, не корзина.
        cache.invalidate_dir(&parent.join("restored.txt"));
        assert!(cache.get(&DirCache::cache_key(&parent).unwrap()).is_none());
        assert!(uses_dir_cache(true, false, false) == false);
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn readonly_ops_do_not_invalidate_cache() {
        let mut cache = DirCache::new(4);
        let key = k(r"C:\readonly-cache");
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("b.txt", false), entry("a.txt", true)]));
        let mut view = cache.get(&key).unwrap();
        view = fs::filter_hidden(view, false);
        fs::sort_entries_by(&mut view, SortKey::Name, false);
        assert_eq!(view.len(), 1);
        assert_eq!(cache.get(&key).unwrap().len(), 2);
    }

    #[test]
    fn request_generation_is_monotonic_and_does_not_wrap_in_session() {
        let mut cache = DirCache::new(4);
        let key = k(r"C:\generation");
        let first = cache.begin(&key);
        let second = cache.begin(&key);
        let third = cache.begin(&key);
        assert!(first < second && second < third);
        assert_eq!(std::mem::size_of_val(&first), 8);
        assert_ne!(first, 0);
    }

    #[test]
    fn recursive_delete_dir_invalidates_whole_subtree_cache() {
        let dir = scratch("tree");
        let sub1 = dir.join("sub1");
        let sub2 = dir.join("sub2");
        std::fs::create_dir(&sub1).unwrap();
        std::fs::create_dir(&sub2).unwrap();
        let mut cache = DirCache::new(8);
        let keys = [
            DirCache::cache_key(&dir).unwrap(),
            DirCache::cache_key(&sub1).unwrap(),
            DirCache::cache_key(&sub2).unwrap(),
        ];
        for key in &keys {
            let generation = cache.begin(key);
            assert!(cache.store(key, generation, vec![entry("old.txt", false)]));
        }
        std::fs::remove_dir_all(&dir).unwrap();
        cache.invalidate_dir(&dir);
        for key in &keys {
            assert!(cache.get(key).is_none());
        }
        assert!(DirCache::cache_key(&sub1).is_none());
    }

    #[test]
    fn move_dir_invalidates_source_subtree_paths() {
        let source = scratch("move-tree");
        let nested = source.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let dest = scratch("move-dest");
        let mut cache = DirCache::new(8);
        let source_key = DirCache::cache_key(&source).unwrap();
        let nested_key = DirCache::cache_key(&nested).unwrap();
        let dest_key = DirCache::cache_key(&dest).unwrap();
        for key in [&source_key, &nested_key, &dest_key] {
            let generation = cache.begin(key);
            assert!(cache.store(key, generation, vec![entry("old.txt", false)]));
        }
        let kind = OpKind::Move {
            sources: vec![source.clone()],
            dest: dest.clone(),
        };
        for path in affected_dirs(&kind) {
            cache.invalidate_dir(&path);
        }
        assert!(cache.get(&source_key).is_none());
        assert!(cache.get(&nested_key).is_none());
        assert!(cache.get(&dest_key).is_none());
        let _ = std::fs::remove_dir_all(source);
        let _ = std::fs::remove_dir_all(dest);
    }

    #[test]
    fn extract_zip_nested_invalidates_created_dirs() {
        let root = scratch("zip-out");
        let nested = root.join("docs");
        std::fs::create_dir(&nested).unwrap();
        let mut cache = DirCache::new(4);
        let nested_key = DirCache::cache_key(&nested).unwrap();
        let generation = cache.begin(&nested_key);
        assert!(cache.store(&nested_key, generation, vec![entry("stale.txt", false)]));
        cache.invalidate_dir(&root);
        assert!(cache.get(&nested_key).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_path_in_cache_gives_miss_not_stale() {
        let dir = scratch("missing");
        let key = DirCache::cache_key(&dir).unwrap();
        let mut cache = DirCache::new(4);
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("ghost.txt", false)]));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(DirCache::cache_key(&dir).is_none());
        cache.invalidate_dir(&dir);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn volume_swap_same_letter_different_cache_key() {
        let dir = scratch("volume");
        let path = DirCache::canonical_key(&dir).unwrap();
        let old_disk = CacheKey::new(path.clone(), "AAAA1111");
        let new_disk = CacheKey::new(path, "BBBB2222");
        assert_ne!(old_disk, new_disk);
        let mut cache = DirCache::new(4);
        let generation = cache.begin(&old_disk);
        assert!(cache.store(&old_disk, generation, vec![entry("flash1.txt", false)]));
        assert!(cache.get(&new_disk).is_none());
        let live = DirCache::cache_key(&dir).unwrap();
        assert_eq!(live.volume, crate::platform::volume_id(&dir).unwrap());
        assert_ne!(live, old_disk);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prefix_invalidation_respects_component_boundary() {
        let base = scratch("boundary");
        let work = base.join("Work");
        let sub = work.join("sub1");
        let workplace = base.join("Workplace");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir(&workplace).unwrap();
        let mut cache = DirCache::new(8);
        let work_key = DirCache::cache_key(&work).unwrap();
        let sub_key = DirCache::cache_key(&sub).unwrap();
        let workplace_key = DirCache::cache_key(&workplace).unwrap();
        for key in [&work_key, &sub_key, &workplace_key] {
            let generation = cache.begin(key);
            assert!(cache.store(key, generation, vec![entry("a.txt", false)]));
        }
        cache.invalidate_dir(&work);
        assert!(cache.get(&work_key).is_none());
        assert!(cache.get(&sub_key).is_none());
        assert!(cache.get(&workplace_key).is_some());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn recursive_delete_invalidates_subtree_and_parent() {
        let parent = scratch("parent");
        let dir = parent.join("dir");
        let sub = dir.join("sub1");
        std::fs::create_dir_all(&sub).unwrap();
        let mut cache = DirCache::new(8);
        let parent_key = DirCache::cache_key(&parent).unwrap();
        let sub_key = DirCache::cache_key(&sub).unwrap();
        for key in [&parent_key, &DirCache::cache_key(&dir).unwrap(), &sub_key] {
            let generation = cache.begin(key);
            assert!(cache.store(key, generation, vec![entry("old.txt", false)]));
        }
        std::fs::remove_dir_all(&dir).unwrap();
        cache.invalidate_dir(&dir);
        assert!(cache.get(&sub_key).is_none());
        assert!(cache.get(&parent_key).is_none());
        assert!(DirCache::cache_key(&sub).is_none());
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn move_dir_invalidates_source_subtree_and_dest_parent() {
        let source_parent = scratch("src-parent");
        let source = source_parent.join("dir");
        let nested = source.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let dest = scratch("dst-parent");
        let mut cache = DirCache::new(8);
        let parent_key = DirCache::cache_key(&source_parent).unwrap();
        let nested_key = DirCache::cache_key(&nested).unwrap();
        let dest_key = DirCache::cache_key(&dest).unwrap();
        for key in [&parent_key, &nested_key, &dest_key] {
            let generation = cache.begin(key);
            assert!(cache.store(key, generation, vec![entry("old.txt", false)]));
        }
        let kind = OpKind::Move {
            sources: vec![source.clone()],
            dest: dest.clone(),
        };
        for path in affected_dirs(&kind) {
            cache.invalidate_dir(&path);
        }
        assert!(cache.get(&nested_key).is_none());
        assert!(cache.get(&parent_key).is_none());
        assert!(cache.get(&dest_key).is_none());
        let moved = dest.join("dir").join("nested");
        assert!(DirCache::cache_key(&moved).is_none());
        let _ = std::fs::remove_dir_all(source_parent);
        let _ = std::fs::remove_dir_all(dest);
    }

    #[test]
    fn dead_path_cache_key_empty_no_stale() {
        let dir = scratch("dead");
        let key = DirCache::cache_key(&dir).unwrap();
        let mut cache = DirCache::new(4);
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, vec![entry("ghost.txt", false)]));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(DirCache::cache_key(&dir).is_none());
        assert!(cache.lookup(&dir).is_none());
    }

    #[test]
    fn canonicalize_error_non_enoent_does_not_panic() {
        let dir = scratch("not-a-dir");
        // Слишком длинное имя — не код 2 (файл не найден). Кэш всё равно молча отказывается.
        let blocked = dir.join("a".repeat(300));
        let error = std::fs::canonicalize(&blocked).unwrap_err();
        assert_ne!(error.raw_os_error(), Some(2), "{error:?}");
        let key = std::panic::catch_unwind(|| DirCache::cache_key(&blocked)).unwrap();
        assert!(key.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn volume_unreadable_not_cached() {
        let dir = scratch("no-volume");
        let path = DirCache::canonical_key(&dir).unwrap();
        assert!(compose_cache_key(Some(path), None).is_none());
        assert!(compose_cache_key(None, Some("AAAA1111".into())).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
