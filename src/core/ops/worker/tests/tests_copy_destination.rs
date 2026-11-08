//! Проверки операций: tests_copy_destination.

use super::*;
use std::sync::mpsc::channel;

#[test]
fn refuses_to_copy_directory_into_its_subdirectory() {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("fm-copy-destination-{id}"));
    let source = root.join("source");
    let destination = source.join("child");
    fs::create_dir_all(&destination).unwrap();
    fs::write(source.join("keep.txt"), b"original").unwrap();

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
            sources: vec![source.clone()],
            dest: destination.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });

    assert_eq!(worker.failed, 1);
    assert_eq!(fs::read(source.join("keep.txt")).unwrap(), b"original");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    assert!(matches!(
        event_rx.try_recv().unwrap(),
        OpEvent::AskError { id: 42, .. }
    ));
    fs::remove_dir_all(root).unwrap();
}
