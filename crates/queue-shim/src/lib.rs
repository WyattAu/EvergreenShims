//! Queue shim — background job processing.
//!
//! Manages a job queue for background tasks (email, reports, etc.).
//!
//! ## Environment Variables
//!
//! ```text
//! QUEUE_BACKEND          Backend: memory, redis (default: memory)
//! QUEUE_URL              Backend URL
//! QUEUE_MAX_WORKERS      Max concurrent workers (default: 4)
//! QUEUE_MAX_RETRIES      Max job retries (default: 3)
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Queue shim.
pub struct QueueShim {
    backend: String,
    url: Option<String>,
    max_workers: u32,
    max_retries: u32,
    jobs_enqueued: u64,
    jobs_processed: u64,
    jobs_failed: u64,
    jobs_retried: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl QueueShim {
    pub fn new() -> Self {
        Self {
            backend: std::env::var("QUEUE_BACKEND").unwrap_or_else(|_| "memory".to_string()),
            url: std::env::var("QUEUE_URL").ok(),
            max_workers: std::env::var("QUEUE_MAX_WORKERS").ok().and_then(|s| s.parse().ok()).unwrap_or(4),
            max_retries: std::env::var("QUEUE_MAX_RETRIES").ok().and_then(|s| s.parse().ok()).unwrap_or(3),
            jobs_enqueued: 0, jobs_processed: 0, jobs_failed: 0, jobs_retried: 0,
            shutdown_tx: None,
        }
    }
}

impl Default for QueueShim { fn default() -> Self { Self::new() } }

#[async_trait::async_trait]
impl Capability for QueueShim {
    fn name(&self) -> &str { "queue" }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("QueueShim initialized (backend={}, workers={})", self.backend, self.max_workers);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("QueueShim started ({} workers)", self.max_workers);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(true); }
        tracing::info!("QueueShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("queue_enqueued_total", self.jobs_enqueued as f64),
            Metric::new("queue_processed_total", self.jobs_processed as f64),
            Metric::new("queue_failed_total", self.jobs_failed as f64),
            Metric::new("queue_retried_total", self.jobs_retried as f64),
        ]
    }
}
