//! Проверки операций: tests.

use super::*;
use std::sync::atomic::AtomicBool;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fm_ops_test_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn job() -> OpJob {
    OpJob {
        id: 1,
        kind: OpKind::CreateDir {
            path: PathBuf::new(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn copy_streams_content_and_preserves_size() {
    let dir = tmpdir("copy");
    let src = dir.join("a.bin");
    let dst = dir.join("b.bin");
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(&src, &data).unwrap();
    let j = job();
    copy_file_stream(&src, &dst, &mut |_| {}, test_ops_platform(), &j).unwrap();
    assert_eq!(fs::read(&dst).unwrap(), data);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancel_removes_partial_file() {
    let dir = tmpdir("cancel");
    let src = dir.join("big.bin");
    let dst = dir.join("part.bin");
    fs::write(&src, vec![7u8; 1_000_000]).unwrap();
    let j = job();
    j.cancel.store(true, Ordering::Relaxed);
    copy_file_stream(&src, &dst, &mut |_| {}, test_ops_platform(), &j).unwrap();
    assert!(!dst.exists());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancellation_after_a_written_chunk_removes_partial_destination() {
    let dir = tmpdir("mid-copy-cancel");
    let src = dir.join("large.bin");
    let dst = dir.join("partial.bin");
    fs::write(&src, vec![0x5a; 4 * 1024 * 1024]).unwrap();
    let job = job();
    let mut copied_bytes = 0;
    copy_file_stream(
        &src,
        &dst,
        &mut |n| {
            copied_bytes += n;
            job.cancel.store(true, Ordering::Relaxed);
        },
        test_ops_platform(),
        &job,
    )
    .unwrap();

    assert!(copied_bytes > 0);
    assert!(!dst.exists());
    assert_eq!(fs::metadata(&src).unwrap().len(), 4 * 1024 * 1024);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancelling_overwrite_preserves_existing_destination() {
    let dir = tmpdir("overwrite-cancel");
    let src = dir.join("new.bin");
    let dst = dir.join("existing.bin");
    fs::write(&src, vec![0x5a; 128 * 1024]).unwrap();
    fs::write(&dst, b"original destination").unwrap();
    let job = job();
    let cancel = job.cancel.clone();

    copy_file_stream(
        &src,
        &dst,
        &mut |_| cancel.store(true, Ordering::Relaxed),
        test_ops_platform(),
        &job,
    )
    .unwrap();

    assert_eq!(fs::read(&dst).unwrap(), b"original destination");
    assert!(fs::read_dir(&dir).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".filemanager-copy-")
    }));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn scan_counts_files_and_bytes() {
    let dir = tmpdir("scan");
    fs::write(dir.join("a"), b"12345").unwrap();
    fs::create_dir(dir.join("sub")).unwrap();
    fs::write(dir.join("sub").join("b"), b"678").unwrap();
    let (mut b, mut c) = (0, 0);
    let j = job();
    scan(&dir, &mut b, &mut c, &j).unwrap();
    assert_eq!((b, c), (8, 2));
    fs::remove_dir_all(&dir).ok();
}
