//! Создание ZIP-архива из выбранных файлов и папок.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::platform::{
    ZipCompressionLevel, ZipCompressionMethod, ZipCreateOptions, ZipExistingBehavior,
};
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use super::common::*;

#[allow(dead_code)]
pub fn create_zip_archive(sources: &[PathBuf], options: &ZipCreateOptions) -> io::Result<PathBuf> {
    create_zip_archive_inner(sources, options, None)
}

pub fn create_zip_archive_cancellable(
    sources: &[PathBuf],
    options: &ZipCreateOptions,
    cancel: &AtomicBool,
) -> io::Result<PathBuf> {
    create_zip_archive_inner(sources, options, Some(cancel))
}

fn create_zip_archive_inner(
    sources: &[PathBuf],
    options: &ZipCreateOptions,
    cancel: Option<&AtomicBool>,
) -> io::Result<PathBuf> {
    check_cancel(cancel)?;
    if sources.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no files were selected",
        ));
    }

    let source_root = fs::canonicalize(&options.source_root)?;
    let display_destination = options.destination.clone();
    let destination = fs::canonicalize(&display_destination)?;
    if !source_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive source folder is not a directory",
        ));
    }
    if !destination.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive destination is not a directory",
        ));
    }
    let archive_filename = crate::platform::normalize_zip_archive_name(&options.archive_name)?;
    let archive_stem = archive_filename.strip_suffix(".zip").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid normalized archive name",
        )
    })?;
    let mut entries = Vec::with_capacity(sources.len());
    let mut selected_paths = HashSet::new();
    for source in sources {
        check_cancel(cancel)?;
        let metadata = fs::symlink_metadata(source)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "symbolic links cannot be added to a ZIP archive",
            ));
        }
        let canonical_source = fs::canonicalize(source)?;
        let relative = canonical_source.strip_prefix(&source_root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "selected item is outside the current folder",
            )
        })?;
        if metadata.is_dir() && destination.starts_with(&canonical_source) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive destination is inside a selected folder",
            ));
        }
        let member = if options.preserve_folder_structure {
            zip_member_name(relative, metadata.is_dir())?
        } else if metadata.is_dir() {
            String::new()
        } else {
            zip_flat_file_name(&canonical_source)?
        };
        if selected_paths.insert(canonical_source.clone()) {
            entries.push((canonical_source, member));
        }
    }
    if entries.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no archive entries were selected",
        ));
    }

    // Сначала собираем архив отдельно, затем применяем выбранное поведение при совпадении имени.
    let stage = create_stage_dir(&destination)?;
    let staged_archive = stage.join("archive.zip");
    let result = (|| {
        check_cancel(cancel)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_archive)?;
        let mut writer = ZipWriter::new(file);
        let zip_options = zip_compression_options(options);
        let mut total_bytes = 0u64;
        let mut total_name_bytes = 0u64;
        let mut member_count = 0usize;
        let mut files_written = 0usize;
        let mut seen_files = HashSet::new();
        for (source, member) in &entries {
            write_zip_path(
                &mut writer,
                &source_root,
                source,
                member,
                zip_options,
                options.preserve_folder_structure,
                &mut total_bytes,
                &mut total_name_bytes,
                &mut member_count,
                &mut files_written,
                &mut seen_files,
                cancel,
            )?;
        }
        if !options.preserve_folder_structure && files_written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "selection contains no files",
            ));
        }
        let archive_file = writer.finish().map_err(zip_writer_error)?;
        archive_file.sync_all()?;
        drop(archive_file);
        check_cancel(cancel)?;
        match options.existing_behavior {
            ZipExistingBehavior::AddSuffix => {
                move_zip_to_unique_name(&staged_archive, &destination, archive_stem, cancel)
            }
            ZipExistingBehavior::Replace => {
                let target = destination.join(&archive_filename);
                publish_staged_path(&staged_archive, &target, cancel, move_file_replace)
            }
            ZipExistingBehavior::Ask => {
                let target = destination.join(&archive_filename);
                publish_staged_path(&staged_archive, &target, cancel, move_file_no_replace)
            }
        }
    })();
    cleanup_stage_dir(&stage);
    result.and_then(|path| {
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "created archive has no filename",
            )
        })?;
        Ok(display_destination.join(name))
    })
}

