//! Проверки операций: tests_operation_decisions.

use super::*;
use std::sync::mpsc::channel;

fn temp_dir() -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("fm-op-decisions-{n}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn worker<'a>(reply_rx: &'a Receiver<OpReply>, events: &'a Sender<OpEvent>) -> Worker<'a> {
    Worker {
        reply_rx,
        events,
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    }
}

#[test]
fn overwrite_all_replaces_all_collisions_after_one_confirmation() {
    let dir = temp_dir();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let dest = dir.join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&a, b"new-a").unwrap();
    fs::write(&b, b"new-b").unwrap();
    fs::write(dest.join("a.txt"), b"old-a").unwrap();
    fs::write(dest.join("b.txt"), b"old-b").unwrap();

    let (reply_tx, reply_rx) = channel();
    reply_tx
        .send(OpReply::Conflict {
            id: 1,
            action: ConflictAction::OverwriteAll,
        })
        .unwrap();
    let (event_tx, event_rx) = channel();
    worker(&reply_rx, &event_tx).run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![a, b],
            dest: dest.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"new-a");
    assert_eq!(fs::read(dest.join("b.txt")).unwrap(), b"new-b");
    assert_eq!(
        event_rx
            .try_iter()
            .filter(|e| matches!(e, OpEvent::AskConflict { .. }))
            .count(),
        1
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unrelated_conflict_reply_cannot_change_the_copy_decision() {
    let dir = temp_dir();
    let src = dir.join("source.txt");
    let dest = dir.join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&src, b"new").unwrap();
    fs::write(dest.join("source.txt"), b"old").unwrap();
    let (reply_tx, reply_rx) = channel();
    reply_tx
        .send(OpReply::Conflict {
            id: 999,
            action: ConflictAction::Skip,
        })
        .unwrap();
    reply_tx
        .send(OpReply::Conflict {
            id: 1,
            action: ConflictAction::Overwrite,
        })
        .unwrap();
    let (event_tx, _event_rx) = channel();

    worker(&reply_rx, &event_tx).run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![src],
            dest: dest.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert_eq!(fs::read(dest.join("source.txt")).unwrap(), b"new");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn skip_all_preserves_every_colliding_destination() {
    let dir = temp_dir();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let dest = dir.join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&a, b"new-a").unwrap();
    fs::write(&b, b"new-b").unwrap();
    fs::write(dest.join("a.txt"), b"old-a").unwrap();
    fs::write(dest.join("b.txt"), b"old-b").unwrap();

    let (reply_tx, reply_rx) = channel();
    reply_tx
        .send(OpReply::Conflict {
            id: 1,
            action: ConflictAction::SkipAll,
        })
        .unwrap();
    let (event_tx, event_rx) = channel();
    worker(&reply_rx, &event_tx).run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![a, b],
            dest: dest.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"old-a");
    assert_eq!(fs::read(dest.join("b.txt")).unwrap(), b"old-b");
    assert_eq!(
        event_rx
            .try_iter()
            .filter(|e| matches!(e, OpEvent::AskConflict { .. }))
            .count(),
        1
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retry_after_source_becomes_available_completes_copy() {
    let dir = temp_dir();
    let src = dir.join("arrives-after-retry.txt");
    let dest = dir.join("dest");
    fs::create_dir(&dest).unwrap();
    let (reply_tx, reply_rx) = channel();
    let (event_tx, event_rx) = channel();
    let source_for_responder = src.clone();
    let responder = std::thread::spawn(move || {
        while let Ok(event) = event_rx.recv() {
            if let OpEvent::AskError { id, .. } = event {
                fs::write(&source_for_responder, b"arrived").unwrap();
                reply_tx
                    .send(OpReply::Error {
                        id,
                        action: ErrorAction::Retry,
                    })
                    .unwrap();
                break;
            }
        }
    });

    worker(&reply_rx, &event_tx).run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![src.clone()],
            dest: dest.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });
    drop(event_tx);
    responder.join().unwrap();
    assert_eq!(
        fs::read(dest.join("arrives-after-retry.txt")).unwrap(),
        b"arrived"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pre_cancelled_delete_leaves_every_source_untouched() {
    let dir = temp_dir();
    let first = dir.join("first.txt");
    let second = dir.join("second.txt");
    fs::write(&first, b"1").unwrap();
    fs::write(&second, b"2").unwrap();
    let (_reply_tx, reply_rx) = channel();
    let (event_tx, _event_rx) = channel();
    worker(&reply_rx, &event_tx).run(&OpJob {
        id: 1,
        kind: OpKind::Delete {
            sources: vec![first.clone(), second.clone()],
        },
        cancel: Arc::new(AtomicBool::new(true)),
    });

    assert_eq!(fs::read(&first).unwrap(), b"1");
    assert_eq!(fs::read(&second).unwrap(), b"2");
    fs::remove_dir_all(dir).unwrap();
}
