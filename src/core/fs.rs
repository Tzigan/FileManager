use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

/// One row of a directory listing.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub hidden: bool,
    pub system: bool,
    pub readonly: bool,
    /// `None` for directories (their size is not computed on listing).
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    /// Lowercase extension without the dot, e.g. `txt`.
    pub extension: Option<String>,
    /// Путь элемента внутри ZIP; для обычных файлов равен `None`.
    pub archive_member: Option<String>,
}

#[derive(Debug)]
pub enum ReadDirError {
    /// Aborted by the caller via the cancellation flag.
    Cancelled,
    Io(io::Error),
}

impl From<io::Error> for ReadDirError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// How often the cancellation flag is checked while scanning a folder.
const CANCEL_CHECK_INTERVAL: usize = 256;

/// Reads a directory listing: directories first, then files, each group
/// sorted case-insensitively by name.
///
/// Must be called on a worker thread — this blocks on filesystem I/O.
/// `cancel` is polled every [`CANCEL_CHECK_INTERVAL`] entries; setting it
/// aborts the scan with [`ReadDirError::Cancelled`].
pub fn read_dir(path: &Path, cancel: &AtomicBool) -> Result<Vec<FileEntry>, ReadDirError> {
    let mut entries = Vec::new();

    for (i, item) in fs::read_dir(path)?.enumerate() {
        if i % CANCEL_CHECK_INTERVAL == 0 && cancel.load(Ordering::Relaxed) {
            return Err(ReadDirError::Cancelled);
        }
        let item = item?;
        if let Some(entry) = map_entry(&item) {
            entries.push(entry);
        }
    }

    sort_entries(&mut entries);
    Ok(entries)
}

pub fn filter_hidden(mut entries: Vec<FileEntry>, show_hidden_files: bool) -> Vec<FileEntry> {
    entries.retain(|entry| is_visible_entry(entry, show_hidden_files));
    entries
}

pub fn is_visible_entry(entry: &FileEntry, show_hidden_files: bool) -> bool {
    !entry.system && (show_hidden_files || !entry.hidden)
}

pub fn sort_entries(entries: &mut Vec<FileEntry>) {
    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));
}

/// Column the file list is sorted by.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SortKey {
    Name,
    Extension,
    Size,
    Modified,
}

/// Directories first; inside a group — by `key`, `desc` reverses the group order.
pub fn sort_entries_by(entries: &mut Vec<FileEntry>, key: SortKey, desc: bool) {
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            let ord = match key {
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Extension => a
                    .extension
                    .cmp(&b.extension)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                SortKey::Size => a
                    .size
                    .cmp(&b.size)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                SortKey::Modified => a
                    .modified
                    .cmp(&b.modified)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            };
            if desc { ord.reverse() } else { ord }
        })
    });
}

pub(crate) fn map_entry(item: &fs::DirEntry) -> Option<FileEntry> {
    let path = item.path();
    let name = item.file_name().to_string_lossy().into_owned();
    let file_type = item.file_type().ok()?;

    // metadata() follows symlinks; on Windows it reuses the find data
    // already fetched by the directory enumeration, so it is cheap.
    let meta = item.metadata().ok();
    // Размер на диске, дата доступа, владелец и список доступа здесь не читаются:
    // это отдельные запросы, их делает панель свойств для одного объекта.
    // Скрытый и системный атрибуты извлекаем из уже полученной метаинформации.
    let (hidden, system) = visibility_attributes(item, meta.as_ref());

    let is_dir = meta
        .as_ref()
        .map(|m| m.is_dir())
        .unwrap_or(file_type.is_dir());
    let size = if is_dir {
        None
    } else {
        meta.as_ref().map(|m| m.len())
    };
    let modified = meta.as_ref().and_then(|m| m.modified().ok());
    let created = meta.as_ref().and_then(|m| m.created().ok());
    let readonly = meta
        .as_ref()
        .map(|m| m.permissions().readonly())
        .unwrap_or(false);

    let extension = listing_extension(is_dir, &path);

    Some(FileEntry {
        name,
        path,
        is_dir,
        is_symlink: file_type.is_symlink(),
        hidden,
        system,
        readonly,
        size,
        modified,
        created,
        extension,
        archive_member: None,
    })
}

