//! Проверки операций: tests_samefolder.

use super::*;
use std::sync::mpsc::channel;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fm_sf_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn same_folder_paste_makes_copy_name() {
    let dir = tmp("cv");
    fs::write(dir.join("a.txt"), b"data").unwrap();
    let (_rtx, rrx) = channel();
    let (etx, _e) = channel();
    let mut w = Worker {
        reply_rx: &rrx,
        events: &etx,
        platform: test_ops_platform(),
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };
    w.run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![dir.join("a.txt")],
            dest: dir.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let copy = dir.join("a — копия.txt");
    assert_eq!(fs::read_to_string(&copy).unwrap(), "data");
    // Second paste → "a — копия (2).txt"
    w.run(&OpJob {
        id: 2,
        kind: OpKind::Copy {
            sources: vec![dir.join("a.txt")],
            dest: dir.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert!(dir.join("a — копия (2).txt").exists());
    assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "data");
    fs::remove_dir_all(&dir).ok();
}

#[cfg(windows)]
#[test]
fn same_folder_copy_detects_case_only_directory_alias() {
    let dir = tmp("case-alias");
    let src = dir.join("Source.txt");
    fs::write(&src, b"original").unwrap();
    let dest = PathBuf::from(dir.to_string_lossy().to_ascii_lowercase());
    let destination_entry = dest.join("Source.txt");
    let copy = copy_variant(&destination_entry);
    assert_ne!(src, destination_entry);
    assert!(same_existing_path(&src, &destination_entry));

    let (reply_tx, reply_rx) = channel();
    reply_tx
        .send(OpReply::Conflict {
            id: 1,
            action: ConflictAction::Cancel,
        })
        .unwrap();
    let (event_tx, _event_rx) = channel();
    let mut worker = Worker {
        reply_rx: &reply_rx,
        events: &event_tx,
        platform: test_ops_platform(),
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };
    worker.run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![src.clone()],
            dest,
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    assert_eq!(fs::read(&src).unwrap(), b"original");
    assert_eq!(fs::read(copy).unwrap(), b"original");
    fs::remove_dir_all(&dir).ok();
}
