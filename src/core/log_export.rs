use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::logging::LOG_FILE_PREFIX;
#[cfg(test)]
use crate::core::logging::LOG_RETAINED_FILES;

pub const LOG_EXPORT_WINDOW_FILES: usize = 14;

#[derive(Debug, Clone)]
pub struct ExportLabels {
    pub title: String,
    pub version: String,
    pub exported_at: String,
    pub scope: String,
    pub journal_note: String,
}

#[derive(Debug)]
pub enum LogExportError {
    NoLogFiles,
    Io { path: PathBuf, source: io::Error },
}

fn is_log_file_name(name: &str) -> bool {
    if name == LOG_FILE_PREFIX {
        return true;
    }
    let Some(date) = name.strip_prefix(&format!("{LOG_FILE_PREFIX}.")) else {
        return false;
    };
    let bytes = date.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

/// Берём только последние четырнадцать файлов: ещё один старый файл остаётся
/// вне окна экспорта, чтобы дневная ротация могла удалить его безопасно.
pub fn recent_log_files(log_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(log_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_log_file_name(&name) {
            files.push(entry.path());
        }
    }
    files.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    let first_recent = files.len().saturating_sub(LOG_EXPORT_WINDOW_FILES);
    files.drain(..first_recent);
    Ok(files)
}

pub fn export_logs(
    log_dir: &Path,
    destination_dir: &Path,
    version: &str,
    exported_at: &str,
    labels: &ExportLabels,
) -> Result<PathBuf, LogExportError> {
    let paths = recent_log_files(log_dir).map_err(|source| LogExportError::Io {
        path: log_dir.to_path_buf(),
        source,
    })?;
    if paths.is_empty() {
        return Err(LogExportError::NoLogFiles);
    }

    // Открываем снимок источников до записи результата: ротация не обрывает чтение.
    let sources: Vec<_> = paths
        .into_iter()
        .map(|path| {
            File::open(&path)
                .map(|file| (path.clone(), file))
                .map_err(|source| LogExportError::Io { path, source })
        })
        .collect::<Result<_, _>>()?;

    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let safe_version: String = version
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
        .collect();
    let output_path = destination_dir.join(format!(
        "filemanager-log-{safe_version}-{epoch_nanos}.txt"
    ));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|source| LogExportError::Io {
            path: output_path.clone(),
            source,
        })?;

    let write_result = (|| -> Result<(), LogExportError> {
        writeln!(output, "{}", labels.title).map_err(|source| LogExportError::Io {
            path: output_path.clone(),
            source,
        })?;
        writeln!(output, "{}: {version}", labels.version).map_err(|source| {
            LogExportError::Io {
                path: output_path.clone(),
                source,
            }
        })?;
        writeln!(output, "{}: {exported_at}", labels.exported_at).map_err(|source| {
            LogExportError::Io {
                path: output_path.clone(),
                source,
            }
        })?;
        writeln!(output, "{}", labels.scope).map_err(|source| LogExportError::Io {
            path: output_path.clone(),
            source,
        })?;
        writeln!(output, "{}", labels.journal_note).map_err(|source| {
            LogExportError::Io {
                path: output_path.clone(),
                source,
            }
        })?;
        for (path, mut source) in sources {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            writeln!(output, "\n--- {name} ---").map_err(|source| LogExportError::Io {
                path: output_path.clone(),
                source,
            })?;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let bytes = source
                    .read(&mut buffer)
                    .map_err(|source| LogExportError::Io {
                        path: path.clone(),
                        source,
                    })?;
                if bytes == 0 {
                    break;
                }
                output
                    .write_all(&buffer[..bytes])
                    .map_err(|source| LogExportError::Io {
                        path: output_path.clone(),
                        source,
                    })?;
            }
            writeln!(output).map_err(|source| LogExportError::Io {
                path: output_path.clone(),
                source,
            })?;
        }
        output.flush().map_err(|source| LogExportError::Io {
            path: output_path.clone(),
            source,
        })?;
        output.sync_all().map_err(|source| LogExportError::Io {
            path: output_path.clone(),
            source,
        })
    })();
    if let Err(error) = write_result {
        drop(output);
        let _ = fs::remove_file(&output_path);
        return Err(error);
    }
    Ok(output_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fm-log-export-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn labels() -> ExportLabels {
        ExportLabels {
            title: "FileManager diagnostics".to_owned(),
            version: "Version".to_owned(),
            exported_at: "Exported at".to_owned(),
            scope: "Rolling file logs only".to_owned(),
            journal_note: "The in-memory operations journal is not included.".to_owned(),
        }
    }

    fn log_file(log_dir: &Path, day: usize, text: &str) -> PathBuf {
        let path = log_dir.join(format!("{LOG_FILE_PREFIX}.2026-01-{day:02}"));
        fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn export_writes_file_with_version_and_time_marker() {
        let root = scratch("metadata");
        let logs = root.join("logs");
        let destination = root.join("chosen-destination");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&destination).unwrap();
        log_file(&logs, 1, "diagnostic line\n");

        let exported = export_logs(
            &logs,
            &destination,
            env!("CARGO_PKG_VERSION"),
            "2026-11-13 12:34:56",
            &labels(),
        )
        .unwrap();
        let content = fs::read_to_string(&exported).unwrap();

        assert_eq!(exported.parent(), Some(destination.as_path()));
        assert!(content.contains(env!("CARGO_PKG_VERSION")));
        assert!(content.contains("2026-11-13 12:34:56"));
        assert!(content.contains("diagnostic line"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_reports_when_no_file_logs_are_available() {
        let root = scratch("empty");
        let destination = root.join("out");
        fs::create_dir_all(&destination).unwrap();
        assert!(matches!(
            export_logs(&root, &destination, "0.9.0", "export-time", &labels()),
            Err(LogExportError::NoLogFiles)
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_includes_file_log_diagnostic_at_info() {
        let root = scratch("diagnostic");
        let logs = root.join("logs");
        let destination = root.join("out");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&destination).unwrap();
        log_file(
            &logs,
            2,
            "INFO file operation failed error_kind=PermissionDenied path=C:\\Work\\locked.txt raw_os_error=5\n",
        );

        let exported =
            export_logs(&logs, &destination, "0.9.0", "export-time", &labels()).unwrap();
        let content = fs::read_to_string(exported).unwrap();
        assert!(content.contains("error_kind=PermissionDenied"));
        assert!(content.contains(r"C:\Work\locked.txt"));
        assert!(content.contains("raw_os_error=5"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_content_scope_is_defined_not_silent() {
        let root = scratch("scope");
        let logs = root.join("logs");
        let destination = root.join("out");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&destination).unwrap();
        log_file(&logs, 3, "one log line\n");

        let exported =
            export_logs(&logs, &destination, "0.9.0", "export-time", &labels()).unwrap();
        let content = fs::read_to_string(exported).unwrap();
        assert!(content.contains("Rolling file logs only"));
        assert!(content.contains("in-memory operations journal is not included"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn log_export_volume_limited() {
        let root = scratch("limit");
        for day in 1..=18 {
            log_file(&root, day, &format!("day-{day}\n"));
        }
        let recent = recent_log_files(&root).unwrap();
        assert_eq!(recent.len(), LOG_EXPORT_WINDOW_FILES);
        assert_eq!(
            recent[0].file_name().unwrap().to_string_lossy(),
            format!("{LOG_FILE_PREFIX}.2026-01-05")
        );
        assert_eq!(
            recent.last().unwrap().file_name().unwrap().to_string_lossy(),
            format!("{LOG_FILE_PREFIX}.2026-01-18")
        );
        assert!(LOG_RETAINED_FILES > LOG_EXPORT_WINDOW_FILES);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_rotation_limit_configured_and_does_not_lose_recent_diag() {
        let root = scratch("recent");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        for day in 1..=18 {
            let line = if day == 18 {
                "recent ErrorKind=PermissionDenied path=C:\\recent.txt\n".to_owned()
            } else {
                format!("old day {day}\n")
            };
            log_file(&root, day, &line);
        }
        let exported = export_logs(&root, &destination, "0.9.0", "export-time", &labels()).unwrap();
        let content = fs::read_to_string(exported).unwrap();
        assert!(content.contains("recent ErrorKind=PermissionDenied"));
        assert!(!content.contains("old day 1\n"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_does_not_delete_file_being_exported() {
        let root = scratch("preserve");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        let retention_grace = log_file(&root, 1, "outside export window\n");
        for day in 2..=LOG_RETAINED_FILES {
            log_file(&root, day, &format!("day-{day}\n"));
        }
        let recent = recent_log_files(&root).unwrap();
        assert_eq!(recent.len(), LOG_EXPORT_WINDOW_FILES);
        assert!(!recent.contains(&retention_grace));

        let exported = export_logs(&root, &destination, "0.9.0", "export-time", &labels()).unwrap();
        let content = fs::read_to_string(exported).unwrap();

        assert_eq!(
            fs::read_to_string(&retention_grace).unwrap(),
            "outside export window\n"
        );
        assert!(content.contains("day-2"));
        assert!(content.contains("day-15"));
        assert!(!content.contains("outside export window"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_path_via_existing_dialog_or_setting_not_hardcoded() {
        let root = scratch("destination");
        let logs = root.join("logs");
        let selected = root.join("user-selected");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&selected).unwrap();
        log_file(&logs, 1, "log\n");

        let exported = export_logs(&logs, &selected, "0.9.0", "export-time", &labels()).unwrap();

        assert_eq!(exported.parent(), Some(selected.as_path()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_no_automatic_send_or_telemetry() {
        let root = scratch("local-only");
        let logs = root.join("logs");
        let selected = root.join("selected");
        fs::create_dir_all(&logs).unwrap();
        fs::create_dir_all(&selected).unwrap();
        log_file(&logs, 1, "local diagnostic\n");

        let exported = export_logs(&logs, &selected, "0.9.0", "export-time", &labels()).unwrap();

        assert!(exported.starts_with(&selected));
        assert!(exported.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_reads_logger_file_without_sharing_violation() {
        let root = scratch("shared-reader");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        let mut logger_appender = crate::core::logging::file_appender(&root).unwrap();
        writeln!(logger_appender, "logger still holds this file").unwrap();
        logger_appender.flush().unwrap();

        let exported = export_logs(&root, &destination, "0.9.0", "export-time", &labels()).unwrap();

        assert!(fs::read_to_string(exported)
            .unwrap()
            .contains("logger still holds this file"));
        drop(logger_appender);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn panic_flushes_log_before_exit_and_export_sees_full_log() {
        let root = scratch("panic-flush");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        let appender = crate::core::logging::file_appender(&root).unwrap();
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(writer)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(
                panic = "simulated panic",
                backtrace = "complete panic backtrace marker",
                "application panic"
            );
        });
        drop(guard);

        let exported =
            export_logs(&root, &destination, "0.9.0", "export-time", &labels()).unwrap();
        let content = fs::read_to_string(exported).unwrap();
        assert!(content.contains("complete panic backtrace marker"));
        let _ = fs::remove_dir_all(root);
    }
}