/// Расширение для списка и группировки. Папка — без расширения.
/// У файла берётся последний суффикс после точки, в нижнем регистре:
/// `NOTES.TXT` и `archive.tar.gz` дают `txt` и `gz`.
pub fn listing_extension(is_dir: bool, path: &Path) -> Option<String> {
    if is_dir {
        None
    } else {
        path.extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
    }
}

/// Lists immediate subdirectories of `path`, sorted case-insensitively.
/// Used by the folder tree; directory entries only, no file metadata.
pub fn read_subdirs(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        if item.file_type().map(|t| t.is_dir()).unwrap_or(false) && !is_system(&item) {
            dirs.push(item.path());
        }
    }
    dirs.sort_by_cached_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(dirs)
}

#[cfg(windows)]
fn visibility_attributes(_item: &fs::DirEntry, metadata: Option<&fs::Metadata>) -> (bool, bool) {
    use std::os::windows::fs::MetadataExt;
    let Some(metadata) = metadata else {
        // Неизвестные атрибуты скрываем, чтобы не показывать защищённую запись.
        return (false, true);
    };
    let attributes = metadata.file_attributes();
    (
        windows_attributes_are_hidden(attributes),
        windows_attributes_are_system(attributes),
    )
}

#[cfg(windows)]
fn is_system(item: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    item.metadata()
        .map(|metadata| windows_attributes_are_system(metadata.file_attributes()))
        .unwrap_or(true)
}

#[cfg(windows)]
fn windows_attributes_are_hidden(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    attributes & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(windows)]
fn windows_attributes_are_system(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    attributes & FILE_ATTRIBUTE_SYSTEM != 0
}

#[cfg(not(windows))]
fn visibility_attributes(item: &fs::DirEntry, _metadata: Option<&fs::Metadata>) -> (bool, bool) {
    item.file_name()
        .to_str()
        .map(|n| n.starts_with('.'))
        .map(|hidden| (hidden, false))
        .unwrap_or((false, false))
}

#[cfg(not(windows))]
fn is_system(_item: &fs::DirEntry) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: None,
            modified: None,
            created: None,
            extension: None,
            archive_member: None,
        }
    }

    #[test]
    fn sorts_dirs_first_then_by_name() {
        let mut v = vec![
            entry("zebra.txt", false),
            entry("Beta", true),
            entry("alpha.txt", false),
            entry("alpha", true),
        ];
        sort_entries(&mut v);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta", "alpha.txt", "zebra.txt"]);
    }

    #[test]
    fn sort_is_case_insensitive() {
        let mut v = vec![entry("B", false), entry("a", false), entry("C", false)];
        sort_entries(&mut v);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a", "B", "C"]);
    }

    #[test]
    fn hidden_entries_are_filtered_by_default_and_can_be_shown() {
        let visible = entry("visible.txt", false);
        let mut hidden = entry("desktop.ini", false);
        hidden.hidden = true;
        let mut system = entry("system-only.dat", false);
        system.system = true;
        let entries = vec![visible, hidden, system];

        let default_entries = filter_hidden(entries.clone(), false);
        assert_eq!(default_entries.len(), 1);
        assert_eq!(default_entries[0].name, "visible.txt");
        let shown_hidden = filter_hidden(entries, true);
        assert_eq!(shown_hidden.len(), 2);
        assert!(shown_hidden.iter().any(|entry| entry.name == "desktop.ini"));
        assert!(
            !shown_hidden
                .iter()
                .any(|entry| entry.name == "system-only.dat")
        );
    }

    #[test]
    fn cancellation_aborts_scan() {
        let cancel = AtomicBool::new(true);
        let dir = std::env::temp_dir();
        let res = read_dir(&dir, &cancel);
        assert!(matches!(res, Err(ReadDirError::Cancelled)));
    }
}

