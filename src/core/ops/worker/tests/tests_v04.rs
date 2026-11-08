//! Проверки операций: tests_v04.

use super::*;
use std::sync::mpsc::channel;
use std::time::Duration;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fm_v04_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn worker<'a>(rrx: &'a Receiver<OpReply>, etx: &'a Sender<OpEvent>) -> Worker<'a> {
    Worker {
        reply_rx: rrx,
        events: etx,
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    }
}

fn job(kind: OpKind) -> OpJob {
    OpJob {
        id: 1,
        kind,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn queue_runs_jobs_in_order() {
    let dir = tmp("queue");
    fs::write(dir.join("a"), b"x").unwrap();
    let (jtx, jrx) = channel();
    let (etx, erx) = channel::<OpEvent>();
    let (_rtx, rrx) = channel();
    spawn(jrx, rrx, etx);
    for (i, name) in ["o1", "o2"].iter().enumerate() {
        jtx.send(OpJob {
            id: (i + 1) as u64,
            kind: OpKind::Copy {
                sources: vec![dir.join("a")],
                dest: dir.join(name),
            },
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
    }
    let mut done = vec![];
    for _ in 0..20 {
        match erx.recv_timeout(Duration::from_secs(10)).unwrap() {
            OpEvent::Done { id, .. } => done.push(id),
            _ => {}
        }
        if done.len() == 2 {
            break;
        }
    }
    assert_eq!(done, vec![1, 2]);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn progress_event_emitted() {
    let dir = tmp("prog");
    fs::write(dir.join("big.bin"), vec![7u8; 200_000]).unwrap();
    let (_rtx, rrx) = channel();
    let (etx, erx) = channel();
    let mut w = worker(&rrx, &etx);
    w.run(&job(OpKind::Copy {
        sources: vec![dir.join("big.bin")],
        dest: dir.join("out"),
    }));
    let mut saw = false;
    while let Ok(ev) = erx.try_recv() {
        if let OpEvent::Progress { done_bytes, .. } = ev {
            saw = saw || done_bytes > 0;
        }
    }
    assert!(saw);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancel_stops_job() {
    let dir = tmp("cancel");
    fs::create_dir(dir.join("src")).unwrap();
    for i in 0..10 {
        fs::write(dir.join("src").join(format!("f{i}")), b"xx").unwrap();
    }
    let (_rtx, rrx) = channel();
    let (etx, _erx) = channel();
    let mut w = worker(&rrx, &etx);
    let j = OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![dir.join("src")],
            dest: dir.join("out"),
        },
        cancel: Arc::new(AtomicBool::new(true)),
    };
    w.run(&j);
    let copied = dir.join("out").join("src");
    let n = fs::read_dir(&copied).map(|r| r.count()).unwrap_or(0);
    assert!(n < 10, "cancelled job copied {n}/10 files");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn conflict_overwrite_replaces() {
    let dir = tmp("ow");
    fs::write(dir.join("a"), b"new").unwrap();
    let dst = dir.join("out");
    fs::create_dir(&dst).unwrap();
    fs::write(dst.join("a"), b"old").unwrap();
    let (rtx, rrx) = channel();
    rtx.send(OpReply::Conflict {
        id: 1,
        action: ConflictAction::Overwrite,
    })
    .unwrap();
    let (etx, _e) = channel();
    worker(&rrx, &etx).run(&job(OpKind::Copy {
        sources: vec![dir.join("a")],
        dest: dst.clone(),
    }));
    assert_eq!(fs::read_to_string(dst.join("a")).unwrap(), "new");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn conflict_skip_keeps() {
    let dir = tmp("sk");
    fs::write(dir.join("a"), b"new").unwrap();
    let dst = dir.join("out");
    fs::create_dir(&dst).unwrap();
    fs::write(dst.join("a"), b"old").unwrap();
    let (rtx, rrx) = channel();
    rtx.send(OpReply::Conflict {
        id: 1,
        action: ConflictAction::Skip,
    })
    .unwrap();
    let (etx, _e) = channel();
    worker(&rrx, &etx).run(&job(OpKind::Copy {
        sources: vec![dir.join("a")],
        dest: dst.clone(),
    }));
    assert_eq!(fs::read_to_string(dst.join("a")).unwrap(), "old");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn conflict_cancel_fails_job() {
    let dir = tmp("cc");
    fs::write(dir.join("a"), b"x").unwrap();
    let dst = dir.join("out");
    fs::create_dir(&dst).unwrap();
    fs::write(dst.join("a"), b"y").unwrap();
    let (rtx, rrx) = channel();
    rtx.send(OpReply::Conflict {
        id: 1,
        action: ConflictAction::Cancel,
    })
    .unwrap();
    let (etx, _erx) = channel();
    worker(&rrx, &etx).run(&job(OpKind::Copy {
        sources: vec![dir.join("a")],
        dest: dst.clone(),
    }));
    assert_eq!(fs::read_to_string(dst.join("a")).unwrap(), "y");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn error_skip_counts_failed() {
    let dir = tmp("eskip");
    let (rtx, rrx) = channel();
    let (etx, erx) = channel();
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
    let mut w = worker(&rrx, &etx);
    w.run(&job(OpKind::Copy {
        sources: vec![dir.join("missing")],
        dest: dir.join("out"),
    }));
    assert!(w.failed >= 1);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn sharing_violation_is_retryable() {
    assert!(is_retryable(&io::Error::from_raw_os_error(32)));
}

#[test]
fn move_copies_then_deletes_source() {
    let dir = tmp("mvdel");
    fs::create_dir(dir.join("src")).unwrap();
    fs::write(dir.join("src").join("n.bin"), vec![1u8; 100]).unwrap();
    let (_rtx, rrx) = channel();
    let (etx, _e) = channel();
    worker(&rrx, &etx).run(&job(OpKind::Move {
        sources: vec![dir.join("src")],
        dest: dir.join("out"),
    }));
    assert!(!dir.join("src").exists());
    assert_eq!(
        fs::metadata(dir.join("out").join("src").join("n.bin"))
            .unwrap()
            .len(),
        100
    );
    fs::remove_dir_all(&dir).ok();
}
