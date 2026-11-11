//! Чтение каталога ZIP и поиск внутри архива.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::fs::{FileEntry, ReadDirError};
use crate::core::search::{self, SearchOptions};

use super::common::*;

#[cfg(test)]
pub(super) fn read_zip_dir(archive: &Path, prefix: &str) -> io::Result<Vec<FileEntry>> {
    read_zip_dir_with_cancel(archive, prefix, None)
}

pub fn read_zip_dir_cancellable(
    archive: &Path,
    prefix: &str,
    cancel: &AtomicBool,
) -> Result<Vec<FileEntry>, ReadDirError> {
    read_zip_dir_with_cancel(archive, prefix, Some(cancel)).map_err(read_dir_error)
}

pub(super) fn read_zip_dir_with_cancel(
    archive: &Path,
    prefix: &str,
    cancel: Option<&AtomicBool>,
) -> io::Result<Vec<FileEntry>> {
    let prefix = normalize_prefix(prefix)?;
    let members = list_members_with_cancel(archive, cancel)?;
    let prefix_with_slash = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    };
    let mut children = BTreeMap::<String, (String, bool)>::new();

    for (index, member) in members.into_iter().enumerate() {
        if index % 256 == 0 && cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "archive listing cancelled",
            ));
        }
        let Some(relative) = member.path.strip_prefix(&prefix_with_slash) else {
            if member.path == prefix {
                continue;
            }
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        let (name, nested) = match relative.split_once('/') {
            Some((name, _)) => (name, true),
            None => (relative, false),
        };
        let child_path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = nested || member.is_dir;
        children
            .entry(name.to_owned())
            .and_modify(|(path, existing_dir)| {
                *existing_dir |= is_dir;
                if path.is_empty() {
                    *path = child_path.clone();
                }
            })
            .or_insert((child_path, is_dir));
    }

    let mut entries = Vec::with_capacity(children.len());
    for (name, (member, is_dir)) in children {
        let extension = if is_dir {
            None
        } else {
            Path::new(&name)
                .extension()
                .map(|value| value.to_string_lossy().to_lowercase())
        };
        entries.push(FileEntry {
            hidden: name.starts_with('.'),
            system: false,
            name,
            path: archive.to_path_buf(),
            is_dir,
            is_symlink: false,
            readonly: true,
            size: None,
            modified: None,
            created: None,
            extension,
            archive_member: Some(member),
        });
    }
    crate::core::fs::sort_entries(&mut entries);
    Ok(entries)
}

pub fn search_zip(
    archive: &Path,
    prefix: &str,
    options: &SearchOptions,
    cancel: &AtomicBool,
) -> Result<Vec<FileEntry>, ReadDirError> {
    if options.min_size.is_some()
        || options.max_size.is_some()
        || options.modified_from.is_some()
        || options.modified_before.is_some()
    {
        return Err(ReadDirError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "ZIP member size and date filters are unavailable",
        )));
    }

    let prefix = normalize_prefix(prefix).map_err(ReadDirError::Io)?;
    let mut entries = if options.recursive {
        let members = list_members_with_cancel(archive, Some(cancel)).map_err(read_dir_error)?;
        let prefix_with_slash = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        let mut found = BTreeMap::<String, FileEntry>::new();
        for (index, member) in members.into_iter().enumerate() {
            if index % 256 == 0 && cancel.load(Ordering::Relaxed) {
                return Err(ReadDirError::Cancelled);
            }
            let relative = if prefix.is_empty() {
                member.path.as_str()
            } else if let Some(relative) = member.path.strip_prefix(&prefix_with_slash) {
                relative
            } else {
                continue;
            };
            if relative.is_empty() {
                continue;
            }
            let parts: Vec<_> = relative.split('/').collect();
            for depth in 1..parts.len() {
                let dir_name = parts[..depth].join("/");
                let dir_member = if prefix.is_empty() {
                    dir_name.clone()
                } else {
                    format!("{prefix}/{dir_name}")
                };
                found
                    .entry(dir_name.clone())
                    .or_insert_with(|| zip_entry(archive, &dir_member, &dir_name, true));
            }
            found
                .entry(relative.to_owned())
                .or_insert_with(|| zip_entry(archive, &member.path, relative, member.is_dir));
        }
        found.into_values().collect()
    } else {
        if cancel.load(Ordering::Relaxed) {
            return Err(ReadDirError::Cancelled);
        }
        read_zip_dir_with_cancel(archive, &prefix, Some(cancel)).map_err(read_dir_error)?
    };

    entries.retain(|entry| search::matches_entry(entry, options));
    entries.sort_by(|a, b| {
        let score_a = search::name_score(&a.name, &options.query, options.fuzzy);
        let score_b = search::name_score(&b.name, &options.query, options.fuzzy);
        score_b
            .cmp(&score_a)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

pub(super) fn zip_entry(archive: &Path, member: &str, name: &str, is_dir: bool) -> FileEntry {
    let extension = if is_dir {
        None
    } else {
        Path::new(name)
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
    };
    FileEntry {
        name: name.to_owned(),
        path: archive.to_path_buf(),
        is_dir,
        is_symlink: false,
        hidden: name.rsplit('/').next().unwrap_or(name).starts_with('.'),
        system: false,
        readonly: true,
        size: None,
        modified: None,
        created: None,
        extension,
        archive_member: Some(member.to_owned()),
    }
}

