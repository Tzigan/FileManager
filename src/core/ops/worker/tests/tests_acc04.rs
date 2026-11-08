//! Проверки операций: tests_acc04.

use super::*;
use std::sync::mpsc::channel;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fm_a4_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn w<'a>(r: &'a Receiver<OpReply>, e: &'a Sender<OpEvent>) -> Worker<'a> {
    Worker {
        reply_rx: r,
        events: e,
        platform: test_ops_platform(),
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    }
}

#[test]
fn copies_large_tree() {
    let d = tmp("tree");
    for lvl in 0..4 {
        let sub = d.join("src").join(format!("l{lvl}"));
        fs::create_dir_all(&sub).unwrap();
        for i in 0..25 {
            fs::write(sub.join(format!("f{i}.bin")), vec![b'x'; 512]).unwrap();
        }
    }
    let (_r, rrx) = channel();
    let (etx, _e) = channel();
    w(&rrx, &etx).run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![d.join("src")],
            dest: d.join("dst"),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let mut n = 0;
    for e in fs::read_dir(d.join("dst").join("src")).unwrap().flatten() {
        n += fs::read_dir(e.path()).map(|r| r.count()).unwrap_or(0);
    }
    assert_eq!(n, 100);
    fs::remove_dir_all(&d).ok();
}

#[test]
fn recursive_copy_preserves_nested_contents_and_empty_directories() {
    let d = tmp("nested-shape");
    let source = d.join("source");
    let empty = source.join("empty child");
    let nested = source.join("nested").join("deeper");
    fs::create_dir_all(&empty).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("payload.bin"), [0, 1, 2, 255]).unwrap();

    let (_reply_tx, reply_rx) = channel();
    let (event_tx, _event_rx) = channel();
    w(&reply_rx, &event_tx).run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![source.clone()],
            dest: d.join("destination"),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    let copied = d.join("destination").join("source");
    assert!(copied.join("empty child").is_dir());
    assert_eq!(
        fs::read(copied.join("nested/deeper/payload.bin")).unwrap(),
        [0, 1, 2, 255]
    );
    fs::remove_dir_all(&d).ok();
}

#[test]
fn error_event_has_message() {
    let d = tmp("errmsg");
    let (rtx, rrx) = channel();
    let (etx, erx) = channel::<OpEvent>();
    std::thread::spawn(move || {
        while let Ok(ev) = erx.recv() {
            if let OpEvent::AskError { id, message, .. } = ev {
                assert!(!message.is_empty());
                let _ = rtx.send(OpReply::Error {
                    id,
                    action: ErrorAction::Skip,
                });
            }
        }
    });
    w(&rrx, &etx).run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![d.join("gone")],
            dest: d.join("out"),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });
    fs::remove_dir_all(&d).ok();
}

#[test]
fn locked_file_does_not_break_queue() {
    let d = tmp("lock");
    let locked = d.join("locked.txt");
    fs::write(&locked, b"x").unwrap();
    fs::write(d.join("good.txt"), b"ok").unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();
        let (rtx, rrx) = channel();
        let (etx, erx) = channel::<OpEvent>();
        std::thread::spawn(move || {
            while let Ok(ev) = erx.recv() {
                if let OpEvent::AskError { id, .. } = ev {
                    let _ = rtx.send(OpReply::Error {
                        id,
                        action: ErrorAction::Skip,
                    });
                }
            }
        });
        let mut worker = w(&rrx, &etx);
        worker.run(&OpJob {
            id: 1,
            kind: OpKind::Copy {
                sources: vec![locked.clone(), d.join("good.txt")],
                dest: d.join("out"),
            },
            cancel: Arc::new(AtomicBool::new(false)),
        });
        assert_eq!(
            fs::read_to_string(d.join("out").join("good.txt")).unwrap(),
            "ok"
        );
        assert!(worker.failed >= 1);
        drop(hold);
    }
    fs::remove_dir_all(&d).ok();
}
