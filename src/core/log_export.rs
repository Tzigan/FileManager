use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::core::logging::{self, LOG_FILE_PREFIX};
#[cfg(test)]
use crate::core::logging::LOG_RETAINED_FILES;

pub const LOG_EXPORT_WINDOW_FILES: usize = 14;
pub const LOG_EXPORT_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const LOG_MARKER_TAIL_BYTES: u64 = 64 * 1024;
const LOG_MARKER_POLL_INTERVAL: Duration = Duration::from_millis(20);
const LOG_COPY_BUFFER_BYTES: usize = 64 * 1024;

static NEXT_EXPORT_MARKER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ExportLabels {
    pub title: String,
    pub version: String,
    pub exported_at: String,
    pub scope: String,
    pub journal_note: String,
    pub freshness_note: String,
    pub skipped_logs_note: String,
}

#[derive(Debug)]
pub enum LogExportError {
    NoLogFiles,
    Io {
        path: PathBuf,
        source: io::Error,
        incomplete_path: Option<PathBuf>,
    },
}

#[derive(Debug)]
pub struct ExportedLog {
    pub path: PathBuf,
    pub skipped_files: Vec<PathBuf>,
}

struct OpenLogSources {
    files: Vec<(PathBuf, File)>,
    skipped_files: Vec<PathBuf>,
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

pub fn wait_for_log_flush(log_dir: &Path, timeout: Duration) -> bool {
    let sequence = NEXT_EXPORT_MARKER.fetch_add(1, Ordering::Relaxed);
    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let marker = format!(
        "FM_LOG_EXPORT_BARRIER_{}_{}_{}",
        std::process::id(),
        epoch_nanos,
        sequence
    );
    tracing::info!(
        target: "filemanager::log_export",
        export_barrier = %marker,
        "log export flush marker"
    );

    let deadline = Instant::now() + timeout;
    loop {
        let marker_written = recent_log_files(log_dir)
            .ok()
            .is_some_and(|files| {
                files
                    .iter()
                    .rev()
                    .take(2)
                    .any(|path| log_tail_contains(path, &marker).unwrap_or(false))
            });
        if marker_written {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(LOG_MARKER_POLL_INTERVAL);
    }
}

fn log_tail_contains(path: &Path, marker: &str) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(
        length.saturating_sub(LOG_MARKER_TAIL_BYTES),
    ))?;
    let mut tail = Vec::new();
    file.take(LOG_MARKER_TAIL_BYTES).read_to_end(&mut tail)?;
    Ok(String::from_utf8_lossy(&tail).contains(marker))
}

fn io_error(path: &Path, source: io::Error) -> LogExportError {
    LogExportError::Io {
        path: path.to_path_buf(),
        source,
        incomplete_path: None,
    }
}

fn open_log_sources(
    paths: Vec<PathBuf>,
    current_log: &Path,
) -> Result<OpenLogSources, LogExportError> {
    let mut files = Vec::new();
    let mut skipped_files = Vec::new();
    for path in paths {
        match File::open(&path) {
            Ok(file) => files.push((path, file)),
            Err(source) if path != current_log => {
                logging::log_io_error("log export skipped an unavailable rotated file", None, Some(&path), &source);
                skipped_files.push(path);
            }
            Err(source) => return Err(io_error(&path, source)),
        }
    }
    if files.is_empty() {
        return Err(LogExportError::NoLogFiles);
    }
    Ok(OpenLogSources {
        files,
        skipped_files,
    })
}

fn write_line(
    output: &mut File,
    error_path: &Path,
    line: impl std::fmt::Display,
) -> Result<(), LogExportError> {
    writeln!(output, "{line}").map_err(|source| io_error(error_path, source))
}

