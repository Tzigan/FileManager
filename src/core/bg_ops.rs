//! Ограниченная очередь тяжёлых независимых фоновых операций.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

pub const MIN_CONCURRENT_BG_OPS: usize = 1;
pub const DEFAULT_CONCURRENT_BG_OPS: usize = 2;
pub const MAX_CONCURRENT_BG_OPS: usize = 8;

pub fn clamp_concurrent_bg_ops(limit: usize) -> usize {
    limit.clamp(MIN_CONCURRENT_BG_OPS, MAX_CONCURRENT_BG_OPS)
}

/// В пул допускаются только независимые тяжёлые задачи.
/// Навигация, DetailsWatch, watcher, invalidate_dir, файловые операции и эскизы сюда не входят.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BgOpKind {
    Search,
    ZipCreate,
    ZipExtract,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BgOpEvent {
    Started {
        id: u64,
        kind: BgOpKind,
    },
    Finished {
        id: u64,
        kind: BgOpKind,
        panicked: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BgOpCancelOutcome {
    Queued,
    Running,
    NotFound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BgOpSubmitError {
    Cancelled,
    ShuttingDown,
}

type BgOpTask = Box<dyn FnOnce(Arc<AtomicBool>) + Send + 'static>;

struct QueuedBgOp {
    id: u64,
    kind: BgOpKind,
    cancel: Arc<AtomicBool>,
    task: BgOpTask,
}

#[derive(Default)]
struct PoolState {
    limit: usize,
    next_id: u64,
    queue: VecDeque<QueuedBgOp>,
    running: HashMap<u64, (BgOpKind, Arc<AtomicBool>)>,
    shutting_down: bool,
}

struct PoolInner {
    state: Mutex<PoolState>,
    ready: Condvar,
    events: Sender<BgOpEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

pub struct BgOpPool {
    inner: Arc<PoolInner>,
    workers: Vec<JoinHandle<()>>,
}

impl BgOpPool {
    pub fn new(
        limit: usize,
        events: Sender<BgOpEvent>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let inner = Arc::new(PoolInner {
            state: Mutex::new(PoolState {
                limit: clamp_concurrent_bg_ops(limit),
                next_id: 1,
                ..PoolState::default()
            }),
            ready: Condvar::new(),
            events,
            wake: Arc::new(wake),
        });
        let mut pool = Self {
            inner,
            workers: Vec::new(),
        };
        pool.ensure_workers(limit);
        pool
    }

    pub fn set_limit(&mut self, limit: usize) {
        let limit = clamp_concurrent_bg_ops(limit);
        if let Ok(mut state) = self.inner.state.lock() {
            state.limit = limit;
            self.inner.ready.notify_all();
        }
        self.ensure_workers(limit);
    }

    pub fn submit(
        &self,
        kind: BgOpKind,
        task: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
    ) -> Result<u64, BgOpSubmitError> {
        self.submit_with_cancel(kind, Arc::new(AtomicBool::new(false)), task)
    }

    pub fn submit_with_cancel(
        &self,
        kind: BgOpKind,
        cancel: Arc<AtomicBool>,
        task: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
    ) -> Result<u64, BgOpSubmitError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(BgOpSubmitError::Cancelled);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutting_down {
            return Err(BgOpSubmitError::ShuttingDown);
        }
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1).max(1);
        state.queue.push_back(QueuedBgOp {
            id,
            kind,
            cancel,
            task: Box::new(task),
        });
        self.inner.ready.notify_one();
        Ok(id)
    }

    pub fn cancel(&self, id: u64) -> BgOpCancelOutcome {
        let queued = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(index) = state.queue.iter().position(|job| job.id == id) {
                let job = state.queue.remove(index).expect("queued job index exists");
                job.cancel.store(true, Ordering::Relaxed);
                Some(job.kind)
            } else if let Some((_, cancel)) = state.running.get(&id) {
                cancel.store(true, Ordering::Relaxed);
                None
            } else {
                return BgOpCancelOutcome::NotFound;
            }
        };
        if let Some(kind) = queued {
            self.emit(BgOpEvent::Finished {
                id,
                kind,
                panicked: false,
            });
            self.inner.ready.notify_all();
            BgOpCancelOutcome::Queued
        } else {
            BgOpCancelOutcome::Running
        }
    }

    /// Убирает задачи, отменённые владельцем их токена, пока они ещё ждут запуска.
    pub fn prune_cancelled(&self) -> usize {
        let removed = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut removed = Vec::new();
            state.queue.retain(|job| {
                if job.cancel.load(Ordering::Relaxed) {
                    removed.push((job.id, job.kind));
                    false
                } else {
                    true
                }
            });
            removed
        };
        for (id, kind) in &removed {
            self.emit(BgOpEvent::Finished {
                id: *id,
                kind: *kind,
                panicked: false,
            });
        }
        if !removed.is_empty() {
            self.inner.ready.notify_all();
        }
        removed.len()
    }

    #[cfg(test)]
    pub fn queue_len(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .queue
            .len()
    }

    #[cfg(test)]
    pub fn running_len(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .running
            .len()
    }

    pub(crate) fn request_shutdown(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutting_down {
            return;
        }
        state.shutting_down = true;
        for job in state.queue.drain(..) {
            job.cancel.store(true, Ordering::Relaxed);
        }
        for (_, cancel) in state.running.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        drop(state);
        self.inner.ready.notify_all();
    }

    pub(crate) fn join_workers(&mut self) {
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }

    fn ensure_workers(&mut self, limit: usize) {
        while self.workers.len() < clamp_concurrent_bg_ops(limit) {
            let inner = Arc::clone(&self.inner);
            self.workers.push(thread::spawn(move || worker_loop(inner)));
        }
    }

    fn emit(&self, event: BgOpEvent) {
        emit(&self.inner, event);
    }
}

