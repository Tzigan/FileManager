//! Копирование и перемещение файлов и каталогов.

use super::*;

impl<'a> Worker<'a> {
    // ------------------------------------------------------------------
    // Copy / Move
    // ------------------------------------------------------------------

    pub(super) fn copy_many(
        &mut self,
        job: &OpJob,
        sources: &[PathBuf],
        dest: &Path,
        is_move: bool,
    ) -> io::Result<()> {
        let mut safe_sources = Vec::with_capacity(sources.len());
        for src in sources {
            if self.cancelled(job) {
                return Ok(());
            }
            let is_dir = fs::symlink_metadata(src).is_ok_and(|meta| meta.is_dir());
            if is_dir && destination_is_inside_directory(src, dest) {
                let error = io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination is the source folder or one of its subfolders",
                );
                match self.ask_error(job, src, &error) {
                    ErrorAction::Skip | ErrorAction::Retry => self.failed += 1,
                    ErrorAction::Cancel => {
                        job.cancel.store(true, Ordering::Relaxed);
                        return Ok(());
                    }
                }
                continue;
            }
            safe_sources.push(src.clone());
        }
        let sources = safe_sources.as_slice();
        if sources.is_empty() {
            return Ok(());
        }

        // Stage 1: count totals (cancellable).
        self.progress(job, OpStage::Scanning, dest, 0, 0, 0, 0);
        let mut total_bytes = 0u64;
        let mut total_files = 0usize;
        for src in sources {
            if self.cancelled(job) {
                return Ok(());
            }
            if let Err(e) = scan(src, &mut total_bytes, &mut total_files, job) {
                match self.ask_error(job, src, &e) {
                    ErrorAction::Retry => {
                        let _ = scan(src, &mut total_bytes, &mut total_files, job);
                    }
                    ErrorAction::Skip => continue,
                    ErrorAction::Cancel => return Ok(()),
                }
            }
        }

        // Stage 2: free-space check (copy & cross-volume move only matter
        // for copies — rename is free; we still check, it's harmless).
        if let Some(free) = self.platform.free_space(dest) {
            if total_bytes > free {
                let e = io::Error::new(
                    io::ErrorKind::OutOfMemory,
                    "not enough space on destination",
                );
                match self.ask_error(job, dest, &e) {
                    ErrorAction::Retry => {}
                    _ => return Ok(()),
                }
            }
        }

