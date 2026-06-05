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
//! ARCHIVAL_COMPRESSION   Compression: none, gzip, zstd (default: zstd)
//! ARCHIVAL_LIFECYCLE_DAYS Days before moving to glacier (default: 0, keep in s3)
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Archive lifecycle tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StorageTier {
    Hot,
    Warm,
    Cold,
}

impl std::fmt::Display for StorageTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hot => write!(f, "hot"),
            Self::Warm => write!(f, "warm"),
            Self::Cold => write!(f, "cold"),
        }
    }
}

/// An archived record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedRecord {
    pub id: String,
    pub table: String,
    pub original_size_bytes: u64,
    pub archived_size_bytes: u64,
    pub archive_path: String,
    pub archived_at: String,
    pub storage_tier: StorageTier,
    pub retention_until: String,
    pub compressed: bool,
}

/// Retention rule for a table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionRule {
    pub table: String,
    pub age_days: u32,
    pub lifecycle_days: u32,
    pub storage_tier: StorageTier,
}

/// Archival summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivalSummary {
    pub tables_archived: HashMap<String, u64>,
    pub total_records: u64,
    pub total_bytes_saved: u64,
    pub compression_ratio: f64,
}

/// Archival shim.
pub struct ArchivalShim {
    schedule: String,
    tables: Vec<String>,
    age_days: u32,
    storage: String,
    bucket: String,
    compression: String,
    lifecycle_days: u32,
    records_archived: u64,
    bytes_archived: u64,
    bytes_saved: u64,
    last_archival: Option<chrono::DateTime<chrono::Utc>>,
    retention_rules: HashMap<String, RetentionRule>,
    archive_log: Vec<ArchivedRecord>,
    record_counter: u64,
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
            compression: std::env::var("ARCHIVAL_COMPRESSION")
                .unwrap_or_else(|_| "zstd".to_string()),
            lifecycle_days: std::env::var("ARCHIVAL_LIFECYCLE_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            records_archived: 0,
            bytes_archived: 0,
            bytes_saved: 0,
            last_archival: None,
            retention_rules: HashMap::new(),
            archive_log: Vec::new(),
            record_counter: 0,
            shutdown_tx: None,
        }
    }

    /// Add a retention rule for a specific table.
    pub fn add_retention_rule(&mut self, rule: RetentionRule) {
        self.retention_rules.insert(rule.table.clone(), rule);
    }

    /// Simulate archiving a batch of records from a table.
    pub fn archive_batch(
        &mut self,
        table: &str,
        count: u64,
        original_size_bytes: u64,
    ) -> ArchivedRecord {
        self.record_counter += 1;

        let compression_ratio = match self.compression.as_str() {
            "gzip" => 0.3,
            "zstd" => 0.25,
            _ => 1.0,
        };

        let archived_size = (original_size_bytes as f64 * compression_ratio) as u64;
        let saved = original_size_bytes.saturating_sub(archived_size);

        let archive_path = format!(
            "{}/{}/archive-{}.dat",
            self.bucket, table, self.record_counter
        );
        let retention_days = self
            .retention_rules
            .get(table)
            .map(|r| r.age_days)
            .unwrap_or(self.age_days);
        let storage_tier = self
            .retention_rules
            .get(table)
            .map(|r| r.storage_tier.clone())
            .unwrap_or(StorageTier::Cold);

        let record = ArchivedRecord {
            id: format!("arc-{:010}", self.record_counter),
            table: table.to_string(),
            original_size_bytes,
            archived_size_bytes: archived_size,
            archive_path: archive_path.clone(),
            archived_at: chrono::Utc::now().to_rfc3339(),
            storage_tier,
            retention_until: (chrono::Utc::now() + chrono::Duration::days(retention_days as i64))
                .to_rfc3339(),
            compressed: self.compression != "none",
        };

        self.records_archived += count;
        self.bytes_archived += archived_size;
        self.bytes_saved += saved;
        self.last_archival = Some(chrono::Utc::now());
        self.archive_log.push(record);

        self.archive_log.last().unwrap().clone()
    }

    /// Check if a record's retention has expired.
    pub fn is_retention_expired(&self, record: &ArchivedRecord) -> bool {
        if let Ok(until) = record
            .retention_until
            .parse::<chrono::DateTime<chrono::Utc>>()
        {
            chrono::Utc::now() > until
        } else {
            false
        }
    }

    /// Purge expired archives.
    pub fn purge_expired(&mut self) -> u64 {
        let before = self.archive_log.len();
        self.archive_log.retain(|r| {
            let now = chrono::Utc::now();
            if let Ok(until) = r.retention_until.parse::<chrono::DateTime<chrono::Utc>>() {
                now <= until
            } else {
                true
            }
        });
        (before - self.archive_log.len()) as u64
    }

    /// Transition archives to colder storage based on lifecycle rules.
    pub fn apply_lifecycle(&mut self) -> u64 {
        let mut transitioned = 0u64;
        for record in &mut self.archive_log {
            if record.storage_tier == StorageTier::Hot {
                if let Ok(archived_at) = record.archived_at.parse::<chrono::DateTime<chrono::Utc>>()
                {
                    let age_days = (chrono::Utc::now() - archived_at).num_days() as u32;
                    let lifecycle = self.lifecycle_days;
                    if lifecycle > 0 && age_days >= lifecycle {
                        record.storage_tier = StorageTier::Cold;
                        transitioned += 1;
                    } else if age_days >= lifecycle / 2 {
                        record.storage_tier = StorageTier::Warm;
                        transitioned += 1;
                    }
                }
            }
        }
        transitioned
    }

    /// Get archival summary.
    pub fn summary(&self) -> ArchivalSummary {
        let mut tables: HashMap<String, u64> = HashMap::new();
        for record in &self.archive_log {
            *tables.entry(record.table.clone()).or_insert(0) += 1;
        }

        let compression_ratio = if self.bytes_archived > 0 {
            self.bytes_archived as f64 / (self.bytes_archived + self.bytes_saved) as f64
        } else {
            1.0
        };

        ArchivalSummary {
            tables_archived: tables,
            total_records: self.archive_log.len() as u64,
            total_bytes_saved: self.bytes_saved,
            compression_ratio,
        }
    }

    /// Get archive count.
    pub fn archive_count(&self) -> usize {
        self.archive_log.len()
    }

    /// Get compression ratio estimate.
    pub fn compression_ratio(&self) -> f64 {
        match self.compression.as_str() {
            "gzip" => 0.3,
            "zstd" => 0.25,
            _ => 1.0,
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
            Metric::new("archival_bytes_saved", self.bytes_saved as f64),
            Metric::new("archival_archive_count", self.archive_log.len() as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_tier_display() {
        assert_eq!(StorageTier::Hot.to_string(), "hot");
        assert_eq!(StorageTier::Warm.to_string(), "warm");
        assert_eq!(StorageTier::Cold.to_string(), "cold");
    }

    #[test]
    fn test_archive_batch_gzip() {
        let mut shim = ArchivalShim {
            compression: "gzip".to_string(),
            bucket: "my-bucket".to_string(),
            ..ArchivalShim::new()
        };
        let record = shim.archive_batch("orders", 1000, 10_000_000);

        assert_eq!(record.table, "orders");
        assert!(record.compressed);
        assert!(record.archived_size_bytes < record.original_size_bytes);
        assert_eq!(shim.records_archived, 1000);
        assert_eq!(shim.archive_count(), 1);
    }

    #[test]
    fn test_archive_batch_no_compression() {
        let mut shim = ArchivalShim {
            compression: "none".to_string(),
            ..ArchivalShim::new()
        };
        let record = shim.archive_batch("users", 100, 1_000_000);

        assert!(!record.compressed);
        assert_eq!(record.archived_size_bytes, record.original_size_bytes);
        assert_eq!(shim.bytes_saved, 0);
    }

    #[test]
    fn test_compression_ratio() {
        let shim = ArchivalShim {
            compression: "gzip".to_string(),
            ..ArchivalShim::new()
        };
        assert!((shim.compression_ratio() - 0.3).abs() < 0.01);

        let shim2 = ArchivalShim {
            compression: "zstd".to_string(),
            ..ArchivalShim::new()
        };
        assert!((shim2.compression_ratio() - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_retention_rule_per_table() {
        let mut shim = ArchivalShim::new();
        shim.add_retention_rule(RetentionRule {
            table: "logs".to_string(),
            age_days: 30,
            lifecycle_days: 7,
            storage_tier: StorageTier::Cold,
        });

        let record = shim.archive_batch("logs", 100, 500_000);
        assert_eq!(record.storage_tier, StorageTier::Cold);
    }

    #[test]
    fn test_is_retention_expired() {
        let shim = ArchivalShim::new();
        let mut record = ArchivedRecord {
            id: "test".to_string(),
            table: "test".to_string(),
            original_size_bytes: 100,
            archived_size_bytes: 50,
            archive_path: "path".to_string(),
            archived_at: chrono::Utc::now().to_rfc3339(),
            storage_tier: StorageTier::Cold,
            retention_until: (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339(),
            compressed: false,
        };
        assert!(shim.is_retention_expired(&record));

        record.retention_until = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
        assert!(!shim.is_retention_expired(&record));
    }

    #[test]
    fn test_purge_expired() {
        let mut shim = ArchivalShim::new();
        shim.archive_batch("orders", 1, 1000);

        let last_record = shim.archive_log.last().unwrap().clone();
        let mut expired = last_record.clone();
        expired.retention_until = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        expired.id = "expired".to_string();
        shim.archive_log.push(expired);

        assert_eq!(shim.archive_count(), 2);
        let purged = shim.purge_expired();
        assert_eq!(purged, 1);
        assert_eq!(shim.archive_count(), 1);
    }

    #[test]
    fn test_apply_lifecycle() {
        let mut shim = ArchivalShim {
            lifecycle_days: 10,
            ..ArchivalShim::new()
        };

        let mut record = ArchivedRecord {
            id: "test".to_string(),
            table: "test".to_string(),
            original_size_bytes: 100,
            archived_size_bytes: 50,
            archive_path: "path".to_string(),
            archived_at: (chrono::Utc::now() - chrono::Duration::days(20)).to_rfc3339(),
            storage_tier: StorageTier::Hot,
            retention_until: (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
            compressed: false,
        };
        shim.archive_log.push(record);

        let transitioned = shim.apply_lifecycle();
        assert!(transitioned > 0);
        assert_eq!(shim.archive_log[0].storage_tier, StorageTier::Cold);
    }

    #[test]
    fn test_summary() {
        let mut shim = ArchivalShim {
            compression: "zstd".to_string(),
            ..ArchivalShim::new()
        };
        shim.archive_batch("orders", 10, 1000);
        shim.archive_batch("users", 5, 500);
        shim.archive_batch("orders", 3, 300);

        let summary = shim.summary();
        assert_eq!(summary.total_records, 3);
        assert_eq!(*summary.tables_archived.get("orders").unwrap_or(&0), 2);
        assert!(summary.compression_ratio > 0.0 && summary.compression_ratio < 1.0);
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = ArchivalShim::new();
        shim.archive_batch("test", 50, 5_000_000);

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].value, 50.0);
        assert_eq!(metrics[3].value, 1.0);
    }
}
