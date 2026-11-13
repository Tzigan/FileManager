use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Once;

use crate::core::i18n::Language;
#[cfg(any(not(debug_assertions), test))]
use crate::core::i18n::I18n;

pub const LOG_FILE_PREFIX: &str = "filemanager.log";
/// Одного старого файла достаточно как запаса: дневной rollover удаляет его,
/// пока окно экспорта читает последние четырнадцать файлов.
pub const LOG_RETAINED_FILES: usize = 15;

static PANIC_LOG_HOOK: Once = Once::new();
static PANIC_LANGUAGE: AtomicU8 = AtomicU8::new(0);

fn install_panic_log_hook() {
    PANIC_LOG_HOOK.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!(
                panic = %info,
                backtrace = %std::backtrace::Backtrace::force_capture(),
                "application panic"
            );
            #[cfg(all(windows, not(debug_assertions)))]
            {
                let (title, message) = panic_user_message(panic_language());
                crate::platform::notify_panic(&title, &message);
            }
            default_hook(info);
        }));
    });
}

pub fn set_panic_language(language: Language) {
    PANIC_LANGUAGE.store(
        u8::from(language == Language::En),
        Ordering::Relaxed,
    );
}

#[cfg(all(windows, not(debug_assertions)))]
fn panic_language() -> Language {
    if PANIC_LANGUAGE.load(Ordering::Relaxed) == 1 {
        Language::En
    } else {
        Language::Ru
    }
}

#[cfg(any(not(debug_assertions), test))]
fn panic_user_message(language: Language) -> (String, String) {
    let i18n = I18n::new(language);
    (
        i18n.t("error.panic.title").to_owned(),
        i18n.t("error.panic.message").to_owned(),
    )
}

fn configured_max_level() -> tracing::Level {
    tracing::Level::INFO
}

pub fn log_io_error(
    context: &str,
    operation_id: Option<u64>,
    path: Option<&Path>,
    error: &std::io::Error,
) {
    if let Some(path) = path {
        tracing::info!(
            context = %context,
            operation_id = ?operation_id,
            path = %path.display(),
            error_kind = ?error.kind(),
            raw_os_error = ?error.raw_os_error(),
            error = %error,
            "I/O operation failed"
        );
    } else {
        tracing::info!(
            context = %context,
            operation_id = ?operation_id,
            error_kind = ?error.kind(),
            raw_os_error = ?error.raw_os_error(),
            error = %error,
            "I/O operation failed"
        );
    }
}

/// Initializes daily rolling file logging into `<log_dir>/filemanager.log`.
///
/// The returned guard must be kept alive for the duration of the process,
/// otherwise buffered log records may be lost on exit.
pub fn file_appender(
    log_dir: &Path,
) -> Result<
    tracing_appender::rolling::RollingFileAppender,
    tracing_appender::rolling::InitError,
> {
    tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .max_log_files(LOG_RETAINED_FILES)
        .build(log_dir)
}

pub fn init(
    log_dir: &Path,
) -> Result<tracing_appender::non_blocking::WorkerGuard, tracing_appender::rolling::InitError> {
    let file_appender = file_appender(log_dir)?;
    let (writer, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_max_level(configured_max_level())
        .try_init()
        .ok();

    install_panic_log_hook();
    Ok(guard)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_log_keeps_info_level_diagnostics_enabled() {
        assert_eq!(configured_max_level(), tracing::Level::INFO);
        let (title, message) = panic_user_message(Language::En);
        assert!(!title.is_empty());
        assert!(!message.is_empty());
        assert!(!message.contains("backtrace"));
        assert!(!message.contains("HRESULT"));
    }

    #[test]
    fn release_log_contains_error_kind_path_and_context() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);

        struct BufferWriter(Arc<Mutex<Vec<u8>>>);

        impl Write for BufferWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
            type Writer = BufferWriter;

            fn make_writer(&'a self) -> Self::Writer {
                BufferWriter(Arc::clone(&self.0))
            }
        }

        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(configured_max_level())
            .with_writer(Buffer(Arc::clone(&output)))
            .finish();
        let path = Path::new(r"C:\data\locked.txt");
        let error = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "access denied",
        );
        tracing::subscriber::with_default(subscriber, || {
            log_io_error("copy destination", Some(17), Some(path), &error);
        });
        let output = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(output.contains("INFO"));
        assert!(output.contains("copy destination"));
        assert!(output.contains("Some(17)"));
        assert!(output.contains(&path.display().to_string()));
        assert!(output.contains("PermissionDenied"));
        assert!(output.contains("access denied"));
    }

    #[test]
    fn release_panic_shows_neutral_user_message_not_stacktrace_or_empty() {
        for language in [Language::Ru, Language::En] {
            let (title, message) = panic_user_message(language);
            assert!(!title.trim().is_empty());
            assert!(!message.trim().is_empty());
            assert!(!message.to_lowercase().contains("stack"));
            assert!(!message.contains("0x"));
        }
    }

    #[test]
    fn log_file_created() {
        let dir = std::env::temp_dir().join(format!("fm_log_{}", std::process::id()));
        let _g = init(&dir).unwrap();
        tracing::info!("test log entry");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(!files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("fm_log2_{}", std::process::id()));
        let _g1 = init(&dir).unwrap();
        let _g2 = init(&dir).unwrap(); // second call must not panic
    }
}
