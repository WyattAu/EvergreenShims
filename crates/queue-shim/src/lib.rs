//! Queue shim — in-memory job queue with worker pool, retry, and dead-letter queue.
//!
//! Manages background job processing with configurable worker count,
//! exponential backoff retries, and a dead-letter queue for exhausted jobs.
//!
//! ## Environment Variables
//!
//! ```text
//! QUEUE_BACKEND          Backend: memory (default: memory)
//! QUEUE_MAX_WORKERS      Max concurrent workers (default: 4)
//! QUEUE_MAX_RETRIES      Max job retries (default: 3)
//! QUEUE_RETRY_BASE_SECS Base retry delay in seconds (default: 5)
//! QUEUE_RETRY_MAX_SECS  Max retry delay in seconds (default: 300)
//! QUEUE_JOB_TIMEOUT_SECS Job timeout in seconds (default: 300)
//! ```

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::{watch, Mutex};

const DEFAULT_JOB_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Retrying,
    Dead,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub payload: Vec<u8>,
    pub status: JobStatus,
    pub attempts: u32,
    pub max_retries: u32,
    pub created_at: String,
    pub started_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArchivedJob {
    pub job: Job,
    pub reason: String,
}

type JobHandler = Arc<
    dyn Fn(Job) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>
        + Send
        + Sync,
>;

pub struct QueueShim {
    max_workers: u32,
    max_retries: u32,
    retry_base_secs: u64,
    retry_max_secs: u64,
    job_timeout_secs: u64,
    jobs_enqueued: Arc<AtomicU64>,
    jobs_processed: Arc<AtomicU64>,
    jobs_failed: Arc<AtomicU64>,
    jobs_retried: Arc<AtomicU64>,
    jobs_dead: Arc<AtomicU64>,
    shutdown_tx: Option<watch::Sender<bool>>,
    inner: Arc<Mutex<QueueInner>>,
    handler: Option<JobHandler>,
}

struct QueueInner {
    pending: VecDeque<Job>,
    running_jobs: Vec<Job>,
    dead_letter_queue: Vec<ArchivedJob>,
    active: bool,
}

