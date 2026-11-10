use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ::windows::Win32::Storage::FileSystem::MoveFileW;
use ::windows::core::PCWSTR;

use crate::core::fs::FileEntry;
use crate::core::fs::ReadDirError;
use crate::core::search::{self, SearchOptions};
use crate::system_tools::system_executable;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

const MAX_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE_LISTING_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARCHIVE_MEMBERS: usize = 100_000;
const MAX_ARCHIVE_NAME_ATTEMPTS: u32 = 10_000;
const COPY_BUFFER_SIZE: usize = 64 * 1024;

static NEXT_STAGE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArchiveMember {
    path: String,
    is_dir: bool,
}

#[cfg(test)]
fn read_zip_dir(archive: &Path, prefix: &str) -> io::Result<Vec<FileEntry>> {
    read_zip_dir_with_cancel(archive, prefix, None)
}

pub fn read_zip_dir_cancellable(
    archive: &Path,
    prefix: &str,
    cancel: &AtomicBool,
) -> Result<Vec<FileEntry>, ReadDirError> {
    read_zip_dir_with_cancel(archive, prefix, Some(cancel)).map_err(read_dir_error)
}

fn read_zip_dir_with_cancel(
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

fn zip_entry(archive: &Path, member: &str, name: &str, is_dir: bool) -> FileEntry {
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
        readonly: true,
        size: None,
        modified: None,
        created: None,
        extension,
        archive_member: Some(member.to_owned()),
    }
}

fn list_members(archive: &Path) -> io::Result<Vec<ArchiveMember>> {
    list_members_with_cancel(archive, None)
}

fn list_members_with_cancel(
    archive: &Path,
    cancel: Option<&AtomicBool>,
) -> io::Result<Vec<ArchiveMember>> {
    let tar = system_executable("tar.exe")?;
    let mut child = Command::new(tar)
        .args(["-tf"])
        .arg(archive)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| io::Error::other("could not start the archive reader"))?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("archive reader did not return a listing"));
    };
    let mut listing = Vec::new();
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            drop(stdout);
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "archive listing cancelled",
            ));
        }
        let count = match stdout.read(&mut buffer) {
            Ok(count) => count,
            Err(error) => {
                drop(stdout);
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if count == 0 {
            break;
        }
        if listing.len().saturating_add(count) as u64 > MAX_ARCHIVE_LISTING_BYTES {
            drop(stdout);
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive listing exceeds the supported limit",
            ));
        }
        listing.extend_from_slice(&buffer[..count]);
    }
    drop(stdout);
    if !child.wait()?.success() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive is damaged or unsupported",
        ));
    }
    let listing = String::from_utf8(listing).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive contains unsupported filename encoding",
        )
    })?;
    let mut seen = HashSet::new();
    let mut members = Vec::new();
    for raw in listing.lines() {
        if let Some(member) = normalize_member(raw)? {
            if !seen.insert(member.path.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "archive contains duplicate paths",
                ));
            }
            members.push(member);
            if members.len() > MAX_ARCHIVE_MEMBERS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "archive contains too many entries",
                ));
            }
        }
    }
    Ok(members)
}

fn read_dir_error(error: io::Error) -> ReadDirError {
    if error.kind() == io::ErrorKind::Interrupted {
        ReadDirError::Cancelled
    } else {
        ReadDirError::Io(error)
    }
}

fn normalize_prefix(prefix: &str) -> io::Result<String> {
    if prefix.is_empty() {
        Ok(String::new())
    } else {
        normalize_member(prefix)?
            .map(|member| member.path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid archive folder"))
    }
}

fn normalize_member(raw: &str) -> io::Result<Option<ArchiveMember>> {
    let mut path = raw;
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    let is_dir = path.ends_with('/');
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "." {
        return Ok(None);
    }
    if path.starts_with('/')
        || path.chars().any(|ch| matches!(ch, '\\' | ':' | '\0'))
        || path.split('/').any(|part| !valid_component(part))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive contains an unsafe path",
        ));
    }
    Ok(Some(ArchiveMember {
        path: path.to_owned(),
        is_dir,
    }))
}