#[cfg(test)]
mod tests_extra {
    use super::*;
    use std::fs as stdfs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fm_fs_{name}_{}", std::process::id()));
        let _ = stdfs::remove_dir_all(&d);
        stdfs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn read_subdirs_only_dirs_sorted_ci() {
        let d = tmp("subdirs");
        stdfs::create_dir(d.join("b_dir")).unwrap();
        stdfs::create_dir(d.join("A_dir")).unwrap();
        stdfs::write(d.join("file.txt"), b"x").unwrap();
        let got = read_subdirs(&d).unwrap();
        let names: Vec<String> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_lowercase())
            .collect();
        assert_eq!(names, vec!["a_dir", "b_dir"]);
        stdfs::remove_dir_all(&d).ok();
    }

    #[test]
    fn read_dir_populates_entry_fields() {
        let d = tmp("fields");
        stdfs::write(d.join("note.md"), b"abc").unwrap();
        stdfs::create_dir(d.join("sub")).unwrap();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entries = read_dir(&d, &cancel).unwrap();
        let f = entries.iter().find(|e| e.name == "note.md").unwrap();
        assert!(!f.is_dir);
        assert_eq!(f.size, Some(3));
        assert!(f.modified.is_some());
        assert_eq!(f.extension.as_deref(), Some("md"));
        let dir = entries.iter().find(|e| e.name == "sub").unwrap();
        assert!(dir.is_dir && dir.size.is_none());
        stdfs::remove_dir_all(&d).ok();
    }

    #[test]
    fn extensions_are_lowercase_and_absent_for_extensionless_files() {
        let d = tmp("extensions");
        stdfs::write(d.join("ARCHIVE.TAR.GZ"), b"x").unwrap();
        stdfs::write(d.join("README"), b"x").unwrap();
        stdfs::write(d.join(".profile"), b"x").unwrap();
        let entries = read_dir(&d, &AtomicBool::new(false)).unwrap();

        assert_eq!(
            entries
                .iter()
                .find(|e| e.name == "ARCHIVE.TAR.GZ")
                .unwrap()
                .extension
                .as_deref(),
            Some("gz")
        );
        assert_eq!(
            entries
                .iter()
                .find(|e| e.name == "README")
                .unwrap()
                .extension,
            None
        );
        assert_eq!(
            entries
                .iter()
                .find(|e| e.name == ".profile")
                .unwrap()
                .extension,
            None
        );
        stdfs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod tests_v02 {
    use super::*;
    use std::fs as stdfs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fm_v02_{name}_{}", std::process::id()));
        let _ = stdfs::remove_dir_all(&d);
        stdfs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn listing_records_logical_size_without_allocated_size() {
        let d = tmp("logical");
        stdfs::write(d.join("note.txt"), b"hi").unwrap();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entries = read_dir(&d, &cancel).unwrap();
        let note = entries
            .iter()
            .find(|entry| entry.name == "note.txt")
            .unwrap();
        assert_eq!(note.size, Some(2));
        assert_eq!(listing_extension(false, &note.path).as_deref(), Some("txt"));
        stdfs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unicode_names_roundtrip() {
        let d = tmp("uni");
        stdfs::write(d.join("отчёт_файл.txt"), b"x").unwrap();
        stdfs::create_dir(d.join("папка_日本語")).unwrap();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entries = read_dir(&d, &cancel).unwrap();
        assert!(entries.iter().any(|e| e.name == "отчёт_файл.txt"));
        assert!(entries.iter().any(|e| e.name == "папка_日本語" && e.is_dir));
        stdfs::remove_dir_all(&d).ok();
    }

    #[test]
    fn read_dir_missing_errors() {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(read_dir(std::path::Path::new("Z:\\no\\such\\dir_fm"), &cancel).is_err());
    }
}

#[cfg(test)]
mod tests_acc {
    use super::*;
    use std::fs as stdfs;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    fn cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fm_acc_{name}_{}", std::process::id()));
        let _ = stdfs::remove_dir_all(&d);
        stdfs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn emoji_and_very_long_names() {
        let d = tmp("names");
        stdfs::write(d.join("файл 📁 emoji.txt"), b"x").unwrap();
        let long = format!("{}.txt", "n".repeat(180));
        stdfs::write(d.join(&long), b"x").unwrap();
        let entries = read_dir(&d, &cancel()).unwrap();
        assert!(entries.iter().any(|e| e.name.contains("📁")));
        assert!(entries.iter().any(|e| e.name.len() > 150));
        stdfs::remove_dir_all(&d).ok();
    }

    #[test]
    fn big_dir_reads_fast() {
        let d = tmp("big");
        for i in 0..1500 {
            stdfs::write(d.join(format!("f{i:04}.txt")), b"x").unwrap();
        }
        let t = Instant::now();
        let entries = read_dir(&d, &cancel()).unwrap();
        assert_eq!(entries.len(), 1500);
        assert!(t.elapsed().as_secs() < 10, "read_dir too slow");
        stdfs::remove_dir_all(&d).ok();
    }

    #[test]
    fn file_as_dir_errors() {
        let d = tmp("fdir");
        let f = d.join("x.txt");
        stdfs::write(&f, b"x").unwrap();
        assert!(read_dir(&f, &cancel()).is_err());
        stdfs::remove_dir_all(&d).ok();
    }
}

#[cfg(all(test, windows))]
mod tests_windows_metadata {
    use super::*;

    #[test]
    fn hidden_and_system_attributes_are_tracked_separately() {
        assert!(!windows_attributes_are_hidden(0));
        assert!(windows_attributes_are_hidden(0x2));
        assert!(!windows_attributes_are_hidden(0x4));
        assert!(!windows_attributes_are_system(0x2));
        assert!(windows_attributes_are_system(0x4));
    }

    #[test]
    fn directory_listing_reports_hidden_and_readonly_windows_attributes() {
        let dir = std::env::temp_dir().join(format!("fm-windows-metadata-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("attributes.txt");
        fs::write(&file, b"metadata").unwrap();
        crate::platform::set_attributes(&file, true, true).unwrap();

        let entries = read_dir(&dir, &AtomicBool::new(false)).unwrap();
        let item = entries.iter().find(|e| e.name == "attributes.txt").unwrap();
        assert!(item.hidden);
        assert!(item.readonly);

        crate::platform::set_attributes(&file, false, false).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod tests_v06 {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: None,
            modified: None,
            created: None,
            extension: None,
            archive_member: None,
        }
    }

    #[test]
    fn sort_by_size_dirs_stay_first() {
        let mut v = vec![
            entry("z.txt", false),
            {
                let mut e = entry("a.txt", false);
                e.size = Some(10);
                e
            },
            {
                let mut e = entry("m.txt", false);
                e.size = Some(1);
                e
            },
            entry("dir", true),
        ];
        v[0].size = Some(99);
        sort_entries_by(&mut v, SortKey::Size, false);
        assert_eq!(v[0].name, "dir");
        assert_eq!(v[1].name, "m.txt");
        assert_eq!(v[3].name, "z.txt");
    }

    #[test]
    fn sort_desc_reverses_group() {
        let mut v = vec![
            entry("a.txt", false),
            entry("b.txt", false),
            entry("d", true),
        ];
        sort_entries_by(&mut v, SortKey::Name, true);
        assert_eq!(v[0].name, "d");
        assert_eq!(v[1].name, "b.txt");
        assert_eq!(v[2].name, "a.txt");
    }

    #[test]
    fn sort_by_extension_then_name() {
        let mut v = vec![
            entry("b.txt", false),
            {
                let mut e = entry("a.txt", false);
                e.extension = Some("txt".into());
                e
            },
            {
                let mut e = entry("c.md", false);
                e.extension = Some("md".into());
                e
            },
        ];
        v[0].extension = Some("txt".into());
        sort_entries_by(&mut v, SortKey::Extension, false);
        assert_eq!(v[0].name, "c.md");
        assert_eq!(v[1].name, "a.txt");
        assert_eq!(v[2].name, "b.txt");
    }

    #[test]
    fn sort_by_modified_newer_last_on_asc() {
        let t1 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100);
        let t2 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(200);
        let mut v = vec![
            {
                let mut e = entry("new.txt", false);
                e.modified = Some(t2);
                e
            },
            {
                let mut e = entry("old.txt", false);
                e.modified = Some(t1);
                e
            },
        ];
        sort_entries_by(&mut v, SortKey::Modified, false);
        assert_eq!(v[0].name, "old.txt");
        assert_eq!(v[1].name, "new.txt");
    }

    #[test]
    fn equal_keys_fall_back_to_name() {
        let mut v = vec![
            {
                let mut e = entry("b.bin", false);
                e.size = Some(5);
                e
            },
            {
                let mut e = entry("a.bin", false);
                e.size = Some(5);
                e
            },
        ];
        sort_entries_by(&mut v, SortKey::Size, false);
        assert_eq!(v[0].name, "a.bin");
    }
}