impl QueueShim {
    pub fn new() -> Self {
        Self {
            max_workers: std::env::var("QUEUE_MAX_WORKERS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(4),
            max_retries: std::env::var("QUEUE_MAX_RETRIES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            retry_base_secs: std::env::var("QUEUE_RETRY_BASE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            retry_max_secs: std::env::var("QUEUE_RETRY_MAX_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            job_timeout_secs: std::env::var("QUEUE_JOB_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_JOB_TIMEOUT_SECS),
            jobs_enqueued: Arc::new(AtomicU64::new(0)),
            jobs_processed: Arc::new(AtomicU64::new(0)),
            jobs_failed: Arc::new(AtomicU64::new(0)),
            jobs_retried: Arc::new(AtomicU64::new(0)),
            jobs_dead: Arc::new(AtomicU64::new(0)),
            shutdown_tx: None,
            inner: Arc::new(Mutex::new(QueueInner {
                pending: VecDeque::new(),
                running_jobs: Vec::new(),
                dead_letter_queue: Vec::new(),
                active: false,
            })),
            handler: None,
        }
    }

    pub fn set_handler<F>(&mut self, handler: F)
    where
        F: Fn(Job) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        self.handler = Some(Arc::new(handler));
    }

    pub async fn enqueue(&mut self, name: String, payload: Vec<u8>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let job = Job {
            id: id.clone(),
            name,
            payload,
            status: JobStatus::Pending,
            attempts: 0,
            max_retries: self.max_retries,
            created_at: chrono::Utc::now().to_rfc3339(),
            started_at: None,
            last_error: None,
        };
        self.inner.lock().await.pending.push_back(job);
        self.jobs_enqueued.fetch_add(1, Ordering::Relaxed);
        id
    }

    pub async fn dequeue(&mut self) -> Option<Job> {
        let mut inner = self.inner.lock().await;
        if inner.running_jobs.len() < self.max_workers as usize {
            if let Some(mut job) = inner.pending.pop_front() {
                job.status = JobStatus::Running;
                job.started_at = Some(chrono::Utc::now().to_rfc3339());
                inner.running_jobs.push(job.clone());
                return Some(job);
            }
        }
        None
    }

    pub async fn complete_job(&mut self, job_id: &str) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().await;
        if let Some(pos) = inner.running_jobs.iter().position(|j| j.id == job_id) {
            inner.running_jobs.remove(pos);
            self.jobs_processed.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        anyhow::bail!("Job {} not found in running state", job_id)
    }

    pub async fn fail_job(&mut self, job_id: &str, error: String) -> anyhow::Result<JobStatus> {
        let mut inner = self.inner.lock().await;
        if let Some(pos) = inner.running_jobs.iter().position(|j| j.id == job_id) {
            let mut job = inner.running_jobs.remove(pos);
            job.attempts += 1;
            job.last_error = Some(error.clone());

            if job.attempts <= job.max_retries {
                job.status = JobStatus::Retrying;
                let delay = self.retry_delay(job.attempts - 1);
                self.jobs_retried.fetch_add(1, Ordering::Relaxed);
                drop(inner);
                tokio::time::sleep(delay).await;
                let mut inner = self.inner.lock().await;
                job.status = JobStatus::Pending;
                job.started_at = None;
                inner.pending.push_back(job);
                Ok(JobStatus::Retrying)
            } else {
                job.status = JobStatus::Dead;
                inner.dead_letter_queue.push(ArchivedJob {
                    job,
                    reason: format!("Exceeded {} retries: {}", self.max_retries, error),
                });
                self.jobs_failed.fetch_add(1, Ordering::Relaxed);
                self.jobs_dead.fetch_add(1, Ordering::Relaxed);
                Ok(JobStatus::Dead)
            }
        } else {
            anyhow::bail!("Job {} not found in running state", job_id)
        }
    }

    pub async fn queue_depth(&self) -> usize {
        let inner = self.inner.lock().await;
        inner.pending.len() + inner.running_jobs.len()
    }

    pub async fn dlq_length(&self) -> usize {
        self.inner.lock().await.dead_letter_queue.len()
    }

    pub fn retry_delay(&self, attempt: u32) -> Duration {
        let delay_secs = self.retry_base_secs * 2u32.saturating_pow(attempt.min(31)) as u64;
        let capped = delay_secs.min(self.retry_max_secs);
        Duration::from_secs(capped)
    }

    pub async fn drain_dlq(&mut self) -> Vec<ArchivedJob> {
        let mut inner = self.inner.lock().await;
        std::mem::take(&mut inner.dead_letter_queue)
    }

    pub async fn running_count(&self) -> usize {
        self.inner.lock().await.running_jobs.len()
    }

    pub async fn pending_count(&self) -> usize {
        self.inner.lock().await.pending.len()
    }

    fn spawn_workers(&self, shutdown_rx: watch::Receiver<bool>) {
        let handler = match self.handler.clone() {
            Some(h) => h,
            None => return,
        };
        let max_workers = self.max_workers;
        let max_retries = self.max_retries;
        let job_timeout_secs = self.job_timeout_secs;

        for worker_id in 0..max_workers {
            let inner = Arc::clone(&self.inner);
            let handler = Arc::clone(&handler);
            let jobs_processed = Arc::clone(&self.jobs_processed);
            let jobs_failed = Arc::clone(&self.jobs_failed);
            let jobs_retried = Arc::clone(&self.jobs_retried);
            let jobs_dead = Arc::clone(&self.jobs_dead);
            let mut shutdown_rx = shutdown_rx.clone();

            tokio::spawn(async move {
                loop {
                    // Check shutdown
                    if *shutdown_rx.borrow() {
                        tracing::debug!(worker = worker_id, "Worker shutting down");
                        break;
                    }

                    // Try to dequeue
                    let job = {
                        let mut inner = inner.lock().await;
                        if let Some(mut job) = inner.pending.pop_front() {
                            // Check if job has timed out while pending
                            if let Some(ref started_str) = job.started_at {
                                if let Ok(started) =
                                    chrono::DateTime::parse_from_rfc3339(started_str)
                                {
                                    let elapsed = chrono::Utc::now()
                                        .signed_duration_since(started.with_timezone(&chrono::Utc));
                                    if elapsed.num_seconds() as u64 > job_timeout_secs {
                                        job.status = JobStatus::Dead;
                                        job.last_error = Some("Job timed out in queue".to_string());
                                        inner.dead_letter_queue.push(ArchivedJob {
                                            job,
                                            reason: "Timed out waiting for processing".to_string(),
                                        });
                                        jobs_dead.fetch_add(1, Ordering::Relaxed);
                                        continue;
                                    }
                                }
                            }
                            job.status = JobStatus::Running;
                            job.started_at = Some(chrono::Utc::now().to_rfc3339());
                            inner.running_jobs.push(job.clone());
                            Some(job)
                        } else {
                            None
                        }
                    };

                    let job = match job {
                        Some(j) => j,
                        None => {
                            tokio::select! {
                                _ = shutdown_rx.changed() => continue,
                                _ = tokio::time::sleep(Duration::from_millis(100)) => continue,
                            }
                        }
                    };

                    // Check execution timeout
                    let timed_out = job
                        .started_at
                        .as_ref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|started| {
                            let elapsed = chrono::Utc::now()
                                .signed_duration_since(started.with_timezone(&chrono::Utc));
                            elapsed.num_seconds() as u64 > job_timeout_secs
                        })
                        .unwrap_or(false);

                    if timed_out {
                        let mut inner = inner.lock().await;
                        if let Some(pos) = inner.running_jobs.iter().position(|j| j.id == job.id)
                        {
                            inner.running_jobs.remove(pos);
                        }
                        let mut job = job;
                        job.status = JobStatus::Dead;
                        job.last_error = Some("Job execution timed out".to_string());
                        inner.dead_letter_queue.push(ArchivedJob {
                            job,
                            reason: "Execution timed out".to_string(),
                        });
                        jobs_dead.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }

                    // Process the job
                    let result = handler(job.clone()).await;

                    let mut inner = inner.lock().await;
                    if let Some(pos) = inner.running_jobs.iter().position(|j| j.id == job.id) {
                        inner.running_jobs.remove(pos);
                    }

                    match result {
                        Ok(()) => {
                            jobs_processed.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            let mut job = job;
                            job.attempts += 1;
                            job.last_error = Some(e.to_string());

                            if job.attempts <= max_retries {
                                job.status = JobStatus::Pending;
                                job.started_at = None;
                                jobs_retried.fetch_add(1, Ordering::Relaxed);
                                // Enqueue for retry (lock is already held, just push)
                                inner.pending.push_back(job);
                            } else {
                                job.status = JobStatus::Dead;
                                inner.dead_letter_queue.push(ArchivedJob {
                                    job,
                                    reason: format!("Exceeded retries: {}", e),
                                });
                                jobs_failed.fetch_add(1, Ordering::Relaxed);
                                jobs_dead.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            });
        }
    }
}

impl Default for QueueShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for QueueShim {
    fn name(&self) -> &str {
        "queue"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            workers = self.max_workers,
            max_retries = self.max_retries,
            job_timeout_secs = self.job_timeout_secs,
            "QueueShim initialized"
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        {
            let mut inner = self.inner.lock().await;
            inner.active = true;
        }
        self.spawn_workers(shutdown_rx);
        tracing::info!(workers = self.max_workers, "QueueShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        {
            let mut inner = self.inner.lock().await;
            inner.active = false;
        }
        tracing::info!(
            enqueued = self.jobs_enqueued.load(Ordering::Relaxed),
            processed = self.jobs_processed.load(Ordering::Relaxed),
            failed = self.jobs_failed.load(Ordering::Relaxed),
            dead = self.jobs_dead.load(Ordering::Relaxed),
            "QueueShim stopped"
        );
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new(
                "queue_enqueued_total",
                self.jobs_enqueued.load(Ordering::Relaxed) as f64,
            ),
            Metric::new(
                "queue_processed_total",
                self.jobs_processed.load(Ordering::Relaxed) as f64,
            ),
            Metric::new(
                "queue_failed_total",
                self.jobs_failed.load(Ordering::Relaxed) as f64,
            ),
            Metric::new(
                "queue_retried_total",
                self.jobs_retried.load(Ordering::Relaxed) as f64,
            ),
            Metric::new(
                "queue_dead_total",
                self.jobs_dead.load(Ordering::Relaxed) as f64,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[tokio::test]
    async fn test_enqueue_dequeue() {
        let mut shim = QueueShim::new();
        let id = shim.enqueue("test-job".into(), b"payload".to_vec()).await;
        let job = shim.dequeue().await.unwrap();
        assert_eq!(job.id, id);
        assert_eq!(job.name, "test-job");
        assert_eq!(job.status, JobStatus::Running);
    }

    #[tokio::test]
    async fn test_dequeue_respects_worker_limit() {
        let mut shim = QueueShim::new();
        for i in 0..4 {
            shim.enqueue(format!("job-{}", i), vec![]).await;
        }
        for _ in 0..4 {
            assert!(shim.dequeue().await.is_some());
        }
        assert!(shim.dequeue().await.is_none());
    }

    #[tokio::test]
    async fn test_complete_job() {
        let mut shim = QueueShim::new();
        let id = shim.enqueue("done".into(), vec![]).await;
        shim.dequeue().await.unwrap();
        shim.complete_job(&id).await.unwrap();
        assert_eq!(shim.jobs_processed.load(Ordering::Relaxed), 1);
        assert_eq!(shim.running_count().await, 0);
    }

    #[tokio::test]
    async fn test_complete_nonexistent_job() {
        let mut shim = QueueShim::new();
        assert!(shim.complete_job("nope").await.is_err());
    }

    #[tokio::test]
    async fn test_fail_job_retries() {
        let mut shim = QueueShim::new();
        let id = shim.enqueue("flaky".into(), vec![]).await;
        shim.dequeue().await.unwrap();
        let status = shim.fail_job(&id, "timeout".into()).await.unwrap();
        assert_eq!(status, JobStatus::Retrying);
        assert_eq!(shim.jobs_retried.load(Ordering::Relaxed), 1);
        assert_eq!(shim.pending_count().await, 1);
    }

    #[tokio::test]
    async fn test_fail_job_dead_letter() {
        let mut shim = QueueShim::new();
        let id = shim.enqueue("doomed".into(), vec![]).await;

        for _ in 0..4 {
            shim.dequeue().await;
            let status = shim.fail_job(&id, "fail".into()).await.unwrap();
            if status == JobStatus::Retrying {
                shim.dequeue().await;
            }
        }
        assert_eq!(shim.jobs_dead.load(Ordering::Relaxed), 1);
        assert_eq!(shim.dlq_length().await, 1);
    }

    #[tokio::test]
    async fn test_fail_nonexistent_job() {
        let mut shim = QueueShim::new();
        assert!(shim.fail_job("nope", "err".into()).await.is_err());
    }

    #[tokio::test]
    async fn test_drain_dlq() {
        let mut shim = QueueShim::new();
        let id = shim.enqueue("dlq-test".into(), vec![]).await;
        shim.dequeue().await;
        for _ in 0..4 {
            let status = shim.fail_job(&id, "boom".into()).await.unwrap();
            if status == JobStatus::Retrying {
                shim.dequeue().await;
            }
        }
        let dlq = shim.drain_dlq().await;
        assert_eq!(dlq.len(), 1);
        assert_eq!(dlq[0].job.id, id);
        assert_eq!(shim.dlq_length().await, 0);
    }

    #[tokio::test]
    async fn test_queue_depth() {
        let mut shim = QueueShim::new();
        shim.enqueue("a".into(), vec![]).await;
        shim.enqueue("b".into(), vec![]).await;
        shim.enqueue("c".into(), vec![]).await;
        assert_eq!(shim.queue_depth().await, 3);
        shim.dequeue().await;
        assert_eq!(shim.queue_depth().await, 3);
    }

    #[test]
    fn test_retry_delay_exponential() {
        let shim = QueueShim::new();
        let d0 = shim.retry_delay(0);
        let d1 = shim.retry_delay(1);
        let d2 = shim.retry_delay(2);
        assert!(d2 > d1);
        assert!(d1 > d0);
    }

    #[test]
    fn test_retry_delay_capped() {
        let shim = QueueShim::new();
        let d = shim.retry_delay(20);
        assert!(d.as_secs() <= shim.retry_max_secs);
    }

    #[test]
    fn test_metrics() {
        let shim = QueueShim {
            jobs_enqueued: Arc::new(AtomicU64::new(100)),
            jobs_processed: Arc::new(AtomicU64::new(80)),
            jobs_failed: Arc::new(AtomicU64::new(10)),
            jobs_retried: Arc::new(AtomicU64::new(8)),
            jobs_dead: Arc::new(AtomicU64::new(2)),
            ..QueueShim::new()
        };
        let m = shim.metrics();
        assert_eq!(m.len(), 5);
    }

    #[test]
    fn test_env_config() {
        std::env::set_var("QUEUE_MAX_WORKERS", "8");
        std::env::set_var("QUEUE_MAX_RETRIES", "5");
        let shim = QueueShim::new();
        assert_eq!(shim.max_workers, 8);
        assert_eq!(shim.max_retries, 5);
        std::env::remove_var("QUEUE_MAX_WORKERS");
        std::env::remove_var("QUEUE_MAX_RETRIES");
    }

    #[tokio::test]
    async fn test_job_attempts_tracked() {
        let mut shim = QueueShim::new();
        let id = shim.enqueue("attempt-track".into(), vec![]).await;
        shim.dequeue().await;
        shim.fail_job(&id, "err1".into()).await.unwrap();
        let pending = shim
            .inner
            .lock()
            .await
            .pending
            .front()
            .unwrap()
            .clone();
        assert_eq!(pending.attempts, 1);
        assert_eq!(pending.last_error.as_deref(), Some("err1"));
    }

    #[tokio::test]
    async fn test_worker_processes_job() {
        let mut shim = QueueShim::new();
        let processed = Arc::new(AtomicU64::new(0));
        let processed_clone = Arc::clone(&processed);
        shim.set_handler(move |_job| {
            let p = Arc::clone(&processed_clone);
            Box::pin(async move {
                p.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
        });

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        shim.shutdown_tx = Some(shutdown_tx);
        {
            let mut inner = shim.inner.lock().await;
            inner.active = true;
        }
        shim.spawn_workers(shutdown_rx);

        shim.enqueue("worker-test".into(), vec![1, 2, 3]).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        assert_eq!(processed.load(Ordering::Relaxed), 1);
        assert_eq!(shim.running_count().await, 0);
        let _ = shim.shutdown_tx.as_ref().unwrap().send(true);
    }

    #[tokio::test]
    async fn test_worker_retries_on_failure() {
        let mut shim = QueueShim::new();
        let attempts = Arc::new(AtomicU64::new(0));
        let attempts_clone = Arc::clone(&attempts);
        shim.set_handler(move |_job| {
            let a = Arc::clone(&attempts_clone);
            Box::pin(async move {
                a.fetch_add(1, Ordering::Relaxed);
                anyhow::bail!("simulated failure")
            })
        });

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        shim.shutdown_tx = Some(shutdown_tx);
        {
            let mut inner = shim.inner.lock().await;
            inner.active = true;
        }
        shim.spawn_workers(shutdown_rx);

        shim.enqueue("retry-test".into(), vec![]).await;
        tokio::time::sleep(Duration::from_secs(8)).await;

        assert!(attempts.load(Ordering::Relaxed) >= 2);
        let _ = shim.shutdown_tx.as_ref().unwrap().send(true);
    }
}