fn write_export_contents(
    output: &mut File,
    temp_path: &Path,
    current_log: &Path,
    sources: Vec<(PathBuf, File)>,
    skipped_files: &mut Vec<PathBuf>,
    labels: &ExportLabels,
    version: &str,
    exported_at: &str,
) -> Result<(), LogExportError> {
    write_line(output, temp_path, &labels.title)?;
    write_line(output, temp_path, format!("{}: {version}", labels.version))?;
    write_line(
        output,
        temp_path,
        format!("{}: {exported_at}", labels.exported_at),
    )?;
    write_line(output, temp_path, &labels.scope)?;
    write_line(output, temp_path, &labels.journal_note)?;
    write_line(output, temp_path, &labels.freshness_note)?;

    for (path, mut source) in sources {
        let section_start = output
            .stream_position()
            .map_err(|error| io_error(temp_path, error))?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        write_line(output, temp_path, format!("\n--- {name} ---"))?;

        let mut buffer = [0u8; LOG_COPY_BUFFER_BYTES];
        let mut source_failed = None;
        loop {
            let bytes = match source.read(&mut buffer) {
                Ok(bytes) => bytes,
                Err(error) => {
                    source_failed = Some(error);
                    break;
                }
            };
            if bytes == 0 {
                break;
            }
            output
                .write_all(&buffer[..bytes])
                .map_err(|error| io_error(temp_path, error))?;
        }

        if let Some(source_error) = source_failed {
            if path == current_log {
                return Err(io_error(&path, source_error));
            }
            output
                .set_len(section_start)
                .map_err(|error| io_error(temp_path, error))?;
            output
                .seek(SeekFrom::Start(section_start))
                .map_err(|error| io_error(temp_path, error))?;
            logging::log_io_error(
                "log export skipped an unreadable rotated file",
                None,
                Some(&path),
                &source_error,
            );
            skipped_files.push(path);
            continue;
        }
        write_line(output, temp_path, "")?;
    }

    if !skipped_files.is_empty() {
        write_line(output, temp_path, format!("\n{}", labels.skipped_logs_note))?;
        for path in skipped_files.iter() {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            write_line(output, temp_path, name)?;
        }
    }
    Ok(())
}

fn remove_incomplete_temp(error: LogExportError, temp_path: &Path) -> LogExportError {
    let remains = match fs::remove_file(temp_path) {
        Ok(()) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => Some(temp_path.to_path_buf()),
    };
    match error {
        LogExportError::Io { path, source, .. } => LogExportError::Io {
            path,
            source,
            incomplete_path: remains,
        },
        LogExportError::NoLogFiles => LogExportError::NoLogFiles,
    }
}

fn write_atomic_export(
    destination_dir: &Path,
    file_name: &str,
    write: impl FnOnce(&mut File, &Path, &Path) -> Result<(), LogExportError>,
) -> Result<PathBuf, LogExportError> {
    let final_path = destination_dir.join(file_name);
    let temp_path = destination_dir.join(format!("{file_name}.partial"));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|source| io_error(&temp_path, source))?;
    let result = write(&mut output, &temp_path, &final_path)
        .and_then(|()| output.flush().map_err(|source| io_error(&temp_path, source)))
        .and_then(|()| output.sync_all().map_err(|source| io_error(&temp_path, source)));
    if let Err(error) = result {
        drop(output);
        return Err(remove_incomplete_temp(error, &temp_path));
    }
    drop(output);

    if let Err(source) = crate::platform::publish_file_no_replace(&temp_path, &final_path) {
        return Err(remove_incomplete_temp(
            io_error(&final_path, source),
            &temp_path,
        ));
    }
    Ok(final_path)
}

fn publish_log_sources(
    destination_dir: &Path,
    version: &str,
    exported_at: &str,
    current_log: &Path,
    mut sources: Vec<(PathBuf, File)>,
    mut skipped_files: Vec<PathBuf>,
    labels: &ExportLabels,
) -> Result<ExportedLog, LogExportError> {
    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let safe_version: String = version
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
        .collect();
    let file_name = format!("filemanager-log-{safe_version}-{epoch_nanos}.txt");
    let path = write_atomic_export(destination_dir, &file_name, |output, temp_path, _final_path| {
        write_export_contents(
            output,
            temp_path,
            current_log,
            std::mem::take(&mut sources),
            &mut skipped_files,
            labels,
            version,
            exported_at,
        )
    })?;
    Ok(ExportedLog {
        path,
        skipped_files,
    })
}