impl Drop for BgOpPool {
    fn drop(&mut self) {
        self.request_shutdown();
        self.join_workers();
    }
}

fn worker_loop(inner: Arc<PoolInner>) {
    loop {
        let job = {
            let mut state = inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                if state.shutting_down {
                    return;
                }
                if state.running.len() < state.limit
                    && let Some(job) = state.queue.pop_front()
                {
                    if job.cancel.load(Ordering::Relaxed) {
                        let event = BgOpEvent::Finished {
                            id: job.id,
                            kind: job.kind,
                            panicked: false,
                        };
                        drop(state);
                        emit(&inner, event);
                        state = inner
                            .state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        continue;
                    }
                    state
                        .running
                        .insert(job.id, (job.kind, Arc::clone(&job.cancel)));
                    break job;
                }
                state = inner
                    .ready
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };

        emit(
            &inner,
            BgOpEvent::Started {
                id: job.id,
                kind: job.kind,
            },
        );
        let panicked =
            catch_unwind(AssertUnwindSafe(|| (job.task)(Arc::clone(&job.cancel)))).is_err();
        inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .running
            .remove(&job.id);
        emit(
            &inner,
            BgOpEvent::Finished {
                id: job.id,
                kind: job.kind,
                panicked,
            },
        );
        inner.ready.notify_all();
    }
}

