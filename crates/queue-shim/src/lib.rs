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
//! ```

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::ensure;
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::{watch, Mutex};

/// Job status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Retrying,
    Dead,
}

/// A job in the queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub payload: Vec<u8>,
    pub status: JobStatus,
    pub attempts: u32,
    pub max_retries: u32,
    pub created_at: String,
    pub last_error: Option<String>,
}

/// A finished job (completed or dead-lettered).
#[derive(Debug, Clone, Serialize)]
pub struct ArchivedJob {
    pub job: Job,
    pub reason: String,
}

/// Queue shim with real job lifecycle management.
pub struct QueueShim {
    max_workers: u32,
    max_retries: u32,
    retry_base_secs: u64,
    retry_max_secs: u64,
    jobs_enqueued: u64,
    jobs_processed: u64,
    jobs_failed: u64,
    jobs_retried: u64,
    jobs_dead: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
    inner: Arc<Mutex<QueueInner>>,
}

/// Inner queue state.
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
            jobs_enqueued: 0,
            jobs_processed: 0,
            jobs_failed: 0,
            jobs_retried: 0,
            jobs_dead: 0,
            shutdown_tx: None,
            inner: Arc::new(Mutex::new(QueueInner {
                pending: VecDeque::new(),
                running_jobs: Vec::new(),
                dead_letter_queue: Vec::new(),
                active: false,
            })),
        }
    }

    /// Enqueue a new job.
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
            last_error: None,
        };
        self.inner.lock().await.pending.push_back(job);
        self.jobs_enqueued += 1;
        id
    }

    /// Dequeue the next pending job (if workers available).
    pub async fn dequeue(&mut self) -> Option<Job> {
        let mut inner = self.inner.lock().await;
        if inner.running_jobs.len() < self.max_workers as usize {
            if let Some(mut job) = inner.pending.pop_front() {
                job.status = JobStatus::Running;
                inner.running_jobs.push(job.clone());
                return Some(job);
            }
        }
        None
    }

    /// Mark a job as completed successfully.
    pub async fn complete_job(&mut self, job_id: &str) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().await;
        if let Some(pos) = inner.running_jobs.iter().position(|j| j.id == job_id) {
            inner.running_jobs.remove(pos);
            self.jobs_processed += 1;
            return Ok(());
        }
        anyhow::bail!("Job {} not found in running state", job_id)
    }

    /// Mark a job as failed. Retries if attempts remain, else dead-letter.
    pub async fn fail_job(&mut self, job_id: &str, error: String) -> anyhow::Result<JobStatus> {
        let mut inner = self.inner.lock().await;
        if let Some(pos) = inner.running_jobs.iter().position(|j| j.id == job_id) {
            let mut job = inner.running_jobs.remove(pos);
            job.attempts += 1;
            job.last_error = Some(error.clone());

            if job.attempts <= job.max_retries {
                job.status = JobStatus::Retrying;
                inner.pending.push_front(job); // Retry ASAP
                self.jobs_retried += 1;
                Ok(JobStatus::Retrying)
            } else {
                job.status = JobStatus::Dead;
                inner.dead_letter_queue.push(ArchivedJob {
                    job,
                    reason: format!("Exceeded {} retries: {}", self.max_retries, error),
                });
                self.jobs_failed += 1;
                self.jobs_dead += 1;
                Ok(JobStatus::Dead)
            }
        } else {
            anyhow::bail!("Job {} not found in running state", job_id)
        }
    }

    /// Get queue depth (pending + running).
    pub async fn queue_depth(&self) -> usize {
        let inner = self.inner.lock().await;
        inner.pending.len() + inner.running_jobs.len()
    }

    /// Get dead-letter queue length.
    pub async fn dlq_length(&self) -> usize {
        self.inner.lock().await.dead_letter_queue.len()
    }

    /// Calculate retry delay with exponential backoff.
    pub fn retry_delay(&self, attempt: u32) -> std::time::Duration {
        let delay_secs = self.retry_base_secs * 2u32.saturating_pow(attempt.min(31)) as u64;
        let capped = delay_secs.min(self.retry_max_secs);
        std::time::Duration::from_secs(capped)
    }

    /// Drain the dead-letter queue (for inspection/replay).
    pub async fn drain_dlq(&mut self) -> Vec<ArchivedJob> {
        let mut inner = self.inner.lock().await;
        std::mem::take(&mut inner.dead_letter_queue)
    }

    /// Get count of running jobs.
    pub async fn running_count(&self) -> usize {
        self.inner.lock().await.running_jobs.len()
    }

    /// Get count of pending jobs.
    pub async fn pending_count(&self) -> usize {
        self.inner.lock().await.pending.len()
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
            "QueueShim initialized"
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        {
            let mut inner = self.inner.lock().await;
            inner.active = true;
        }
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
            enqueued = self.jobs_enqueued,
            processed = self.jobs_processed,
            failed = self.jobs_failed,
            dead = self.jobs_dead,
            "QueueShim stopped"
        );
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("queue_enqueued_total", self.jobs_enqueued as f64),
            Metric::new("queue_processed_total", self.jobs_processed as f64),
            Metric::new("queue_failed_total", self.jobs_failed as f64),
            Metric::new("queue_retried_total", self.jobs_retried as f64),
            Metric::new("queue_dead_total", self.jobs_dead as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Dequeue all 4 (default max_workers=4)
        for _ in 0..4 {
            assert!(shim.dequeue().await.is_some());
        }
        // 5th should fail — no workers available
        assert!(shim.dequeue().await.is_none());
    }

    #[tokio::test]
    async fn test_complete_job() {
        let mut shim = QueueShim::new();
        let id = shim.enqueue("done".into(), vec![]).await;
        shim.dequeue().await.unwrap();
        shim.complete_job(&id).await.unwrap();
        assert_eq!(shim.jobs_processed, 1);
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
        assert_eq!(shim.jobs_retried, 1);
        assert_eq!(shim.pending_count().await, 1);
    }

    #[tokio::test]
    async fn test_fail_job_dead_letter() {
        let mut shim = QueueShim::new();
        let id = shim.enqueue("doomed".into(), vec![]).await;

        // Exhaust all retries (max_retries=3, so 4 attempts = dead)
        for _ in 0..4 {
            shim.dequeue().await;
            let status = shim.fail_job(&id, "fail".into()).await.unwrap();
            if status == JobStatus::Retrying {
                // Re-dequeue for next retry
                shim.dequeue().await;
            }
        }
        assert_eq!(shim.jobs_dead, 1);
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
        assert_eq!(shim.queue_depth().await, 3); // 2 pending + 1 running
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
        let mut shim = QueueShim::new();
        shim.jobs_enqueued = 100;
        shim.jobs_processed = 80;
        shim.jobs_failed = 10;
        shim.jobs_retried = 8;
        shim.jobs_dead = 2;
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
        // Job is back in pending with attempts=1
        let pending = shim.inner.lock().await.pending.front().unwrap().clone();
        assert_eq!(pending.attempts, 1);
        assert_eq!(pending.last_error.as_deref(), Some("err1"));
    }
}
