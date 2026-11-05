use std::path::Path;

/// Initializes daily rolling file logging into `<log_dir>/filemanager.log`.
///
/// The returned guard must be kept alive for the duration of the process,
/// otherwise buffered log records may be lost on exit.
pub fn init(log_dir: &Path) -> tracing_appender::non_blocking::WorkerGuard {
    let file_appender = tracing_appender::rolling::daily(log_dir, "filemanager.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .ok();

    guard
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_file_created() {
        let dir = std::env::temp_dir().join(format!("fm_log_{}", std::process::id()));
        let _g = init(&dir);
        tracing::info!("test log entry");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(!files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("fm_log2_{}", std::process::id()));
        let _g1 = init(&dir);
        let _g2 = init(&dir); // second call must not panic
    }
}