fn emit(inner: &PoolInner, event: BgOpEvent) {
    let _ = inner.events.send(event);
    let _ = catch_unwind(AssertUnwindSafe(|| (inner.wake)()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    fn pool(limit: usize) -> (BgOpPool, Receiver<BgOpEvent>) {
        let (events_tx, events_rx) = mpsc::channel();
        (BgOpPool::new(limit, events_tx, || {}), events_rx)
    }

    fn wait_for_event(
        events: &Receiver<BgOpEvent>,
        predicate: impl Fn(BgOpEvent) -> bool,
    ) -> BgOpEvent {
        loop {
            let event = events.recv_timeout(Duration::from_secs(5)).unwrap();
            if predicate(event) {
                return event;
            }
        }
    }

    fn wait_until_released(release: &Receiver<()>, cancel: &AtomicBool) {
        while !cancel.load(Ordering::Relaxed) && release.try_recv().is_err() {
            thread::yield_now();
        }
    }

    #[test]
    fn bg_ops_limit_uses_the_configured_safe_range() {
        assert_eq!(clamp_concurrent_bg_ops(0), MIN_CONCURRENT_BG_OPS);
        assert_eq!(clamp_concurrent_bg_ops(usize::MAX), MAX_CONCURRENT_BG_OPS);
        assert_eq!(
            clamp_concurrent_bg_ops(DEFAULT_CONCURRENT_BG_OPS),
            DEFAULT_CONCURRENT_BG_OPS
        );
    }

    #[test]
    fn bg_ops_limit_queues_extra_tasks() {
        let (pool, events) = pool(1);
        let (release_tx, release_rx) = mpsc::channel();
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let first = pool
            .submit(BgOpKind::Search, move |cancel| {
                let _ = first_started_tx.send(());
                wait_until_released(&release_rx, &cancel);
            })
            .unwrap();
        first_started_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let second_ran = Arc::new(AtomicBool::new(false));
        let second_ran_task = Arc::clone(&second_ran);
        let second = pool
            .submit(BgOpKind::ZipCreate, move |_| {
                second_ran_task.store(true, Ordering::Relaxed);
                let _ = second_started_tx.send(());
            })
            .unwrap();

        assert_eq!(pool.running_len(), 1);
        assert_eq!(pool.queue_len(), 1);
        assert!(!second_ran.load(Ordering::Relaxed));

        release_tx.send(()).unwrap();
        second_started_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let mut finished = HashSet::new();
        while finished.len() < 2 {
            if let BgOpEvent::Finished { id, .. } =
                events.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                finished.insert(id);
            }
        }
        assert!(second_ran.load(Ordering::Relaxed));
        assert!(finished.contains(&first));
        assert!(finished.contains(&second));
    }

    #[test]
    fn bg_ops_cancel_queued_task() {
        let (pool, events) = pool(1);
        let (release_tx, release_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        pool.submit(BgOpKind::Search, move |cancel| {
            let _ = started_tx.send(());
            wait_until_released(&release_rx, &cancel);
        })
        .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let side_effect = Arc::new(AtomicBool::new(false));
        let side_effect_task = Arc::clone(&side_effect);
        let queued = pool
            .submit(BgOpKind::ZipExtract, move |_| {
                side_effect_task.store(true, Ordering::Relaxed);
            })
            .unwrap();
        assert_eq!(pool.cancel(queued), BgOpCancelOutcome::Queued);
        assert_eq!(pool.queue_len(), 0);
        release_tx.send(()).unwrap();
        wait_for_event(
            &events,
            |event| matches!(event, BgOpEvent::Finished { id, .. } if id == queued),
        );
        assert!(!side_effect.load(Ordering::Relaxed));
    }

    #[test]
    fn bg_ops_cancel_running_task() {
        let (pool, events) = pool(1);
        let (started_tx, started_rx) = mpsc::channel();
        let (cancelled_tx, cancelled_rx) = mpsc::channel();
        let id = pool
            .submit(BgOpKind::Search, move |cancel| {
                let _ = started_tx.send(());
                while !cancel.load(Ordering::Relaxed) {
                    thread::yield_now();
                }
                let _ = cancelled_tx.send(());
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(pool.cancel(id), BgOpCancelOutcome::Running);
        cancelled_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            wait_for_event(&events, |event| {
                matches!(event, BgOpEvent::Finished { id: finished, .. } if finished == id)
            }),
            BgOpEvent::Finished {
                panicked: false,
                ..
            }
        ));
    }

    #[test]
    fn app_shutdown_with_pending_bg_ops() {
        let (pool, events) = pool(1);
        let (started_tx, started_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_task = Arc::clone(&stopped);
        pool.submit(BgOpKind::ZipExtract, move |cancel| {
            let _ = started_tx.send(());
            while !cancel.load(Ordering::Relaxed) {
                thread::yield_now();
            }
            stopped_task.store(true, Ordering::Relaxed);
        })
        .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let queued_ran = Arc::new(AtomicBool::new(false));
        let queued_ran_task = Arc::clone(&queued_ran);
        pool.submit(BgOpKind::Search, move |_| {
            queued_ran_task.store(true, Ordering::Relaxed);
        })
        .unwrap();

        drop(pool);
        assert!(stopped.load(Ordering::Relaxed));
        assert!(!queued_ran.load(Ordering::Relaxed));
        let _ = events.try_recv();
    }

    #[test]
    fn app_shutdown_with_pending_bg_ops_and_file_ops_still_hold() {
        let (mut pool, _events) = pool(1);
        let (started_tx, started_rx) = mpsc::channel();
        let pool_stopped = Arc::new(AtomicBool::new(false));
        let pool_stopped_task = Arc::clone(&pool_stopped);
        pool.submit(BgOpKind::ZipCreate, move |cancel| {
            let _ = started_tx.send(());
            while !cancel.load(Ordering::Relaxed) {
                thread::yield_now();
            }
            pool_stopped_task.store(true, Ordering::Relaxed);
        })
        .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let dir = std::env::temp_dir().join(format!("fm-bg-shutdown-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("must-not-be-created.txt");
        let cancel = Arc::new(AtomicBool::new(true));
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let (replies_tx, replies_rx) = mpsc::channel();
        let (ops_events_tx, ops_events_rx) = mpsc::channel();
        let ops_worker = crate::core::ops::spawn(
            jobs_rx,
            replies_rx,
            ops_events_tx,
            crate::platform::ops_platform(),
        );
        jobs_tx
            .send(crate::core::ops::OpJob {
                id: 41,
                kind: crate::core::ops::OpKind::CreateFile {
                    path: output.clone(),
                },
                cancel,
            })
            .unwrap();

        pool.request_shutdown();
        drop(jobs_tx);
        drop(replies_tx);
        ops_worker.join().unwrap();
        pool.join_workers();

        assert!(pool_stopped.load(Ordering::Relaxed));
        assert!(!output.exists());
        assert!(matches!(
            ops_events_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            crate::core::ops::OpEvent::Cancelled { id: 41 }
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn critical_flows_not_blocked_by_pool() {
        let (pool, _events) = pool(1);
        let (release_tx, release_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        pool.submit(BgOpKind::Search, move |cancel| {
            let _ = started_tx.send(());
            wait_until_released(&release_rx, &cancel);
        })
        .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(pool.running_len(), 1);

        let mut details = crate::core::entry_details::DetailsWatch::default();
        let crate::core::entry_details::DetailsStep::Start { generation, path } =
            details.follow(Some(Path::new(r"C:\critical.txt")))
        else {
            panic!("тяжёлые свойства должны стартовать независимо от занятого пула");
        };
        assert!(matches!(
            details.follow(Some(Path::new(r"C:\next.txt"))),
            crate::core::entry_details::DetailsStep::Start { .. }
        ));
        assert!(!details.receive(
            generation,
            &path,
            crate::core::entry_details::EntryDetails::unavailable()
        ));
        assert_eq!(crate::core::dir_watch::WATCH_COORDINATOR_THREADS, 1);

        let directory = std::env::temp_dir().join(format!("fm-bg-critical-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("entry.txt"), b"x").unwrap();
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            crate::core::fs::read_dir(&directory, &cancel),
            Err(crate::core::fs::ReadDirError::Cancelled)
        ));
        let _ = std::fs::remove_dir_all(directory);
        release_tx.send(()).unwrap();
    }

    #[test]
    fn flat_ops_and_notify_still_work() {
        let (pool, _events) = pool(1);
        let (release_tx, release_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        pool.submit(BgOpKind::Search, move |cancel| {
            let _ = started_tx.send(());
            wait_until_released(&release_rx, &cancel);
        })
        .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let directory =
            std::env::temp_dir().join(format!("fm-bg-invalidate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let key = crate::core::dir_cache::DirCache::cache_key(&directory).unwrap();
        let mut cache = crate::core::dir_cache::DirCache::new(2);
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, Vec::new()));
        cache.invalidate_dir(&directory);
        assert!(cache.get(&key).is_none());

        let mut debounce = crate::core::dir_watch::WatchDebounce::default();
        assert_eq!(
            debounce.note(crate::core::dir_watch::WatchSignal::Changed),
            crate::core::dir_watch::WatchReaction::Coalesce
        );
        assert!(matches!(
            debounce.flush(),
            Some(crate::core::dir_watch::WatchReaction::Reload { overflow: false })
        ));
        let _ = std::fs::remove_dir_all(&directory);
        release_tx.send(()).unwrap();
    }
}
