//! Проверки операций: tests_v02lp.

use super::*;

#[test]
fn long_path_adds_prefix() {
    #[cfg(windows)]
    {
        let long = PathBuf::from("C:\\".to_string() + &"a".repeat(260));
        assert!(long_path(&long).to_string_lossy().starts_with(r"\\?\"));
        let short = PathBuf::from("C:\\x");
        assert_eq!(long_path(&short), short);
    }
}

#[test]
fn copy_into_deep_path() {
    let dir = std::env::temp_dir().join(format!("fm_deep_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let deep = (0..12).fold(dir.clone(), |p, i| {
        p.join(format!("d{i:02}_{}", "x".repeat(15)))
    });
    fs::create_dir_all(&deep).unwrap();
    fs::write(dir.join("f.txt"), b"deep").unwrap();
    let (_r, rrx) = std::sync::mpsc::channel();
    let (etx, _e) = std::sync::mpsc::channel();
    let mut w = Worker {
        reply_rx: &rrx,
        events: &etx,
        overwrite_all: false,
        skip_all: false,
        failed: 0,
    };
    w.run(&OpJob {
        id: 1,
        kind: OpKind::Copy {
            sources: vec![dir.join("f.txt")],
            dest: deep.clone(),
        },
        cancel: Arc::new(AtomicBool::new(false)),
    });
    assert_eq!(fs::read_to_string(deep.join("f.txt")).unwrap(), "deep");
    fs::remove_dir_all(&dir).ok();
}
