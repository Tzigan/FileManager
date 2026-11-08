//! Проверки операций: tests_rename_safety.

use super::*;
use std::sync::mpsc::channel;

fn temp_dir() -> PathBuf {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("fm-rename-safety-{id}"));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn rename_rejects_destination_outside_source_folder() {
    let root = temp_dir();
    let source_dir = root.join("source");
    let other_dir = root.join("other");
    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(&other_dir).unwrap();
    let source = source_dir.join("old.txt");
    let destination = other_dir.join("existing.txt");
    fs::write(&source, b"source").unwrap();
    fs::write(&destination, b"destination").unwrap();

    let (reply_tx, reply_rx) = channel();
    reply_tx
        .send(OpReply::Error {
            id: 7,
            action: ErrorAction::Skip,
        })
        .unwrap();
    let (event_tx, event_rx) = channel();
    let mut worker = Worker {
        reply_rx: &reply_rx,
        events: &event_tx,
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };
    worker.run(&OpJob {
        id: 7,
        kind: OpKind::Rename {
            from: source.clone(),
            to: destination.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    assert_eq!(worker.failed, 1);
    assert_eq!(fs::read(&source).unwrap(), b"source");
    assert_eq!(fs::read(&destination).unwrap(), b"destination");
    assert!(
        event_rx
            .try_iter()
            .any(|event| matches!(event, OpEvent::AskError { id: 7, .. }))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_overwrite_rename_keeps_existing_destination() {
    let dir = temp_dir();
    let missing_source = dir.join("missing.txt");
    let destination = dir.join("existing.txt");
    fs::write(&destination, b"destination").unwrap();

    let (reply_tx, reply_rx) = channel();
    reply_tx
        .send(OpReply::Conflict {
            id: 9,
            action: ConflictAction::Overwrite,
        })
        .unwrap();
    reply_tx
        .send(OpReply::Error {
            id: 9,
            action: ErrorAction::Skip,
        })
        .unwrap();
    let (event_tx, _event_rx) = channel();
    let mut worker = Worker {
        reply_rx: &reply_rx,
        events: &event_tx,
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };
    worker.run(&OpJob {
        id: 9,
        kind: OpKind::Rename {
            from: missing_source,
            to: destination.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    assert_eq!(worker.failed, 1);
    assert_eq!(fs::read(&destination).unwrap(), b"destination");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn successful_overwrite_rename_replaces_destination() {
    let dir = temp_dir();
    let source = dir.join("source.txt");
    let destination = dir.join("destination.txt");
    fs::write(&source, b"source").unwrap();
    fs::write(&destination, b"old destination").unwrap();

    rename_with_overwrite(&source, &destination, true).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read(&destination).unwrap(), b"source");
    fs::remove_dir_all(dir).unwrap();
}