fn valid_component(part: &str) -> bool {
    if part.is_empty()
        || part == "."
        || part == ".."
        || part.ends_with('.')
        || part.ends_with(' ')
        || part
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return false;
    }
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(|ch| ch == '.' || ch == ' ')
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) && !reserved_numbered_device(&stem, "COM")
        && !reserved_numbered_device(&stem, "LPT")
}

fn reserved_numbered_device(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix).is_some_and(|suffix| {
        matches!(
            suffix,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    })
}

pub fn extract_zip_file(archive: &Path, member: &str, destination: &Path) -> io::Result<PathBuf> {
    let member = normalize_member(member)?
        .filter(|member| !member.is_dir)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an archive file"))?;
    if !list_members(archive)?
        .iter()
        .any(|item| !item.is_dir && item.path == member.path)
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "archive member was not found",
        ));
    }
    let destination = if destination.as_os_str().is_empty() {
        Path::new(".")
    } else {
        destination
    };
    fs::create_dir_all(destination)?;
    let filename =
        member.path.rsplit('/').next().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid archive filename")
        })?;
    let output = destination.join(filename);
    let mut total = 0;
    extract_member_to(archive, &member.path, &output, &mut total)?;
    Ok(output)
}

pub fn extract_zip_folder(archive: &Path, folder: &str, destination: &Path) -> io::Result<PathBuf> {
    let folder = normalize_prefix(folder)?;
    if folder.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive folder was not specified",
        ));
    }
    let members = list_members(archive)?;
    if !members.iter().any(|member| {
        (member.path == folder && member.is_dir) || member.path.starts_with(&format!("{folder}/"))
    }) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "archive folder was not found",
        ));
    }
    let output = join_member(destination, &folder)?;
    extract_tree(archive, &members, Some(&folder), destination, &output)
}

pub fn extract_zip_all(archive: &Path, destination: &Path) -> io::Result<PathBuf> {
    let name = archive
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid archive name"))?;
    let output_name = normalize_member(name)?
        .filter(|member| !member.is_dir)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid archive name"))?;
    let members = list_members(archive)?;
    let output = join_member(destination, &output_name.path)?;
    extract_tree(archive, &members, None, destination, &output)
}

pub fn create_zip_archive(sources: &[PathBuf], destination: &Path) -> io::Result<PathBuf> {
    if sources.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no files were selected",
        ));
    }

    let display_root = destination.to_path_buf();
    let root = fs::canonicalize(destination)?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive destination is not a directory",
        ));
    }
    let archive_stem = zip_archive_stem(sources)?;
    let mut entries = Vec::with_capacity(sources.len());
    let mut selected_names = HashSet::new();
    for source in sources {
        let metadata = fs::symlink_metadata(source)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "symbolic links cannot be added to a ZIP archive",
            ));
        }
        let canonical_source = fs::canonicalize(source)?;
        let relative = canonical_source.strip_prefix(&root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "selected item is outside the current folder",
            )
        })?;
        let member = zip_member_name(relative, metadata.is_dir())?;
        if selected_names.insert(member.clone()) {
            entries.push((canonical_source, member));
        }
    }
    if entries.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no archive entries were selected",
        ));
    }

    // Сначала собираем архив отдельно, затем переносим его без перезаписи.
    let stage = create_stage_dir(&root)?;
    let staged_archive = stage.join("archive.zip");
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_archive)?;
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut total_bytes = 0u64;
        let mut total_name_bytes = 0u64;
        let mut member_count = 0usize;
        for (source, member) in &entries {
            write_zip_path(
                &mut writer,
                &root,
                source,
                member,
                options,
                &mut total_bytes,
                &mut total_name_bytes,
                &mut member_count,
            )?;
        }
        let archive_file = writer.finish().map_err(zip_writer_error)?;
        archive_file.sync_all()?;
        drop(archive_file);
        move_zip_to_unique_name(&staged_archive, &root, &archive_stem)
    })();
    let _ = fs::remove_dir_all(&stage);
    result.and_then(|path| {
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "created archive has no filename",
            )
        })?;
        Ok(display_root.join(name))
    })
}

