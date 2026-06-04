#![allow(dead_code)]
//! Archival shim — data archival to cold storage.
//!
//! Moves old data from hot storage to cold storage (S3, Glacier, etc.).
//!
//! ## Environment Variables
//!
//! ```text
//! ARCHIVAL_SCHEDULE      Cron schedule (default: 0 3 * * *)
//! ARCHIVAL_TABLES        Tables to archive
//! ARCHIVAL_AGE_DAYS      Archive data older than N days (default: 90)
//! ARCHIVAL_STORAGE       Storage: s3, glacier, local (default: s3)
//! ARCHIVAL_BUCKET        S3 bucket name
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Archival shim.
pub struct ArchivalShim {
    schedule: String,
    tables: Vec<String>,
    age_days: u32,
    storage: String,
    bucket: String,
    records_archived: u64,
    bytes_archived: u64,
    last_archival: Option<chrono::DateTime<chrono::Utc>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ArchivalShim {
    pub fn new() -> Self {
        Self {
            schedule: std::env::var("ARCHIVAL_SCHEDULE")
                .unwrap_or_else(|_| "0 3 * * *".to_string()),
            tables: std::env::var("ARCHIVAL_TABLES")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect(),
            age_days: std::env::var("ARCHIVAL_AGE_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(90),
            storage: std::env::var("ARCHIVAL_STORAGE").unwrap_or_else(|_| "s3".to_string()),
            bucket: std::env::var("ARCHIVAL_BUCKET").unwrap_or_default(),
            records_archived: 0,
            bytes_archived: 0,
            last_archival: None,
            shutdown_tx: None,
        }
    }
}

impl Default for ArchivalShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ArchivalShim {
    fn name(&self) -> &str {
        "archival"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "ArchivalShim initialized (schedule={}, age={}d, storage={})",
            self.schedule,
            self.age_days,
            self.storage
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ArchivalShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("ArchivalShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("archival_records_total", self.records_archived as f64),
            Metric::new("archival_bytes_total", self.bytes_archived as f64),
        ]
    }
}
