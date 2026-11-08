//! Проверки операций: tests_extra.

use super::*;
use std::sync::mpsc::channel;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fm_ops_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn job_with(kind: OpKind) -> OpJob {
    OpJob {
        id: 9,
        kind,
        cancel: Arc::new(AtomicBool::new(false)),
    }
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

#[test]
fn copy_job_copies_file() {
    let dir = tmpdir("wcopy");
    fs::write(dir.join("a.txt"), b"hi").unwrap();
    let (_rtx, rrx) = channel();
    let (etx, _erx) = channel();
    let mut w = worker(&rrx, &etx);
    w.run(&job_with(OpKind::Copy {
        sources: vec![dir.join("a.txt")],
        dest: dir.join("out"),
    }));
    assert_eq!(
        fs::read_to_string(dir.join("out").join("a.txt")).unwrap(),
        "hi"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn rename_job_renames() {
    let dir = tmpdir("wren");
    fs::write(dir.join("old.txt"), b"x").unwrap();
    let (_rtx, rrx) = channel();
    let (etx, _erx) = channel();
    let mut w = worker(&rrx, &etx);
    w.run(&job_with(OpKind::Rename {
        from: dir.join("old.txt"),
        to: dir.join("new.txt"),
    }));
    assert!(!dir.join("old.txt").exists());
    assert!(dir.join("new.txt").exists());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn create_dir_and_file() {
    let dir = tmpdir("wcreate");
    let (_rtx, rrx) = channel();
    let (etx, _erx) = channel();
    let mut w = worker(&rrx, &etx);
    w.run(&job_with(OpKind::CreateDir {
        path: dir.join("nd"),
    }));
    w.run(&job_with(OpKind::CreateFile {
        path: dir.join("nf.txt"),
    }));
    assert!(dir.join("nd").is_dir() && dir.join("nf.txt").is_file());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn conflict_skipall_leaves_dest() {
    let dir = tmpdir("wskip");
    fs::write(dir.join("a.txt"), b"new").unwrap();
    let dst = dir.join("out");
    fs::create_dir(&dst).unwrap();
    fs::write(dst.join("a.txt"), b"old").unwrap();
    let (rtx, rrx) = channel();
    rtx.send(OpReply::Conflict {
        id: 9,
        action: ConflictAction::SkipAll,
    })
    .unwrap();
    let (etx, _erx) = channel();
    let mut w = worker(&rrx, &etx);
    w.run(&job_with(OpKind::Copy {
        sources: vec![dir.join("a.txt")],
        dest: dst.clone(),
    }));
    assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "old");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn delete_job_removes() {
    let dir = tmpdir("wdel");
    fs::write(dir.join("a.txt"), b"x").unwrap();
    let (_rtx, rrx) = channel();
    let (etx, _erx) = channel();
    let mut w = worker(&rrx, &etx);
    w.run(&job_with(OpKind::Delete {
        sources: vec![dir.join("a.txt")],
    }));
    assert!(!dir.join("a.txt").exists());
    fs::remove_dir_all(&dir).ok();
}