        // Stage 3: process each source.
        let mut done_bytes = 0u64;
        let mut done_files = 0usize;
        for src in sources {
            if self.cancelled(job) {
                return Ok(());
            }
            let dst = dest.join(src.file_name().unwrap_or_default());
            // Pasting into the same folder — Explorer copies to "name — копия".
            let dst = if same_existing_path(src, &dst) {
                copy_variant(&dst)
            } else {
                dst
            };
            let stage = if is_move {
                OpStage::Moving
            } else {
                OpStage::Copying
            };
            self.progress(
                job,
                stage,
                src,
                done_bytes,
                total_bytes,
                done_files,
                total_files,
            );
            if is_move {
                self.move_entry(
                    job,
                    src,
                    &dst,
                    &mut done_bytes,
                    &mut done_files,
                    total_bytes,
                    total_files,
                );
            } else {
                let _ = self.copy_entry(
                    job,
                    src,
                    &dst,
                    &mut done_bytes,
                    &mut done_files,
                    total_bytes,
                    total_files,
                );
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn copy_entry(
        &mut self,
        job: &OpJob,
        src: &Path,
        dst: &Path,
        done_b: &mut u64,
        done_f: &mut usize,
        total_b: u64,
        total_f: usize,
    ) -> bool {
        if self.cancelled(job) {
            return false;
        }
        let meta = loop {
            match fs::symlink_metadata(src) {
                Ok(m) => break m,
                Err(e) => match self.ask_error(job, src, &e) {
                    ErrorAction::Retry => continue,
                    ErrorAction::Skip => {
                        self.failed += 1;
                        return false;
                    }
                    ErrorAction::Cancel => {
                        job.cancel.store(true, Ordering::Relaxed);
                        return false;
                    }
                },
            }
        };
        if meta.is_dir() {
            if !self.ensure_dir(job, dst) {
                self.failed += 1;
                return false;
            }
            let items = loop {
                match fs::read_dir(src) {
                    Ok(items) => break items,
                    Err(e) => match self.ask_error(job, src, &e) {
                        ErrorAction::Retry => continue,
                        ErrorAction::Skip => {
                            self.failed += 1;
                            return false;
                        }
                        ErrorAction::Cancel => {
                            job.cancel.store(true, Ordering::Relaxed);
                            return false;
                        }
                    },
                }
            };
            let mut complete = true;
            for item in items {
                let item = match item {
                    Ok(item) => item,
                    Err(e) => match self.ask_error(job, src, &e) {
                        ErrorAction::Retry => {
                            return self
                                .copy_entry(job, src, dst, done_b, done_f, total_b, total_f);
                        }
                        ErrorAction::Skip => {
                            self.failed += 1;
                            complete = false;
                            continue;
                        }
                        ErrorAction::Cancel => {
                            job.cancel.store(true, Ordering::Relaxed);
                            return false;
                        }
                    },
                };
                if !self.copy_entry(
                    job,
                    &item.path(),
                    &dst.join(item.file_name()),
                    done_b,
                    done_f,
                    total_b,
                    total_f,
                ) {
                    complete = false;
                }
            }
            return complete;
        }
        // File: resolve overwrite conflict first.
        if dst.exists() {
            match self.ask_conflict(job, src, dst) {
                ConflictAction::Cancel => {
                    job.cancel.store(true, Ordering::Relaxed);
                    return false;
                }
                ConflictAction::Skip => return false,
                _ => {}
            }
        }
        loop {
            match copy_file_stream(
                src,
                dst,
                &mut |written| {
                    *done_b += written;
                },
                self.platform,
                job,
            ) {
                Ok(()) => {
                    if self.cancelled(job) {
                        return false;
                    }
                    *done_f += 1;
                    self.progress(
                        job,
                        OpStage::Copying,
                        src,
                        *done_b,
                        total_b,
                        *done_f,
                        total_f,
                    );
                    return true;
                }
                Err(e) => match self.ask_error(job, src, &e) {
                    ErrorAction::Retry => continue,
                    ErrorAction::Skip => {
                        self.failed += 1;
                        return false;
                    }
                    ErrorAction::Cancel => {
                        job.cancel.store(true, Ordering::Relaxed);
                        return false;
                    }
                },
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn move_entry(
        &mut self,
        job: &OpJob,
        src: &Path,
        dst: &Path,
        done_b: &mut u64,
        done_f: &mut usize,
        total_b: u64,
        total_f: usize,
    ) {
        if self.cancelled(job) {
            return;
        }
        if dst.exists() {
            match self.ask_conflict(job, src, dst) {
                ConflictAction::Cancel => {
                    job.cancel.store(true, Ordering::Relaxed);
                    return;
                }
                ConflictAction::Skip => return,
                _ => {}
            }
        }
        // Fast path: rename (same volume).
        match fs::rename(src, dst) {
            Ok(()) => {
                let size = fs::symlink_metadata(dst).map(|m| m.len()).unwrap_or(0);
                *done_b += size;
                *done_f += 1;
                return;
            }
            Err(e) if is_cross_volume(&e) => {
                // copy → verify → delete; source is removed only on success.
                let copied = self.copy_entry(job, src, dst, done_b, done_f, total_b, total_f);
                match remove_move_source_after_copy(src, dst, copied, self.cancelled(job)) {
                    Ok(true) => {}
                    Ok(false) => {}
                    Err(e) => {
                        self.failed += 1;
                        let _ = self.ask_error(job, src, &e);
                    }
                }
            }
            Err(e) => match self.ask_error(job, src, &e) {
                ErrorAction::Retry => {
                    self.move_entry(job, src, dst, done_b, done_f, total_b, total_f)
                }
                ErrorAction::Skip => self.failed += 1,
                ErrorAction::Cancel => job.cancel.store(true, Ordering::Relaxed),
            },
        }
    }

    pub(super) fn ensure_dir(&mut self, job: &OpJob, dst: &Path) -> bool {
        if dst.is_dir() {
            return true; // merge into existing directory
        }
        if dst.exists() {
            match self.ask_conflict(job, dst, dst) {
                ConflictAction::Skip => return false,
                ConflictAction::Cancel => {
                    job.cancel.store(true, Ordering::Relaxed);
                    return false;
                }
                _ => {}
            }
        }
        match fs::create_dir_all(dst) {
            Ok(()) => true,
            Err(e) => match self.ask_error(job, dst, &e) {
                ErrorAction::Retry => self.ensure_dir(job, dst),
                ErrorAction::Skip => false,
                ErrorAction::Cancel => {
                    job.cancel.store(true, Ordering::Relaxed);
                    false
                }
            },
        }
    }
}