fn zip_archive_stem(sources: &[PathBuf]) -> io::Result<String> {
    if sources.len() != 1 {
        return Ok("archive".to_owned());
    }
    let source = &sources[0];
    let metadata = fs::symlink_metadata(source)?;
    let name = if metadata.is_dir() {
        source.file_name()
    } else {
        source.file_stem().or_else(|| source.file_name())
    }
    .and_then(|name| name.to_str())
    .filter(|name| !name.is_empty())
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "filename encoding is unsupported",
        )
    })?;
    Ok(if valid_component(name) {
        name.to_owned()
    } else {
        "archive".to_owned()
    })
}

fn zip_member_name(relative: &Path, is_dir: bool) -> io::Result<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                parts.push(part.to_str().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "filename encoding is unsupported",
                    )
                })?);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "selected item has an unsafe path",
                ));
            }
        }
    }
    let name = parts.join("/");
    let name = if is_dir { format!("{name}/") } else { name };
    let member = normalize_member(&name)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty archive path"))?;
    Ok(if member.is_dir {
        format!("{}/", member.path)
    } else {
        member.path
    })
}

fn write_zip_path<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    root: &Path,
    path: &Path,
    member: &str,
    options: SimpleFileOptions,
    total_bytes: &mut u64,
    total_name_bytes: &mut u64,
    member_count: &mut usize,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    // Не следуем по ссылкам, чтобы не включать в архив данные вне выбранной папки.
    if file_type.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "symbolic links cannot be added to a ZIP archive",
        ));
    }
    let canonical_path = fs::canonicalize(path)?;
    if !canonical_path.starts_with(root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "selected path resolves outside the current folder",
        ));
    }
    *member_count = member_count.saturating_add(1);
    if *member_count > MAX_ARCHIVE_MEMBERS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "selected content exceeds the ZIP member limit",
        ));
    }
    *total_name_bytes = total_name_bytes
        .checked_add(member.len() as u64 + 2)
        .filter(|size| *size <= MAX_ARCHIVE_LISTING_BYTES)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "selected content exceeds the ZIP listing limit",
            )
        })?;

    if metadata.is_dir() {
        let directory_name = if member.ends_with('/') {
            member.to_owned()
        } else {
            format!("{member}/")
        };
        writer
            .add_directory(directory_name, options)
            .map_err(zip_writer_error)?;
        for child in fs::read_dir(&canonical_path)? {
            let child = child?;
            let child_path = child.path();
            let child_metadata = fs::symlink_metadata(&child_path)?;
            let canonical_child = fs::canonicalize(&child_path)?;
            let child_relative = canonical_child.strip_prefix(root).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "folder contents are outside the current folder",
                )
            })?;
            let child_member = zip_member_name(child_relative, child_metadata.is_dir())?;
            write_zip_path(
                writer,
                root,
                &canonical_child,
                &child_member,
                options,
                total_bytes,
                total_name_bytes,
                member_count,
            )?;
        }
    } else if metadata.is_file() {
        writer
            .start_file(member.trim_end_matches('/'), options)
            .map_err(zip_writer_error)?;
        let mut source = fs::File::open(&canonical_path)?;
        copy_with_limit(
            &mut source,
            writer,
            total_bytes,
            "selected content exceeds the ZIP size limit",
        )?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "selected item is not a regular file or folder",
        ));
    }
    Ok(())
}

