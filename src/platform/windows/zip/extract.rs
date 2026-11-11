//! Извлечение файлов, папок и всего архива без перезаписи существующих путей.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::common::*;

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

pub(super) fn extract_tree(
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

