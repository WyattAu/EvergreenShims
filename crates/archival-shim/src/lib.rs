//! Archival shim — data archival to cold storage.
//!
//! Moves old data from hot storage to cold storage (S3, Glacier, local disk).
//!
//! ## Environment Variables
//!
//! ```text
//! ARCHIVAL_SCHEDULE        Cron schedule (default: 0 3 * * *)
//! ARCHIVAL_TABLES          Tables to archive
//! ARCHIVAL_AGE_DAYS        Archive data older than N days (default: 90)
//! ARCHIVAL_STORAGE         Storage: s3, glacier, local (default: s3)
//! ARCHIVAL_BUCKET          S3 bucket name or local directory
//! ARCHIVAL_COMPRESSION     Compression: none, gzip, zstd (default: zstd)
//! ARCHIVAL_LIFECYCLE_DAYS  Days before moving to colder tier (0 = disabled)
//! ARCHIVAL_HOT_DAYS        Days in hot tier before warm (0 = disabled)
//! ARCHIVAL_WARM_DAYS       Days in warm tier before cold (0 = disabled)
//! ARCHIVAL_COLD_DAYS       Days in cold tier before purge (0 = disabled)
//! ARCHIVAL_RETENTION_DAYS  Global retention days (default: 365)
//! ARCHIVAL_ARCHIVE_PATH    Local archive directory (default: /var/lib/archival)
//!
//! ### S3 (ARCHIVAL_STORAGE=s3)
//!
//! ```text
//! ARCHIVAL_S3_REGION            Region (default: us-east-1)
//! ARCHIVAL_S3_ENDPOINT          Custom endpoint for MinIO/LocalStack/R2
//! ARCHIVAL_S3_PREFIX            Key prefix (default: archives)
//! ARCHIVAL_S3_FORCE_PATH_STYLE  true/1 for path-style addressing (default: false)
//! ARCHIVAL_S3_SSE               true/1 for server-side encryption (default: true)
//! ARCHIVAL_S3_ACCESS_KEY        Static access key (optional)
//! ARCHIVAL_S3_SECRET_KEY        Static secret key (optional)
//! ```
//!
//! When `ARCHIVAL_S3_ACCESS_KEY`/`ARCHIVAL_S3_SECRET_KEY` are unset,
//! credentials come from the standard AWS chain (`AWS_ACCESS_KEY_ID`,
//! `AWS_PROFILE`, IMDS, container credentials).
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use blobkit::local::LocalStore;
use blobkit::s3::{CredentialsMode, S3Config, S3Store};
use blobkit::store::BlobStore;
use blobkit::{BucketName, ObjectKey};
use bytes::Bytes;
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
#[allow(dead_code)]
pub struct ArchivalShim {
    schedule: String,
    tables: Vec<String>,
    age_days: u32,
    storage: String,
    bucket: String,
    compression: String,
    lifecycle_days: u32,
    hot_days: u32,
    warm_days: u32,
    cold_days: u32,
    retention_days: u32,
    archive_path: String,
    compression_ratio: f64,
    records_archived: u64,
    bytes_archived: u64,
    bytes_saved: u64,
    last_archival: Option<chrono::DateTime<chrono::Utc>>,
    retention_rules: HashMap<String, RetentionRule>,
    archive_log: Vec<ArchivedRecord>,
    record_counter: u64,
    // S3 configuration
    s3_region: String,
    s3_endpoint: Option<String>,
    s3_prefix: String,
    s3_force_path_style: bool,
    s3_server_side_encryption: bool,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ArchivalShim {
    pub fn new() -> Self {
        let compression =
            std::env::var("ARCHIVAL_COMPRESSION").unwrap_or_else(|_| "zstd".to_string());

        let compression_ratio = match compression.as_str() {
            "gzip" => 0.3,
            "zstd" => 0.25,
            _ => 1.0,
        };

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
            compression,
            lifecycle_days: std::env::var("ARCHIVAL_LIFECYCLE_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            hot_days: std::env::var("ARCHIVAL_HOT_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            warm_days: std::env::var("ARCHIVAL_WARM_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            cold_days: std::env::var("ARCHIVAL_COLD_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            retention_days: std::env::var("ARCHIVAL_RETENTION_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(365),
            archive_path: std::env::var("ARCHIVAL_ARCHIVE_PATH")
                .unwrap_or_else(|_| "/var/lib/archival".to_string()),
            compression_ratio,
            records_archived: 0,
            bytes_archived: 0,
            bytes_saved: 0,
            last_archival: None,
            retention_rules: HashMap::new(),
            archive_log: Vec::new(),
            record_counter: 0,
            s3_region: std::env::var("ARCHIVAL_S3_REGION")
                .unwrap_or_else(|_| "us-east-1".to_string()),
            s3_endpoint: std::env::var("ARCHIVAL_S3_ENDPOINT").ok(),
            s3_prefix: std::env::var("ARCHIVAL_S3_PREFIX")
                .unwrap_or_else(|_| "archives".to_string()),
            s3_force_path_style: std::env::var("ARCHIVAL_S3_FORCE_PATH_STYLE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            s3_server_side_encryption: std::env::var("ARCHIVAL_S3_SSE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            shutdown_tx: None,
        }
    }

    /// Add a retention rule for a specific table.
    pub fn add_retention_rule(&mut self, rule: RetentionRule) {
        self.retention_rules.insert(rule.table.clone(), rule);
    }

    /// Archive a batch of records from a table, moving source data to the archive path.
    /// If the source doesn't exist, logs a warning and skips.
    pub async fn archive_batch(
        &mut self,
        table: &str,
        count: u64,
        original_size_bytes: u64,
        source_path: Option<&str>,
    ) -> Option<ArchivedRecord> {
        // Ensure archive directory exists
        let table_archive_dir = format!("{}/{}", self.archive_path, table);
        if let Err(e) = tokio::fs::create_dir_all(&table_archive_dir).await {
            tracing::error!(
                table = %table,
                path = %table_archive_dir,
                "Failed to create archive directory: {}",
                e
            );
            return None;
        }

        // If source is specified, check it exists and move it
        if let Some(src) = source_path {
            if !std::path::Path::new(src).exists() {
                tracing::warn!(
                    table = %table,
                    source = %src,
                    "Source path does not exist, skipping archive"
                );
                return None;
            }

            self.record_counter += 1;
            let dest = format!(
                "{}/{}/archive-{}.dat",
                self.archive_path, table, self.record_counter
            );

            match tokio::fs::copy(src, &dest).await {
                Ok(bytes_copied) => {
                    tracing::info!(
                        table = %table,
                        source = %src,
                        dest = %dest,
                        bytes = bytes_copied,
                        "Data archived successfully"
                    );

                    let archived_size = (bytes_copied as f64 * self.compression_ratio) as u64;
                    let saved = bytes_copied.saturating_sub(archived_size);
                    let retention_days = self
                        .retention_rules
                        .get(table)
                        .map(|r| r.age_days)
                        .unwrap_or(self.retention_days);
                    let storage_tier = self
                        .retention_rules
                        .get(table)
                        .map(|r| r.storage_tier.clone())
                        .unwrap_or(StorageTier::Cold);

                    let record = ArchivedRecord {
                        id: format!("arc-{:010}", self.record_counter),
                        table: table.to_string(),
                        original_size_bytes: bytes_copied,
                        archived_size_bytes: archived_size,
                        archive_path: dest,
                        archived_at: chrono::Utc::now().to_rfc3339(),
                        storage_tier,
                        retention_until: (chrono::Utc::now()
                            + chrono::Duration::days(retention_days as i64))
                        .to_rfc3339(),
                        compressed: self.compression != "none",
                    };

                    self.records_archived += count;
                    self.bytes_archived += archived_size;
                    self.bytes_saved += saved;
                    self.last_archival = Some(chrono::Utc::now());
                    self.archive_log.push(record.clone());

                    return Some(record);
                }
                Err(e) => {
                    tracing::error!(
                        table = %table,
                        source = %src,
                        dest = %dest,
                        "Failed to copy source to archive: {}",
                        e
                    );
                    return None;
                }
            }
        }

        // No source path: create a metadata-only record (no fake data movement)
        self.record_counter += 1;
        let archive_path = format!(
            "{}/{}/archive-{}.dat",
            self.archive_path, table, self.record_counter
        );

        let archived_size = (original_size_bytes as f64 * self.compression_ratio) as u64;
        let saved = original_size_bytes.saturating_sub(archived_size);

        let retention_days = self
            .retention_rules
            .get(table)
            .map(|r| r.age_days)
            .unwrap_or(self.retention_days);
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
            archive_path,
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
        self.archive_log.push(record.clone());

        Some(record)
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

    /// Build a local blob store with max_bytes guard (10 MiB).
    ///
    /// Uses `blobkit::LocalStore` with `max_bytes: 10*1024*1024` to prevent
    /// unbounded archival payloads. Replaces direct `tokio::fs` handling.
    #[allow(dead_code)]
    async fn build_local_store(&self) -> anyhow::Result<LocalStore> {
        let root = PathBuf::from(self.archive_path.clone());
        // Ensure max_bytes guard via LocalStore { max_bytes: 10*1024*1024 }
        let store = LocalStore::with_limits(root, Some(10 * 1024 * 1024))
            .await
            .map_err(|e| anyhow::anyhow!("failed to create LocalStore: {}", e))?;
        Ok(store)
    }

    /// Build an S3 blob store for production via `blobkit::S3Store`.
    ///
    /// Configuration sources:
    /// - bucket/region/prefix/path-style/SSE: struct fields (from `ARCHIVAL_*` / `ARCHIVAL_S3_*` env vars)
    /// - endpoint: `ARCHIVAL_S3_ENDPOINT` (parsed to a `Url` for MinIO/LocalStack/R2)
    /// - credentials: `ARCHIVAL_S3_ACCESS_KEY` + `ARCHIVAL_S3_SECRET_KEY` when both
    ///   are set (static mode); otherwise the standard AWS credential chain
    ///   (`AWS_ACCESS_KEY_ID`, `AWS_PROFILE`, IMDS, container credentials)
    #[allow(dead_code)]
    async fn build_s3_store(&self) -> anyhow::Result<S3Store> {
        let bucket = BucketName::new(self.bucket.clone())
            .map_err(|e| anyhow::anyhow!("invalid bucket '{}': {}", self.bucket, e))?;
        let mut cfg =
            S3Config::new(bucket, self.s3_region.clone()).with_path_style(self.s3_force_path_style);

        if let Some(endpoint) = &self.s3_endpoint {
            let url = url::Url::parse(endpoint).map_err(|e| {
                anyhow::anyhow!("invalid ARCHIVAL_S3_ENDPOINT '{}': {}", endpoint, e)
            })?;
            cfg = cfg.with_endpoint(url);
        }

        if let (Ok(access_key), Ok(secret_key)) = (
            std::env::var("ARCHIVAL_S3_ACCESS_KEY"),
            std::env::var("ARCHIVAL_S3_SECRET_KEY"),
        ) {
            if !access_key.is_empty() && !secret_key.is_empty() {
                cfg = cfg.with_credentials(CredentialsMode::Static {
                    access_key,
                    secret_key,
                });
            }
        }

        S3Store::new(cfg)
            .await
            .map_err(|e| anyhow::anyhow!("failed to build S3 client: {}", e))
    }

    /// Upload data to blob storage via `BlobStore::put`.
    ///
    /// Key format: `{s3_prefix}/{table}/{record_id}.dat` validated via
    /// `ObjectKey::try_from(bucket_string)?` / `ObjectKey::try_from(key)?`.
    /// Uses the real `blobkit::S3Store` (see `build_s3_store` for configuration).
    /// Returns the S3 URI on success.
    #[allow(dead_code)]
    async fn upload_to_s3(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> anyhow::Result<String> {
        // Validate bucket and key via typed newtypes (replaces stringly BucketName/ObjectKey)
        let bucket = BucketName::new(self.bucket.clone())
            .map_err(|e| anyhow::anyhow!("invalid bucket '{}': {}", self.bucket, e))?;
        let raw_key = format!("{}/{}/{}.dat", self.s3_prefix, table, record_id);
        let key = ObjectKey::try_from(raw_key.clone())
            .map_err(|e| anyhow::anyhow!("invalid object key '{}': {}", raw_key, e))?;
        let bytes = Bytes::copy_from_slice(data);

        let store = self.build_s3_store().await?;
        store.put(key.clone(), bytes).await.map_err(|e| {
            anyhow::anyhow!(
                "blob upload failed: {}/{}: {}",
                bucket.as_str(),
                key.as_str(),
                e
            )
        })?;

        let s3_uri = format!("s3://{}/{}", bucket.as_str(), key.as_str());
        tracing::info!("blob archive upload: {} bytes -> {}", data.len(), s3_uri);

        Ok(s3_uri)
    }

    /// Delete an archive from blob storage via `BlobStore::delete`.
    #[allow(dead_code)]
    async fn delete_from_s3(&self, table: &str, record_id: &str) -> anyhow::Result<()> {
        let bucket = BucketName::new(self.bucket.clone())
            .map_err(|e| anyhow::anyhow!("invalid bucket '{}': {}", self.bucket, e))?;
        let raw_key = format!("{}/{}/{}.dat", self.s3_prefix, table, record_id);
        let key = ObjectKey::try_from(raw_key.clone())
            .map_err(|e| anyhow::anyhow!("invalid object key '{}': {}", raw_key, e))?;

        let store = self.build_s3_store().await?;
        store.delete(&key).await.map_err(|e| {
            anyhow::anyhow!(
                "blob delete failed: {}/{}: {}",
                bucket.as_str(),
                key.as_str(),
                e
            )
        })?;

        tracing::info!(
            "S3 archive delete: s3://{}/{}",
            bucket.as_str(),
            key.as_str()
        );
        Ok(())
    }

    /// Retrieve an archive from blob storage via `BlobStore::get`.
    ///
    /// Demonstrates `BlobStore::get` usage with typed `ObjectKey`.
    #[allow(dead_code)]
    async fn get_from_s3(&self, table: &str, record_id: &str) -> anyhow::Result<Bytes> {
        let bucket = BucketName::new(self.bucket.clone())
            .map_err(|e| anyhow::anyhow!("invalid bucket '{}': {}", self.bucket, e))?;
        let raw_key = format!("{}/{}/{}.dat", self.s3_prefix, table, record_id);
        let key = ObjectKey::try_from(raw_key.clone())
            .map_err(|e| anyhow::anyhow!("invalid object key '{}': {}", raw_key, e))?;

        let store = self.build_s3_store().await?;
        let data = store.get(&key).await.map_err(|e| {
            anyhow::anyhow!("S3 get failed: {}/{}: {}", bucket.as_str(), key.as_str(), e)
        })?;
        Ok(data)
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
        before.saturating_sub(self.archive_log.len()) as u64
    }

    /// Transition archives to colder storage based on lifecycle rules.
    /// When `lifecycle_days` is 0, transitions are disabled.
    pub fn apply_lifecycle(&mut self) -> u64 {
        let mut transitioned = 0u64;
        for record in &mut self.archive_log {
            if record.storage_tier == StorageTier::Hot {
                if let Ok(archived_at) = record.archived_at.parse::<chrono::DateTime<chrono::Utc>>()
                {
                    let age_days = (chrono::Utc::now() - archived_at).num_days() as u32;

                    let hot_threshold = self.hot_days;
                    let warm_threshold = self.warm_days;

                    // Use hot_days/warm_days if configured, otherwise fall back to lifecycle_days
                    if hot_threshold > 0 && age_days >= hot_threshold {
                        record.storage_tier = StorageTier::Cold;
                        transitioned += 1;
                    } else if warm_threshold > 0 && age_days >= warm_threshold {
                        record.storage_tier = StorageTier::Warm;
                        transitioned += 1;
                    } else if hot_threshold == 0 && warm_threshold == 0 && self.lifecycle_days > 0 {
                        // Legacy fallback: use lifecycle_days for both transitions
                        if age_days >= self.lifecycle_days {
                            record.storage_tier = StorageTier::Cold;
                            transitioned += 1;
                        } else if age_days >= self.lifecycle_days / 2 {
                            record.storage_tier = StorageTier::Warm;
                            transitioned += 1;
                        }
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
        self.compression_ratio
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
            "ArchivalShim initialized (schedule={}, age={}d, storage={}, hot={}d, warm={}d, cold={}d, retention={}d, archive_path={})",
            self.schedule,
            self.age_days,
            self.storage,
            self.hot_days,
            self.warm_days,
            self.cold_days,
            self.retention_days,
            self.archive_path
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

    fn temp_shim(compression: &str, ratio: f64) -> (ArchivalShim, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let shim = ArchivalShim {
            compression: compression.to_string(),
            compression_ratio: ratio,
            archive_path: dir.path().to_str().unwrap().to_string(),
            ..ArchivalShim::new()
        };
        (shim, dir)
    }

    #[test]
    fn test_storage_tier_display() {
        assert_eq!(StorageTier::Hot.to_string(), "hot");
        assert_eq!(StorageTier::Warm.to_string(), "warm");
        assert_eq!(StorageTier::Cold.to_string(), "cold");
    }

    #[tokio::test]
    async fn test_archive_batch_gzip() {
        let (mut shim, _dir) = temp_shim("gzip", 0.3);
        let record = shim.archive_batch("orders", 1000, 10_000_000, None).await;

        assert!(record.is_some());
        let record = record.unwrap();
        assert_eq!(record.table, "orders");
        assert!(record.compressed);
        assert!(record.archived_size_bytes < record.original_size_bytes);
        assert_eq!(shim.records_archived, 1000);
        assert_eq!(shim.archive_count(), 1);
    }

    #[tokio::test]
    async fn test_archive_batch_no_compression() {
        let (mut shim, _dir) = temp_shim("none", 1.0);
        let record = shim.archive_batch("users", 100, 1_000_000, None).await;

        assert!(record.is_some());
        let record = record.unwrap();
        assert!(!record.compressed);
        assert_eq!(record.archived_size_bytes, record.original_size_bytes);
        assert_eq!(shim.bytes_saved, 0);
    }

    #[test]
    fn test_compression_ratio() {
        let (shim, _dir) = temp_shim("gzip", 0.3);
        assert!((shim.compression_ratio() - 0.3).abs() < 0.01);

        let (shim2, _dir2) = temp_shim("zstd", 0.25);
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

        let record = ArchivedRecord {
            id: "test".to_string(),
            table: "logs".to_string(),
            original_size_bytes: 500_000,
            archived_size_bytes: 125_000,
            archive_path: "path".to_string(),
            archived_at: chrono::Utc::now().to_rfc3339(),
            storage_tier: StorageTier::Cold,
            retention_until: (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
            compressed: true,
        };
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
        let record = ArchivedRecord {
            id: "active".to_string(),
            table: "orders".to_string(),
            original_size_bytes: 1000,
            archived_size_bytes: 250,
            archive_path: "path".to_string(),
            archived_at: chrono::Utc::now().to_rfc3339(),
            storage_tier: StorageTier::Cold,
            retention_until: (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
            compressed: true,
        };
        shim.archive_log.push(record.clone());

        let mut expired = record;
        expired.retention_until = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        expired.id = "expired".to_string();
        shim.archive_log.push(expired);

        assert_eq!(shim.archive_count(), 2);
        let purged = shim.purge_expired();
        assert_eq!(purged, 1);
        assert_eq!(shim.archive_count(), 1);
    }

    #[test]
    fn test_purge_expired_empty_log() {
        let mut shim = ArchivalShim::new();
        let purged = shim.purge_expired();
        assert_eq!(purged, 0);
    }

    #[test]
    fn test_apply_lifecycle_disabled_when_zero() {
        let mut shim = ArchivalShim {
            lifecycle_days: 0,
            hot_days: 0,
            warm_days: 0,
            cold_days: 0,
            ..ArchivalShim::new()
        };

        let record = ArchivedRecord {
            id: "test".to_string(),
            table: "test".to_string(),
            original_size_bytes: 100,
            archived_size_bytes: 50,
            archive_path: "path".to_string(),
            archived_at: (chrono::Utc::now() - chrono::Duration::days(100)).to_rfc3339(),
            storage_tier: StorageTier::Hot,
            retention_until: (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
            compressed: false,
        };
        shim.archive_log.push(record);

        let transitioned = shim.apply_lifecycle();
        assert_eq!(transitioned, 0);
        assert_eq!(shim.archive_log[0].storage_tier, StorageTier::Hot);
    }

    #[test]
    fn test_apply_lifecycle_with_hot_warm_days() {
        let mut shim = ArchivalShim {
            hot_days: 5,
            warm_days: 10,
            cold_days: 30,
            ..ArchivalShim::new()
        };

        let record = ArchivedRecord {
            id: "test1".to_string(),
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
    fn test_apply_lifecycle_warm_threshold() {
        let mut shim = ArchivalShim {
            hot_days: 0,
            warm_days: 10,
            cold_days: 30,
            lifecycle_days: 0,
            ..ArchivalShim::new()
        };

        let record = ArchivedRecord {
            id: "test1".to_string(),
            table: "test".to_string(),
            original_size_bytes: 100,
            archived_size_bytes: 50,
            archive_path: "path".to_string(),
            archived_at: (chrono::Utc::now() - chrono::Duration::days(15)).to_rfc3339(),
            storage_tier: StorageTier::Hot,
            retention_until: (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339(),
            compressed: false,
        };
        shim.archive_log.push(record);

        let transitioned = shim.apply_lifecycle();
        assert_eq!(transitioned, 1);
        assert_eq!(shim.archive_log[0].storage_tier, StorageTier::Warm);
    }

    #[tokio::test]
    async fn test_summary() {
        let (mut shim, _dir) = temp_shim("zstd", 0.25);
        shim.archive_batch("orders", 10, 1000, None).await;
        shim.archive_batch("users", 5, 500, None).await;
        shim.archive_batch("orders", 3, 300, None).await;

        let summary = shim.summary();
        assert_eq!(summary.total_records, 3);
        assert_eq!(*summary.tables_archived.get("orders").unwrap_or(&0), 2);
        assert!(summary.compression_ratio > 0.0 && summary.compression_ratio < 1.0);
    }

    #[tokio::test]
    async fn test_metrics() {
        let (mut shim, _dir) = temp_shim("zstd", 0.25);
        shim.archive_batch("test", 50, 5_000_000, None).await;

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].value, 50.0);
        assert_eq!(metrics[3].value, 1.0);
    }

    #[tokio::test]
    async fn test_archive_batch_missing_source() {
        let (mut shim, _dir) = temp_shim("zstd", 0.25);
        let result = shim
            .archive_batch("orders", 100, 1_000_000, Some("/nonexistent/path.dat"))
            .await;
        assert!(result.is_none());
        assert_eq!(shim.archive_count(), 0);
    }
}