fn zip_writer_error(error: zip::result::ZipError) -> io::Error {
    match error {
        zip::result::ZipError::Io(error) => error,
        error => io::Error::new(io::ErrorKind::InvalidData, error.to_string()),
    }
}

fn move_zip_to_unique_name(
    staged_archive: &Path,
    parent: &Path,
    stem: &str,
) -> io::Result<PathBuf> {
    for suffix in 1..=MAX_ARCHIVE_NAME_ATTEMPTS {
        let filename = if suffix == 1 {
            format!("{stem}.zip")
        } else {
            format!("{stem} ({suffix}).zip")
        };
        let destination = parent.join(filename);
        match move_file_no_replace(staged_archive, &destination) {
            Ok(()) => return Ok(destination),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not choose an unused archive name",
    ))
}

fn extract_tree(
    archive: &Path,
    members: &[ArchiveMember],
    prefix: Option<&str>,
    destination_root: &Path,
    output: &Path,
) -> io::Result<PathBuf> {
    let destination_root = if destination_root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        destination_root
    };
    match fs::symlink_metadata(output) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination already exists",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(destination_root)?;
    let canonical_root = fs::canonicalize(destination_root)?;
    let mut existing_parent = parent;
    while !existing_parent.exists() {
        existing_parent = existing_parent
            .parent()
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
    }
    let canonical_existing_parent = fs::canonicalize(existing_parent)?;
    if !canonical_existing_parent.starts_with(&canonical_root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe extraction destination",
        ));
    }
    fs::create_dir_all(parent)?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe extraction destination",
        ));
    }
    let stage = create_stage_dir(parent)?;
    let result = (|| {
        let mut total = 0u64;
        for member in members {
            let relative = match prefix {
                Some(prefix) if member.path == prefix => "",
                Some(prefix) => {
                    let prefix = format!("{prefix}/");
                    let Some(relative) = member.path.strip_prefix(&prefix) else {
                        continue;
                    };
                    relative
                }
                None => member.path.as_str(),
            };
            if relative.is_empty() {
                continue;
            }
            let destination = join_member(&stage, relative)?;
            if member.is_dir {
                fs::create_dir_all(destination)?;
            } else {
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                extract_member_to(archive, &member.path, &destination, &mut total)?;
            }
        }
        move_directory_no_replace(&stage, output)?;
        Ok(output.to_path_buf())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

fn move_directory_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination_wide: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
        )
    }
    .map_err(|error| {
        if fs::symlink_metadata(destination).is_ok() {
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists")
        } else {
            io::Error::other(error.to_string())
        }
    })
}

fn move_file_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination_wide: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
        )
    }
    .map_err(|error| {
        if fs::symlink_metadata(destination).is_ok() {
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists")
        } else {
            io::Error::other(error.to_string())
        }
    })
}

fn create_stage_dir(parent: &Path) -> io::Result<PathBuf> {
    for _ in 0..16 {
        let id = NEXT_STAGE_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".filemanager-zip-stage-{}-{id}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a temporary extraction folder",
    ))
}

fn join_member(root: &Path, member: &str) -> io::Result<PathBuf> {
    let member = normalize_member(member)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty archive path"))?;
    let mut path = root.to_path_buf();
    for component in member.path.split('/') {
        path.push(component);
    }
    Ok(path)
}

fn extract_member_to(
    archive: &Path,
    member: &str,
    destination: &Path,
    total: &mut u64,
) -> io::Result<()> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = (|| {
        let tar = system_executable("tar.exe")?;
        let mut child = Command::new(tar)
            .args(["-xOf"])
            .arg(archive)
            .arg("--")
            .arg(member)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| io::Error::other("could not start the archive extractor"))?;
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(
                "archive extractor did not return file data",
            ));
        };
        if let Err(error) = copy_with_limit(
            &mut stdout,
            &mut output,
            total,
            "archive expands beyond the extraction limit",
        ) {
            drop(stdout);
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        drop(stdout);
        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive member could not be extracted",
            ));
        }
        output.sync_all()?;
        Ok(())
    })();
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

