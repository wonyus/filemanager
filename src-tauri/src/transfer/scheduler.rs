use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    sync::{atomic::AtomicUsize, Arc},
};

use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

/// Priority values mirror the SDD contract.  Higher values are dequeued
/// first; FIFO order is preserved among equal priorities.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct JobPriority(pub u8);

impl JobPriority {
    pub const INTERACTIVE: Self = Self(100);
    pub const USER_TRANSFER: Self = Self(80);
    pub const RECURSIVE: Self = Self(60);
    pub const PLANNING: Self = Self(40);
    pub const PREVIEW: Self = Self(20);
    pub const HOUSEKEEPING: Self = Self(10);
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct SchedulerConfig {
    pub max_concurrent_jobs: usize,
    pub queue_capacity: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_jobs: 4,
            queue_capacity: 1_000,
        }
    }
}

impl SchedulerConfig {
    pub fn normalized(self) -> Self {
        Self {
            max_concurrent_jobs: self.max_concurrent_jobs.clamp(1, 16),
            queue_capacity: self.queue_capacity.clamp(1, 10_000),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct QueuedJob {
    id: Uuid,
    priority: JobPriority,
    sequence: u64,
}

impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SchedulerError {
    QueueFull,
    DuplicateJob,
    NotQueued,
    Closed,
}

struct SchedulerInner {
    queue: Mutex<BinaryHeap<QueuedJob>>,
    sequence: std::sync::atomic::AtomicU64,
    notify: Notify,
    semaphore: Arc<Semaphore>,
    max_concurrent_jobs: AtomicUsize,
    total_permits: AtomicUsize,
    queue_capacity: usize,
    closed: std::sync::atomic::AtomicBool,
}

/// A bounded, priority-aware queue.  `acquire_next` waits until both a queued
/// job and a concurrency permit are available, then returns an RAII permit;
/// dropping the permit releases the slot and wakes another waiter.
#[derive(Clone)]
pub struct TransferScheduler {
    inner: Arc<SchedulerInner>,
}

pub struct ScheduledJob {
    pub id: Uuid,
    permit: Option<OwnedSemaphorePermit>,
    scheduler: TransferScheduler,
}

impl Drop for ScheduledJob {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            let target = self
                .scheduler
                .inner
                .max_concurrent_jobs
                .load(std::sync::atomic::Ordering::Acquire);
            if self.scheduler.inner.semaphore.available_permits() >= target {
                permit.forget();
                self.scheduler
                    .inner
                    .total_permits
                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            } else {
                drop(permit);
            }
        }
        self.scheduler.inner.notify.notify_one();
    }
}

impl TransferScheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        let config = config.normalized();
        Self {
            inner: Arc::new(SchedulerInner {
                queue: Mutex::new(BinaryHeap::new()),
                sequence: std::sync::atomic::AtomicU64::new(0),
                notify: Notify::new(),
                semaphore: Arc::new(Semaphore::new(config.max_concurrent_jobs)),
                max_concurrent_jobs: AtomicUsize::new(config.max_concurrent_jobs),
                total_permits: AtomicUsize::new(config.max_concurrent_jobs),
                queue_capacity: config.queue_capacity,
                closed: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    pub async fn enqueue(&self, id: Uuid, priority: JobPriority) -> Result<(), SchedulerError> {
        if self.inner.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(SchedulerError::Closed);
        }
        let mut queue = self.inner.queue.lock().await;
        if queue.iter().any(|job| job.id == id) {
            return Err(SchedulerError::DuplicateJob);
        }
        if queue.len() >= self.inner.queue_capacity {
            return Err(SchedulerError::QueueFull);
        }
        let sequence = self
            .inner
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        queue.push(QueuedJob {
            id,
            priority,
            sequence,
        });
        drop(queue);
        self.inner.notify.notify_one();
        Ok(())
    }

    pub async fn cancel_queued(&self, id: Uuid) -> bool {
        let removed = self.remove_queued(id).await;
        if removed {
            self.inner.notify.notify_one();
        }
        removed
    }

    /// Acquire a semaphore permit for a specific queued job.  This is used by
    /// command-triggered workers so starting several jobs concurrently cannot
    /// bypass the global concurrency bound.
    pub async fn acquire_specific(&self, id: Uuid) -> Result<ScheduledJob, SchedulerError> {
        if self.inner.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(SchedulerError::Closed);
        }
        let permit = self
            .inner
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| SchedulerError::Closed)?;
        if !self.remove_queued(id).await {
            drop(ScheduledJob {
                id,
                permit: Some(permit),
                scheduler: self.clone(),
            });
            return Err(SchedulerError::NotQueued);
        }
        Ok(ScheduledJob {
            id,
            permit: Some(permit),
            scheduler: self.clone(),
        })
    }

    pub async fn pending_len(&self) -> usize {
        self.inner.queue.lock().await.len()
    }

