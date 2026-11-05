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
    pub readonly: bool,
    /// `None` for directories (their size is not computed on listing).
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    /// Lowercase extension without the dot, e.g. `txt`.
    pub extension: Option<String>,
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

pub fn sort_entries(entries: &mut Vec<FileEntry>) {
    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));
}

fn map_entry(item: &fs::DirEntry) -> Option<FileEntry> {
    let path = item.path();
    let name = item.file_name().to_string_lossy().into_owned();
    let file_type = item.file_type().ok()?;

    // metadata() follows symlinks; on Windows it reuses the find data
    // already fetched by the directory enumeration, so it is cheap.
    let meta = item.metadata().ok();

    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(file_type.is_dir());
    let size = if is_dir { None } else { meta.as_ref().map(|m| m.len()) };
    let modified = meta.as_ref().and_then(|m| m.modified().ok());
    let created = meta.as_ref().and_then(|m| m.created().ok());
    let readonly = meta
        .as_ref()
        .map(|m| m.permissions().readonly())
        .unwrap_or(false);

    let extension = if is_dir {
        None
    } else {
        path.extension()
            .map(|e| e.to_string_lossy().to_lowercase())
    };

    Some(FileEntry {
        name,
        path,
        is_dir,
        is_symlink: file_type.is_symlink(),
        hidden: is_hidden(item),
        readonly,
        size,
        modified,
        created,
        extension,
    })
}

/// Lists immediate subdirectories of `path`, sorted case-insensitively.
/// Used by the folder tree; directory entries only, no file metadata.
pub fn read_subdirs(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        if item.file_type().map(|t| t.is_dir()).unwrap_or(false) {
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
fn is_hidden(item: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    item.metadata()
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_hidden(item: &fs::DirEntry) -> bool {
    item.file_name()
        .to_str()
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
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
            readonly: false,
            size: None,
            modified: None,
            created: None,
            extension: None,
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
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
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