pub(super) fn zip_compression_options(options: &ZipCreateOptions) -> SimpleFileOptions {
    let method = match options.compression_method {
        ZipCompressionMethod::Store => CompressionMethod::Stored,
        ZipCompressionMethod::Deflate => CompressionMethod::Deflated,
    };
    let level = match (options.compression_method, options.compression_level) {
        (ZipCompressionMethod::Store, _) => None,
        (ZipCompressionMethod::Deflate, ZipCompressionLevel::Fast) => Some(1),
        (ZipCompressionMethod::Deflate, ZipCompressionLevel::Standard) => Some(6),
        (ZipCompressionMethod::Deflate, ZipCompressionLevel::Maximum) => Some(9),
    };
    SimpleFileOptions::default()
        .compression_method(method)
        .compression_level(level)
}

pub(super) fn zip_member_name(relative: &Path, is_dir: bool) -> io::Result<String> {
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

pub(super) fn zip_flat_file_name(path: &Path) -> io::Result<String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| valid_component(name))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "filename encoding or component is unsupported",
            )
        })?;
    Ok(name.to_owned())
}

pub(super) fn write_zip_path<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    root: &Path,
    path: &Path,
    member: &str,
    options: SimpleFileOptions,
    preserve_folder_structure: bool,
    total_bytes: &mut u64,
    total_name_bytes: &mut u64,
    member_count: &mut usize,
    files_written: &mut usize,
    seen_files: &mut HashSet<String>,
    cancel: Option<&AtomicBool>,
) -> io::Result<()> {
    check_cancel(cancel)?;
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
        if preserve_folder_structure && !member.is_empty() {
            let directory_name = if member.ends_with('/') {
                member.to_owned()
            } else {
                format!("{member}/")
            };
            writer
                .add_directory(directory_name, options)
                .map_err(zip_writer_error)?;
        }
        for child in fs::read_dir(&canonical_path)? {
            check_cancel(cancel)?;
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
            let child_member = if preserve_folder_structure {
                zip_member_name(child_relative, child_metadata.is_dir())?
            } else if child_metadata.is_dir() {
                String::new()
            } else {
                zip_flat_file_name(&canonical_child)?
            };
            write_zip_path(
                writer,
                root,
                &canonical_child,
                &child_member,
                options,
                preserve_folder_structure,
                total_bytes,
                total_name_bytes,
                member_count,
                files_written,
                seen_files,
                cancel,
            )?;
        }
    } else if metadata.is_file() {
        if member.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file has no archive name",
            ));
        }
        if !seen_files.insert(member.to_lowercase()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive would contain duplicate file names",
            ));
        }
        let file_options = zip_file_options(options, metadata.len());
        writer
            .start_file(member.trim_end_matches('/'), file_options)
            .map_err(zip_writer_error)?;
        let mut source = fs::File::open(&canonical_path)?;
        copy_with_limit_cancellable(
            &mut source,
            writer,
            total_bytes,
            "selected content exceeds the ZIP size limit",
            cancel,
        )?;
        *files_written = files_written.saturating_add(1);
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "selected item is not a regular file or folder",
        ));
    }
    Ok(())
}

pub(super) fn zip_file_options(options: SimpleFileOptions, source_size: u64) -> SimpleFileOptions {
    // Включаем ZIP64 заранее у границы ZIP32, чтобы учесть возможный рост Deflate.
    options
        .large_file(source_size >= ZIP64_FILE_SIZE_THRESHOLD.saturating_sub(ZIP64_FILE_SIZE_MARGIN))
}

pub(super) fn zip_writer_error(error: zip::result::ZipError) -> io::Error {
    match error {
        zip::result::ZipError::Io(error) => error,
        error => io::Error::new(io::ErrorKind::InvalidData, error.to_string()),
    }
}

pub(super) fn move_zip_to_unique_name(
    staged_archive: &Path,
    parent: &Path,
    stem: &str,
    cancel: Option<&AtomicBool>,
) -> io::Result<PathBuf> {
    move_zip_to_unique_name_with(staged_archive, parent, stem, cancel, move_file_no_replace)
}

pub(super) fn move_zip_to_unique_name_with(
    staged_archive: &Path,
    parent: &Path,
    stem: &str,
    cancel: Option<&AtomicBool>,
    mut publish: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    for suffix in 1..=MAX_ARCHIVE_NAME_ATTEMPTS {
        let filename = if suffix == 1 {
            format!("{stem}.zip")
        } else {
            format!("{stem} ({suffix}).zip")
        };
        let destination = parent.join(filename);
        match publish_staged_path(staged_archive, &destination, cancel, |source, target| {
            publish(source, target)
        }) {
            Ok(path) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not choose an unused archive name",
    ))
}