    pub fn max_concurrent_jobs(&self) -> usize {
        self.inner
            .max_concurrent_jobs
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn set_max_concurrent_jobs(&self, max_concurrent_jobs: usize) -> usize {
        let target = max_concurrent_jobs.clamp(1, 16);
        self.inner
            .max_concurrent_jobs
            .store(target, std::sync::atomic::Ordering::Release);
        let current_total = self
            .inner
            .total_permits
            .load(std::sync::atomic::Ordering::Acquire);
        if target > current_total {
            self.inner.semaphore.add_permits(target - current_total);
            self.inner
                .total_permits
                .store(target, std::sync::atomic::Ordering::Release);
        } else if target < current_total {
            let removable = (current_total - target).min(self.inner.semaphore.available_permits());
            self.inner.semaphore.forget_permits(removable);
            self.inner
                .total_permits
                .fetch_sub(removable, std::sync::atomic::Ordering::AcqRel);
        }
        self.inner.notify.notify_waiters();
        target
    }

    async fn remove_queued(&self, id: Uuid) -> bool {
        let mut queue = self.inner.queue.lock().await;
        let before = queue.len();
        let retained = queue
            .drain()
            .filter(|job| job.id != id)
            .collect::<BinaryHeap<_>>();
        *queue = retained;
        queue.len() != before
    }

    pub fn close(&self) {
        self.inner
            .closed
            .store(true, std::sync::atomic::Ordering::Release);
        self.inner.notify.notify_waiters();
    }

    pub async fn acquire_next(&self) -> Option<ScheduledJob> {
        loop {
            if self.inner.closed.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            let permit = match self.inner.semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return None,
            };
            let notified = self.inner.notify.notified();
            let maybe_job = self.inner.queue.lock().await.pop();
            if let Some(job) = maybe_job {
                return Some(ScheduledJob {
                    id: job.id,
                    permit: Some(permit),
                    scheduler: self.clone(),
                });
            }
            drop(ScheduledJob {
                id: Uuid::nil(),
                permit: Some(permit),
                scheduler: self.clone(),
            });
            notified.await;
        }
    }

    pub async fn active_capacity(&self) -> usize {
        self.inner.semaphore.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn prioritizes_interactive_jobs_and_respects_capacity() {
        let scheduler = TransferScheduler::new(SchedulerConfig {
            max_concurrent_jobs: 1,
            queue_capacity: 2,
        });
        let low = Uuid::new_v4();
        let high = Uuid::new_v4();
        scheduler
            .enqueue(low, JobPriority::RECURSIVE)
            .await
            .unwrap();
        scheduler
            .enqueue(high, JobPriority::INTERACTIVE)
            .await
            .unwrap();
        let permit = scheduler.acquire_next().await.unwrap();
        assert_eq!(permit.id, high);
        drop(permit);
        let permit = scheduler.acquire_next().await.unwrap();
        assert_eq!(permit.id, low);
        drop(permit);
    }

    #[tokio::test]
    async fn queued_cancellation_and_full_queue_are_deterministic() {
        let scheduler = TransferScheduler::new(SchedulerConfig {
            max_concurrent_jobs: 1,
            queue_capacity: 1,
        });
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        scheduler
            .enqueue(first, JobPriority::USER_TRANSFER)
            .await
            .unwrap();
        assert_eq!(
            scheduler.enqueue(second, JobPriority::USER_TRANSFER).await,
            Err(SchedulerError::QueueFull)
        );
        assert!(scheduler.cancel_queued(first).await);
        assert_eq!(scheduler.pending_len().await, 0);
        assert!(timeout(Duration::from_millis(10), scheduler.acquire_next())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn specific_acquisition_holds_the_global_permit() {
        let scheduler = TransferScheduler::new(SchedulerConfig {
            max_concurrent_jobs: 1,
            queue_capacity: 4,
        });
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        scheduler
            .enqueue(first, JobPriority::USER_TRANSFER)
            .await
            .unwrap();
        scheduler
            .enqueue(second, JobPriority::USER_TRANSFER)
            .await
            .unwrap();
        let first_permit = scheduler.acquire_specific(first).await.unwrap();
        assert_eq!(first_permit.id, first);
        assert!(timeout(
            Duration::from_millis(10),
            scheduler.acquire_specific(second)
        )
        .await
        .is_err());
        drop(first_permit);
        let second_permit = scheduler.acquire_specific(second).await.unwrap();
        assert_eq!(second_permit.id, second);
    }

    #[tokio::test]
    async fn reconfiguration_resizes_without_exceeding_target() {
        let scheduler = TransferScheduler::new(SchedulerConfig {
            max_concurrent_jobs: 2,
            queue_capacity: 4,
        });
        assert_eq!(scheduler.max_concurrent_jobs(), 2);
        scheduler.set_max_concurrent_jobs(4);
        assert_eq!(scheduler.active_capacity().await, 4);
        let id = Uuid::new_v4();
        scheduler
            .enqueue(id, JobPriority::USER_TRANSFER)
            .await
            .unwrap();
        let permit = scheduler.acquire_next().await.unwrap();
        scheduler.set_max_concurrent_jobs(1);
        drop(permit);
        assert_eq!(scheduler.max_concurrent_jobs(), 1);
        assert!(scheduler.active_capacity().await <= 1);
        scheduler.set_max_concurrent_jobs(3);
        assert_eq!(scheduler.active_capacity().await, 3);
    }
}