pub fn export_logs(
    log_dir: &Path,
    destination_dir: &Path,
    version: &str,
    exported_at: &str,
    labels: &ExportLabels,
) -> Result<ExportedLog, LogExportError> {
    let paths = recent_log_files(log_dir).map_err(|source| io_error(log_dir, source))?;
    if paths.is_empty() {
        return Err(LogExportError::NoLogFiles);
    }
    let current_log = paths.last().cloned().ok_or(LogExportError::NoLogFiles)?;
    let OpenLogSources {
        files,
        skipped_files,
    } = open_log_sources(paths, &current_log)?;
    publish_log_sources(
        destination_dir,
        version,
        exported_at,
        &current_log,
        files,
        skipped_files,
        labels,
    )
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
            freshness_note: "Flush marker confirmed.".to_owned(),
            skipped_logs_note: "Some rotated logs were skipped:".to_owned(),
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
        let content = fs::read_to_string(&exported.path).unwrap();

        assert_eq!(exported.path.parent(), Some(destination.as_path()));
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
        let content = fs::read_to_string(exported.path).unwrap();
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
        let content = fs::read_to_string(exported.path).unwrap();
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
        let content = fs::read_to_string(exported.path).unwrap();
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
        let content = fs::read_to_string(exported.path).unwrap();

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

        assert_eq!(exported.path.parent(), Some(selected.as_path()));
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

        assert!(exported.path.starts_with(&selected));
        assert!(exported.path.is_file());
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

        assert!(fs::read_to_string(exported.path)
            .unwrap()
            .contains("logger still holds this file"));
        drop(logger_appender);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_export_includes_recent_events_or_documents_pending_buffer_limitation() {
        let root = scratch("live-marker");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        let appender = crate::core::logging::file_appender(&root).unwrap();
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(writer)
            .finish();
        let (exported, freshness_confirmed) =
            tracing::subscriber::with_default(subscriber, || {
                tracing::error!(detail = "immediate diagnostic marker", "recent test error");
                let freshness_confirmed =
                    wait_for_log_flush(&root, LOG_EXPORT_FLUSH_TIMEOUT);
                let mut labels = labels();
                labels.freshness_note = if freshness_confirmed {
                    "flush marker confirmed".to_owned()
                } else {
                    "newest events may still be pending".to_owned()
                };
                let exported = export_logs(
                    &root,
                    &destination,
                    env!("CARGO_PKG_VERSION"),
                    "export-time",
                    &labels,
                )
                .unwrap();
                (exported, freshness_confirmed)
            });
        drop(guard);

        let content = fs::read_to_string(exported.path).unwrap();
        assert!(content.contains("immediate diagnostic marker"));
        if freshness_confirmed {
            assert!(content.contains("flush marker confirmed"));
        } else {
            assert!(content.contains("newest events may still be pending"));
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_flush_timeout_is_written_into_export_header() {
        let root = scratch("flush-timeout");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        log_file(&root, 1, "older diagnostic\n");

        let freshness_confirmed = wait_for_log_flush(&root, Duration::ZERO);
        assert!(!freshness_confirmed);
        let mut labels = labels();
        labels.freshness_note = "newest events may still be pending".to_owned();
        let exported =
            export_logs(&root, &destination, "0.9.0", "export-time", &labels).unwrap();

        assert!(
            fs::read_to_string(exported.path)
                .unwrap()
                .contains("newest events may still be pending")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_streams_files_without_loading_all_into_memory() {
        let root = scratch("streaming");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        let mut contents = vec![b'x'; 8 * 1024 * 1024];
        contents.extend_from_slice(b"END_OF_LARGE_LOG");
        let source = root.join(format!("{LOG_FILE_PREFIX}.2026-01-01"));
        fs::write(&source, &contents).unwrap();

        let exported = export_logs(&root, &destination, "0.9.0", "export-time", &labels()).unwrap();

        assert!(LOG_COPY_BUFFER_BYTES < contents.len());
        assert!(fs::metadata(exported.path).unwrap().len() > contents.len() as u64);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_failure_does_not_leave_misleading_complete_file() {
        let root = scratch("write-failure");
        let final_name = "failed-export.txt";
        let result = write_atomic_export(&root, final_name, |output, temp_path, _| {
            output.write_all(b"partial header").unwrap();
            Err(io_error(
                temp_path,
                io::Error::other("simulated disk full"),
            ))
        });

        assert!(matches!(
            result,
            Err(LogExportError::Io {
                incomplete_path: None,
                ..
            })
        ));
        assert!(!root.join(final_name).exists());
        assert!(!root.join(format!("{final_name}.partial")).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_failure_reports_partial_temp_when_cleanup_is_blocked() {
        use std::os::windows::fs::OpenOptionsExt;

        let root = scratch("temp-left");
        let temp_path = root.join("failed-export.txt.partial");
        fs::write(&temp_path, "partial output").unwrap();
        let blocking_handle = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0x0000_0001 | 0x0000_0002)
            .open(&temp_path)
            .unwrap();

        let error = remove_incomplete_temp(
            io_error(&temp_path, io::Error::other("simulated write failure")),
            &temp_path,
        );

        assert!(matches!(
            error,
            LogExportError::Io {
                incomplete_path: Some(path),
                ..
            } if path == temp_path
        ));
        drop(blocking_handle);
        fs::remove_file(temp_path).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn export_target_name_collision_handled() {
        let root = scratch("collision");
        let final_name = "existing-export.txt";
        let final_path = root.join(final_name);
        fs::write(&final_path, "previous complete export").unwrap();

        let result = write_atomic_export(&root, final_name, |output, _, _| {
            output.write_all(b"new export").unwrap();
            Ok(())
        });

        assert!(matches!(
            result,
            Err(LogExportError::Io {
                incomplete_path: None,
                ..
            })
        ));
        assert_eq!(
            fs::read_to_string(final_path).unwrap(),
            "previous complete export"
        );
        assert!(!root.join(format!("{final_name}.partial")).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rotation_during_export_does_not_miss_current_file() {
        let root = scratch("rotation-race");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        log_file(&root, 1, "old rotated diagnostic\n");
        let current = log_file(&root, 2, "current before rotation\n");
        let paths = recent_log_files(&root).unwrap();
        let OpenLogSources {
            files,
            skipped_files,
        } = open_log_sources(paths, &current).unwrap();

        log_file(&root, 3, "new file after rotation\n");
        let exported = publish_log_sources(
            &destination,
            "0.9.0",
            "export-time",
            &current,
            files,
            skipped_files,
            &labels(),
        )
        .unwrap();
        let content = fs::read_to_string(exported.path).unwrap();
        assert!(content.contains("current before rotation"));
        assert!(!content.contains("new file after rotation"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_or_unreadable_rotated_file_is_skipped_safely() {
        let root = scratch("missing-rotated");
        let destination = root.join("export");
        fs::create_dir_all(&destination).unwrap();
        let rotated = log_file(&root, 1, "rotated diagnostic\n");
        let current = log_file(&root, 2, "current diagnostic\n");
        let paths = recent_log_files(&root).unwrap();
        fs::remove_file(&rotated).unwrap();

        let OpenLogSources {
            files,
            skipped_files,
        } = open_log_sources(paths, &current).unwrap();
        let exported = publish_log_sources(
            &destination,
            "0.9.0",
            "export-time",
            &current,
            files,
            skipped_files,
            &labels(),
        )
        .unwrap();
        let content = fs::read_to_string(exported.path).unwrap();
        assert!(content.contains("current diagnostic"));
        assert!(content.contains("Some rotated logs were skipped"));
        assert!(content.contains("filemanager.log.2026-01-01"));
        assert_eq!(exported.skipped_files, vec![rotated]);
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
        let content = fs::read_to_string(exported.path).unwrap();
        assert!(content.contains("complete panic backtrace marker"));
        let _ = fs::remove_dir_all(root);
    }
}
