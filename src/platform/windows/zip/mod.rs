//! ZIP: чтение, извлечение и создание разнесены по модулям, публичный API прежний.

mod common;
mod create;
mod extract;
mod read;

pub use create::create_zip_archive;
pub use create::create_zip_archive_cancellable;
pub use extract::{extract_zip_all, extract_zip_file, extract_zip_folder};
pub use extract::{
    extract_zip_all_cancellable, extract_zip_file_cancellable, extract_zip_folder_cancellable,
};
pub use read::{read_zip_dir_cancellable, search_zip};

#[cfg(test)]
mod tests {
    use super::common::*;
    use super::create::*;
    use super::read::read_zip_dir;
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::{self, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::platform::{
        ZipCompressionLevel, ZipCompressionMethod, ZipCreateOptions, ZipExistingBehavior,
    };
    use zip::CompressionMethod;
    use zip::write::{SimpleFileOptions, ZipWriter};

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
    fn cancelled_zip_work_does_not_leave_a_final_output() {
        let root = temp_dir();
        let source = root.join("source.txt");
        fs::write(&source, b"payload").unwrap();
        let cancelled = AtomicBool::new(true);

        assert_eq!(
            create_zip_archive_cancellable(
                std::slice::from_ref(&source),
                &create_options(&root, "cancelled"),
                &cancelled,
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::Interrupted
        );
        assert!(!root.join("cancelled.zip").exists());

        let archive =
            create_zip_archive(std::slice::from_ref(&source), &create_options(&root, "archive"))
                .unwrap();
        let destination = root.join("out");
        fs::create_dir(&destination).unwrap();
        assert_eq!(
            extract_zip_all_cancellable(&archive, &destination, &cancelled)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(
            extract_zip_file_cancellable(&archive, "source.txt", &destination, &cancelled)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        assert!(!destination.join("archive").exists());
        assert!(!destination.join("source.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn create_options(root: &Path, name: &str) -> ZipCreateOptions {
        ZipCreateOptions {
            source_root: root.to_path_buf(),
            destination: root.to_path_buf(),
            archive_name: name.to_owned(),
            compression_method: ZipCompressionMethod::Deflate,
            compression_level: ZipCompressionLevel::Standard,
            preserve_folder_structure: true,
            existing_behavior: ZipExistingBehavior::AddSuffix,
        }
    }

    #[test]
    fn compression_presets_map_to_supported_deflate_levels() {
        let mut options = create_options(Path::new("."), "test");
        assert_eq!(
            zip_compression_options(&options).get_compression_level(),
            Some(6)
        );
        options.compression_level = ZipCompressionLevel::Fast;
        assert_eq!(
            zip_compression_options(&options).get_compression_level(),
            Some(1)
        );
        options.compression_level = ZipCompressionLevel::Maximum;
        assert_eq!(
            zip_compression_options(&options).get_compression_level(),
            Some(9)
        );
        options.compression_method = ZipCompressionMethod::Store;
        assert_eq!(
            zip_compression_options(&options).get_compression_level(),
            None
        );
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

        let file_archive = create_zip_archive(
            std::slice::from_ref(&file),
            &create_options(&root, "readme"),
        )
        .unwrap();
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

        let folder_archive = create_zip_archive(
            std::slice::from_ref(&folder),
            &create_options(&root, "folder"),
        )
        .unwrap();
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
        let archive = create_zip_archive(&sources, &create_options(&root, "archive")).unwrap();
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

        let second_archive =
            create_zip_archive(&sources, &create_options(&root, "archive")).unwrap();
        assert_eq!(
            second_archive.file_name().and_then(|name| name.to_str()),
            Some("archive (2).zip")
        );
        assert!(read_zip_dir(&archive, "").is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zip_options_use_custom_destination_method_and_conflict_behavior() {
        let root = temp_dir();
        let source_root = root.join("source");
        let destination = root.join("archives");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let file = source_root.join("payload.txt");
        fs::write(&file, b"first content").unwrap();

        let mut options = create_options(&source_root, "custom-name");
        options.destination = destination.clone();
        options.compression_method = ZipCompressionMethod::Store;
        let archive = create_zip_archive(std::slice::from_ref(&file), &options).unwrap();
        assert_eq!(
            archive.file_name().and_then(|name| name.to_str()),
            Some("custom-name.zip")
        );
        assert_eq!(archive.parent().unwrap(), destination.as_path());
        {
            let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
            assert_eq!(
                zip.by_name("payload.txt").unwrap().compression(),
                CompressionMethod::Stored
            );
        }

        fs::write(&file, b"replacement content").unwrap();
        options.existing_behavior = ZipExistingBehavior::Replace;
        assert_eq!(
            create_zip_archive(std::slice::from_ref(&file), &options).unwrap(),
            archive
        );
        {
            let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
            let mut content = Vec::new();
            zip.by_name("payload.txt")
                .unwrap()
                .read_to_end(&mut content)
                .unwrap();
            assert_eq!(content, b"replacement content");
        }

        options.existing_behavior = ZipExistingBehavior::Ask;
        assert_eq!(
            create_zip_archive(std::slice::from_ref(&file), &options)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flattening_keeps_only_files_and_rejects_duplicate_names() {
        let root = temp_dir();
        let source_root = root.join("source");
        let destination = root.join("archives");
        let first = source_root.join("first").join("nested");
        let second = source_root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let first_file = first.join("same.txt");
        let second_file = second.join("same.txt");
        fs::write(&first_file, b"first").unwrap();
        fs::write(&second_file, b"second").unwrap();

        let mut options = create_options(&source_root, "flat");
        options.destination = destination.clone();
        options.preserve_folder_structure = false;
        let error = create_zip_archive(
            &[source_root.join("first"), source_root.join("second")],
            &options,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!destination.join("flat.zip").exists());

        options.archive_name = "flattened".to_owned();
        let archive = create_zip_archive(std::slice::from_ref(&first_file), &options).unwrap();
        let entries = read_zip_dir(&archive, "").unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "same.txt" && !entry.is_dir)
        );
        assert!(!entries.iter().any(|entry| entry.name.contains('/')));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_and_extracts_zip64_entries() {
        let root = temp_dir();
        let archive_path = root.join("zip64.zip");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&archive_path)
            .unwrap();
        let mut writer = ZipWriter::new(file);
        writer.set_raw_zip64_extensible_data_sector(Vec::new().into_boxed_slice());
        let options = zip_file_options(
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            ZIP64_FILE_SIZE_THRESHOLD,
        );
        writer.start_file("zip64/readme.txt", options).unwrap();
        writer.write_all(b"ZIP64 content").unwrap();
        writer.finish().unwrap().sync_all().unwrap();

        assert!(
            read_zip_dir(&archive_path, "zip64")
                .unwrap()
                .iter()
                .any(|entry| entry.name == "readme.txt" && !entry.is_dir)
        );
        let output = root.join("extracted");
        let extracted = extract_zip_file(&archive_path, "zip64/readme.txt", &output).unwrap();
        assert_eq!(fs::read(extracted).unwrap(), b"ZIP64 content");

        let folder_output = root.join("folder-output");
        let extracted_folder = extract_zip_folder(&archive_path, "zip64", &folder_output).unwrap();
        assert_eq!(
            fs::read(extracted_folder.join("readme.txt")).unwrap(),
            b"ZIP64 content"
        );

        let all_output = root.join("all-output");
        let extracted_all = extract_zip_all(&archive_path, &all_output).unwrap();
        assert_eq!(
            fs::read(extracted_all.join("zip64/readme.txt")).unwrap(),
            b"ZIP64 content"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
