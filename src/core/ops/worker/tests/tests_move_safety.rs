//! Проверки операций: tests_move_safety.

use super::*;
use std::sync::mpsc::channel;

fn temp_dir() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("fm-move-safety-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(windows)]
fn place_test_junction(link: &Path, target: &Path) -> bool {
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return true;
    }
    std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            &link.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn alias_test_paths() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let root = temp_dir();
    let source = root.join("source");
    let alias = root.join("alias");
    let destination = alias.join("child");
    fs::create_dir_all(source.join("child")).unwrap();
    assert!(place_test_junction(&alias, &source));
    (root, source, alias, destination)
}

#[cfg(windows)]
#[test]
fn alias_target_rejected_by_worker_reports_understandable_error_not_silent_skip() {
    let (root, source, alias, destination) = alias_test_paths();
    let (reply_tx, reply_rx) = channel();
    reply_tx
        .send(OpReply::Error {
            id: 41,
            action: ErrorAction::Skip,
        })
        .unwrap();
    let (event_tx, event_rx) = channel();
    let mut worker = Worker {
        reply_rx: &reply_rx,
        events: &event_tx,
        platform: test_ops_platform(),
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };

    worker.run(&OpJob {
        id: 41,
        kind: OpKind::Move {
            sources: vec![source.clone()],
            dest: destination.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    match event_rx.try_recv().unwrap() {
        OpEvent::AskError {
            path,
            message,
            kind,
            ..
        } => {
            assert_eq!(path, source);
            assert_eq!(kind, std::io::ErrorKind::InvalidInput);
            assert!(message.contains("source folder"));
            assert!(include_str!("../../../../ui/opsui.rs").contains(
                "ErrorKind::InvalidInput | ErrorKind::InvalidData => \"error.operation.invalid\""
            ));
            assert!(
                include_str!("../../../../ui/opsui.rs")
                    .contains("self.ops_ui.error_q = Some((id, path, kind, retryable))")
            );
            assert!(include_str!("../../../../ui/opsui.rs").contains("fn error_dialog"));
            assert!(
                include_str!("../../../../ui/opsui.rs")
                    .contains("operation_error_keys_are_localized_in_both_languages")
            );
            assert!(
                include_str!("../../../../../i18n/ru.json").contains("\"error.operation.invalid\"")
            );
            assert!(
                include_str!("../../../../../i18n/en.json").contains("\"error.operation.invalid\"")
            );
        }
        _ => panic!("alias rejection must ask through the operation error layer"),
    }
    assert_eq!(worker.failed, 1);
    assert!(!destination.join("source").exists());
    drop(worker);
    drop(event_tx);
    fs::remove_dir(alias).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn alias_rejection_counts_as_failed_not_false_success() {
    let (root, source, alias, destination) = alias_test_paths();
    let good = root.join("good.txt");
    fs::write(&good, b"keep processing other sources").unwrap();
    let (reply_tx, reply_rx) = channel();
    reply_tx
        .send(OpReply::Error {
            id: 42,
            action: ErrorAction::Skip,
        })
        .unwrap();
    let (event_tx, event_rx) = channel();
    let mut worker = Worker {
        reply_rx: &reply_rx,
        events: &event_tx,
        platform: test_ops_platform(),
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };

    worker.run(&OpJob {
        id: 42,
        kind: OpKind::Copy {
            sources: vec![source, good],
            dest: destination.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    assert!(matches!(
        event_rx.try_recv().unwrap(),
        OpEvent::AskError {
            kind: std::io::ErrorKind::InvalidInput,
            ..
        }
    ));
    assert_eq!(worker.failed, 1);
    assert_eq!(
        fs::read(destination.join("good.txt")).unwrap(),
        b"keep processing other sources"
    );
    drop(worker);
    drop(event_tx);
    fs::remove_dir(alias).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn partial_or_cancelled_copy_never_removes_move_source() {
    let dir = temp_dir();
    let src = dir.join("source.bin");
    let dst = dir.join("partial.bin");
    fs::write(&src, b"original data").unwrap();
    fs::write(&dst, b"partial").unwrap();

    assert!(!remove_move_source_after_copy(&src, &dst, false, false).unwrap());
    assert!(src.exists());
    assert_eq!(fs::read(&src).unwrap(), b"original data");
    assert!(!remove_move_source_after_copy(&src, &dst, true, true).unwrap());
    assert!(src.exists());

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn complete_copy_removes_source_only_if_destination_exists() {
    let dir = temp_dir();
    let src = dir.join("source.bin");
    let dst = dir.join("destination.bin");
    fs::write(&src, b"verified").unwrap();

    assert!(!remove_move_source_after_copy(&src, &dst, true, false).unwrap());
    assert!(src.exists());

    fs::write(&dst, b"verified").unwrap();
    assert!(remove_move_source_after_copy(&src, &dst, true, false).unwrap());
    assert!(!src.exists());
    assert_eq!(fs::read(&dst).unwrap(), b"verified");

    fs::remove_dir_all(dir).unwrap();
}

#[cfg(windows)]
#[test]
fn failed_locked_file_move_keeps_original_source() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::mpsc::channel;

    let dir = temp_dir();
    let src = dir.join("locked.bin");
    let dest = dir.join("destination");
    fs::create_dir(&dest).unwrap();
    fs::write(&src, b"source must survive a failed move").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&src)
        .unwrap();

    let (reply_tx, reply_rx) = channel();
    let (event_tx, event_rx) = channel();
    let responder = std::thread::spawn(move || {
        while let Ok(event) = event_rx.recv() {
            if let OpEvent::AskError { id, .. } = event {
                let _ = reply_tx.send(OpReply::Error {
                    id,
                    action: ErrorAction::Skip,
                });
            }
        }
    });
    let mut worker = Worker {
        reply_rx: &reply_rx,
        events: &event_tx,
        platform: test_ops_platform(),
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };
    worker.run(&OpJob {
        id: 17,
        kind: OpKind::Move {
            sources: vec![src.clone()],
            dest: dest.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert!(src.exists());
    assert!(!dest.join("locked.bin").exists());
    assert_eq!(worker.failed, 1);

    drop(worker);
    drop(event_tx);
    drop(lock);
    responder.join().unwrap();
    assert_eq!(
        fs::read(&src).unwrap(),
        b"source must survive a failed move"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(windows)]
#[test]
fn incomplete_recursive_move_keeps_the_entire_source_tree() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = temp_dir();
    let src_dir = dir.join("source-tree");
    let dest = dir.join("destination");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir(&dest).unwrap();
    let locked = src_dir.join("locked.txt");
    let good = src_dir.join("good.txt");
    fs::write(&locked, b"locked source data").unwrap();
    fs::write(&good, b"good source data").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&locked)
        .unwrap();

    let (reply_tx, reply_rx) = channel();
    let (event_tx, event_rx) = channel();
    let responder = std::thread::spawn(move || {
        while let Ok(event) = event_rx.recv() {
            if let OpEvent::AskError { id, .. } = event {
                let _ = reply_tx.send(OpReply::Error {
                    id,
                    action: ErrorAction::Skip,
                });
            }
        }
    });
    let mut w = Worker {
        reply_rx: &reply_rx,
        events: &event_tx,
        platform: test_ops_platform(),
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };
    w.run(&OpJob {
        id: 18,
        kind: OpKind::Move {
            sources: vec![src_dir.clone()],
            dest: dest.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    assert!(src_dir.is_dir());
    assert!(locked.exists() && good.exists());
    assert_eq!(w.failed, 1);
    drop(w);
    drop(event_tx);
    drop(lock);
    responder.join().unwrap();
    assert_eq!(fs::read(&locked).unwrap(), b"locked source data");
    assert_eq!(fs::read(&good).unwrap(), b"good source data");
    fs::remove_dir_all(dir).unwrap();
}
