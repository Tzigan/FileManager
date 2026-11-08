//! Удаление, корзина, переименование и создание элементов.

use super::*;

impl<'a> Worker<'a> {
    // ------------------------------------------------------------------
    // Delete / Rename / Create
    // ------------------------------------------------------------------

    pub(super) fn delete_many(&mut self, job: &OpJob, sources: &[PathBuf]) -> io::Result<()> {
        let total = sources.len();
        for (i, src) in sources.iter().enumerate() {
            if self.cancelled(job) {
                return Ok(());
            }
            self.progress(
                job,
                OpStage::Deleting,
                src,
                i as u64,
                total as u64,
                i,
                total,
            );
            loop {
                match remove_any(src) {
                    Ok(()) => break,
                    Err(e) => match self.ask_error(job, src, &e) {
                        ErrorAction::Retry => continue,
                        ErrorAction::Skip => {
                            self.failed += 1;
                            break;
                        }
                        ErrorAction::Cancel => {
                            job.cancel.store(true, Ordering::Relaxed);
                            return Ok(());
                        }
                    },
                }
            }
        }
        Ok(())
    }

    /// Delete via the Recycle Bin in one IFileOperation call (progress
    /// reported per attempt — the bin handles subfolders itself).
    pub(super) fn recycle_many(&mut self, job: &OpJob, sources: &[PathBuf]) -> io::Result<()> {
        self.progress(
            job,
            OpStage::Deleting,
            sources
                .first()
                .map(|p| p.as_path())
                .unwrap_or(Path::new("")),
            0,
            sources.len() as u64,
            0,
            sources.len(),
        );
        if self.cancelled(job) {
            return Ok(());
        }
        loop {
            match crate::platform::recycle_bin(sources) {
                Ok(()) => return Ok(()),
                Err(e) => match self.ask_error(job, &sources[0], &e) {
                    ErrorAction::Retry => continue,
                    ErrorAction::Skip => {
                        self.failed += sources.len();
                        return Ok(());
                    }
                    ErrorAction::Cancel => return Ok(()),
                },
            }
        }
    }

    pub(super) fn rename(&mut self, job: &OpJob, from: &Path, to: &Path) -> io::Result<()> {
        self.progress(job, OpStage::Creating, from, 0, 1, 0, 1);
        let same_parent = from
            .parent()
            .zip(to.parent())
            .is_some_and(|(from_parent, to_parent)| same_existing_path(from_parent, to_parent));
        if !same_parent {
            let error = io::Error::new(
                io::ErrorKind::InvalidInput,
                "rename target must stay in the same folder",
            );
            match self.ask_error(job, from, &error) {
                ErrorAction::Skip | ErrorAction::Retry => self.failed += 1,
                ErrorAction::Cancel => {}
            }
            return Ok(());
        }

        let overwrite_confirmed = if to.exists() {
            match self.ask_conflict(job, from, to) {
                ConflictAction::Cancel => return Ok(()),
                ConflictAction::Skip => return Ok(()),
                _ => true,
            }
        } else {
            false
        };
        loop {
            match rename_with_overwrite(from, to, overwrite_confirmed) {
                Ok(()) => return Ok(()),
                Err(e) => match self.ask_error(job, from, &e) {
                    ErrorAction::Retry => continue,
                    ErrorAction::Skip => {
                        self.failed += 1;
                        return Ok(());
                    }
                    ErrorAction::Cancel => return Ok(()),
                },
            }
        }
    }

    pub(super) fn create(&mut self, job: &OpJob, path: &Path, dir: bool) -> io::Result<()> {
        self.progress(job, OpStage::Creating, path, 0, 1, 0, 1);
        let r = if dir {
            fs::create_dir(path)
        } else {
            fs::File::create_new(path).map(|_| ())
        };
        if let Err(e) = r {
            match self.ask_error(job, path, &e) {
                ErrorAction::Retry => self.create(job, path, dir),
                ErrorAction::Skip => {
                    self.failed += 1;
                    Ok(())
                }
                ErrorAction::Cancel => Ok(()),
            }
        } else {
            Ok(())
        }
    }
}
