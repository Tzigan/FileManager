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
        .init();

    guard
}