fn copy_with_limit(
    reader: &mut impl Read,
    writer: &mut impl Write,
    total: &mut u64,
    limit_error: &'static str,
) -> io::Result<()> {
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        let next = total
            .checked_add(count as u64)
            .filter(|size| *size <= MAX_UNCOMPRESSED_BYTES)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, limit_error))?;
        writer.write_all(&buffer[..count])?;
        *total = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn push_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    0xEDB8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut local = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let name = name.as_bytes();
            let crc = crc32(data);
            let offset = local.len() as u32;

            push_u32(&mut local, 0x0403_4B50);
            push_u16(&mut local, 20);
            push_u16(&mut local, 0);
            push_u16(&mut local, 0);
            push_u16(&mut local, 0);
            push_u16(&mut local, 0);
            push_u32(&mut local, crc);
            push_u32(&mut local, data.len() as u32);
            push_u32(&mut local, data.len() as u32);
            push_u16(&mut local, name.len() as u16);
            push_u16(&mut local, 0);
            local.extend_from_slice(name);
            local.extend_from_slice(data);

            push_u32(&mut central, 0x0201_4B50);
            push_u16(&mut central, 20);
            push_u16(&mut central, 20);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, crc);
            push_u32(&mut central, data.len() as u32);
            push_u32(&mut central, data.len() as u32);
            push_u16(&mut central, name.len() as u16);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u16(&mut central, 0);
            push_u32(&mut central, 0);
            push_u32(&mut central, offset);
            central.extend_from_slice(name);
        }

        let central_offset = local.len() as u32;
        let central_size = central.len() as u32;
        let count = entries.len() as u16;
        local.extend_from_slice(&central);
        push_u32(&mut local, 0x0605_4B50);
        push_u16(&mut local, 0);
        push_u16(&mut local, 0);
        push_u16(&mut local, count);
        push_u16(&mut local, count);
        push_u32(&mut local, central_size);
        push_u32(&mut local, central_offset);
        push_u16(&mut local, 0);
        local
    }

    fn temp_dir() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("filemanager-zip-{id}"));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn rejects_traversal_and_windows_device_paths() {
        assert!(normalize_member("../outside.txt").is_err());
        assert!(normalize_member("C:/outside.txt").is_err());
        assert!(normalize_member("folder/CON.txt").is_err());
        assert!(normalize_member("folder/COM¹.txt").is_err());
        assert_eq!(
            normalize_member("./folder/file.txt").unwrap().unwrap().path,
            "folder/file.txt"
        );
    }

    #[test]
    fn rejects_an_archive_with_a_traversal_member() {
        let root = temp_dir();
        let archive = root.join("unsafe.zip");
        fs::write(&archive, zip_bytes(&[("../outside.txt", b"no")])).unwrap();
        assert_eq!(
            read_zip_dir(&archive, "").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lists_zip_directories_and_extracts_files_without_overwriting() {
        let root = temp_dir();
        let archive = root.join("sample.zip");
        fs::write(
            &archive,
            zip_bytes(&[
                ("folder/", b""),
                ("folder/readme.txt", b"hello from zip"),
                ("root.txt", b"root"),
            ]),
        )
        .unwrap();

        let root_entries = read_zip_dir(&archive, "").unwrap();
        assert!(
            root_entries
                .iter()
                .any(|entry| entry.name == "folder" && entry.is_dir)
        );
        assert!(
            root_entries
                .iter()
                .any(|entry| entry.name == "root.txt" && !entry.is_dir)
        );
        let nested = read_zip_dir(&archive, "folder").unwrap();
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].name, "readme.txt");
        assert_eq!(
            nested[0].archive_member.as_deref(),
            Some("folder/readme.txt")
        );

        let cancel = AtomicBool::new(false);
        let options =
            crate::core::search::parse_options("read", true, false, ".txt", "", "", "", "")
                .unwrap();
        let results = search_zip(&archive, "", &options, &cancel).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "folder/readme.txt");

        let single_dir = root.join("single");
        let extracted = extract_zip_file(&archive, "folder/readme.txt", &single_dir).unwrap();
        assert_eq!(fs::read(&extracted).unwrap(), b"hello from zip");
        assert_eq!(
            extract_zip_file(&archive, "folder/readme.txt", &single_dir)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );

        let folder_root = root.join("folder-output");
        let folder = extract_zip_folder(&archive, "folder", &folder_root).unwrap();
        assert_eq!(
            fs::read(folder.join("readme.txt")).unwrap(),
            b"hello from zip"
        );
        assert_eq!(
            extract_zip_folder(&archive, "folder", &folder_root)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );

        let all_root = root.join("all-output");
        let all = extract_zip_all(&archive, &all_root).unwrap();
        assert_eq!(fs::read(all.join("root.txt")).unwrap(), b"root");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creates_zip_from_files_and_folders_without_overwriting() {
        let root = temp_dir();
        let file = root.join("readme.txt");
        let second_file = root.join("todo.md");
        let folder = root.join("folder");
        let second_folder = root.join("empty-folder");
        fs::create_dir_all(folder.join("nested")).unwrap();
        fs::create_dir_all(&second_folder).unwrap();
        fs::write(&file, b"selected file").unwrap();
        fs::write(&second_file, b"second selected file").unwrap();
        fs::write(folder.join("nested").join("payload.bin"), b"nested file").unwrap();

        let file_archive = create_zip_archive(std::slice::from_ref(&file), &root).unwrap();
        assert_eq!(
            file_archive.file_name().and_then(|name| name.to_str()),
            Some("readme.zip")
        );
        assert!(
            read_zip_dir(&file_archive, "")
                .unwrap()
                .iter()
                .any(|entry| entry.name == "readme.txt" && !entry.is_dir)
        );

        let folder_archive = create_zip_archive(std::slice::from_ref(&folder), &root).unwrap();
        assert_eq!(
            folder_archive.file_name().and_then(|name| name.to_str()),
            Some("folder.zip")
        );
        assert!(
            read_zip_dir(&folder_archive, "folder/nested")
                .unwrap()
                .iter()
                .any(|entry| entry.name == "payload.bin" && !entry.is_dir)
        );

        let sources = vec![
            file.clone(),
            second_file.clone(),
            folder.clone(),
            second_folder.clone(),
        ];
        let archive = create_zip_archive(&sources, &root).unwrap();
        assert_eq!(
            archive.file_name().and_then(|name| name.to_str()),
            Some("archive.zip")
        );
        let root_entries = read_zip_dir(&archive, "").unwrap();
        assert!(root_entries.iter().any(|entry| entry.name == "readme.txt"));
        assert!(root_entries.iter().any(|entry| entry.name == "todo.md"));
        assert!(
            root_entries
                .iter()
                .any(|entry| entry.name == "folder" && entry.is_dir)
        );
        assert!(
            root_entries
                .iter()
                .any(|entry| entry.name == "empty-folder" && entry.is_dir)
        );

        let output = extract_zip_all(&archive, &root.join("extracted")).unwrap();
        assert_eq!(
            fs::read(output.join("readme.txt")).unwrap(),
            b"selected file"
        );
        assert_eq!(
            fs::read(output.join("folder/nested/payload.bin")).unwrap(),
            b"nested file"
        );
        assert_eq!(
            fs::read(output.join("todo.md")).unwrap(),
            b"second selected file"
        );

        let second_archive = create_zip_archive(&sources, &root).unwrap();
        assert_eq!(
            second_archive.file_name().and_then(|name| name.to_str()),
            Some("archive (2).zip")
        );
        assert!(read_zip_dir(&archive, "").is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}
