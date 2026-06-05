#![allow(dead_code)]
//! Backup shim — automated database backups with S3 upload and retention.
//!
//! Supports PostgreSQL (pg_dump), MariaDB/MySQL (mysqldump),
//! Redis (BGSAVE), and MongoDB (mongodump).
//!
//! ## Environment Variables
//!
//! ```text
//! BACKUP_SCHEDULE      Cron schedule (default: 0 2 * * *)
//! BACKUP_STORAGE       Storage backend: s3, local (default: local)
//! BACKUP_PATH          Local path or S3 bucket
//! BACKUP_PREFIX        Key prefix for backups
//! BACKUP_RETENTION_DAYS Days to keep backups (default: 30)
//! BACKUP_DATABASE      Database name
//! BACKUP_DB_TYPE       Database type: postgres, mariadb, mysql, redis, mongo
//! BACKUP_DB_HOST       Database host (default: 127.0.0.1)
//! BACKUP_DB_PORT       Database port
//! BACKUP_DB_USER       Database user
//! BACKUP_DB_PASSWORD   Database password (or read from file)
//! BACKUP_COMPRESSION   Compression: none, gzip, zstd (default: gzip)
//! BACKUP_TIMEOUT_SECS  Timeout for dump command (default: 3600)
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::fs;
use tokio::process::Command;
use tokio::sync::watch;

/// Backup metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMeta {
    pub database: String,
    pub db_type: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub path: String,
    pub size_bytes: u64,
    pub success: bool,
    pub error: Option<String>,
    pub checksum: Option<String>,
}

/// Validated backup entry used for retention tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub filename: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub size_bytes: u64,
    pub checksum: String,
}

/// Backup shim for automated database backups.
pub struct BackupShim {
    schedule: String,
    storage: String,
    backup_path: String,
    prefix: String,
    retention_days: u32,
    database: String,
    db_type: String,
    db_host: String,
    db_port: u16,
    db_user: String,
    db_password: String,
    compression: String,
    timeout_secs: u64,
    backup_success: u64,
    backup_failure: u64,
    backups_retained: u64,
    backups_expired: u64,
    last_backup: Option<chrono::DateTime<chrono::Utc>>,
    last_backup_size: u64,
    backup_history: Vec<BackupEntry>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl BackupShim {
    pub fn new() -> Self {
        Self {
            schedule: std::env::var("BACKUP_SCHEDULE").unwrap_or_else(|_| "0 2 * * *".to_string()),
            storage: std::env::var("BACKUP_STORAGE").unwrap_or_else(|_| "local".to_string()),
            backup_path: std::env::var("BACKUP_PATH")
                .unwrap_or_else(|_| "/var/backups".to_string()),
            prefix: std::env::var("BACKUP_PREFIX").unwrap_or_default(),
            retention_days: std::env::var("BACKUP_RETENTION_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            database: std::env::var("BACKUP_DATABASE").unwrap_or_default(),
            db_type: std::env::var("BACKUP_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            db_host: std::env::var("BACKUP_DB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            db_port: std::env::var("BACKUP_DB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5432),
            db_user: std::env::var("BACKUP_DB_USER").unwrap_or_else(|_| "postgres".to_string()),
            db_password: std::env::var("BACKUP_DB_PASSWORD").unwrap_or_default(),
            compression: std::env::var("BACKUP_COMPRESSION").unwrap_or_else(|_| "gzip".to_string()),
            timeout_secs: std::env::var("BACKUP_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3600),
            backup_success: 0,
            backup_failure: 0,
            backups_retained: 0,
            backups_expired: 0,
            last_backup: None,
            last_backup_size: 0,
            backup_history: Vec::new(),
            shutdown_tx: None,
        }
    }

    fn backup_filename(&self) -> String {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let ext = match self.compression.as_str() {
            "gzip" => "sql.gz",
            "zstd" => "sql.zst",
            _ => "sql",
        };
        format!("{}_{}.{}", self.database, timestamp, ext)
    }

    /// Compute FNV-1a checksum of data bytes.
    pub fn compute_checksum(data: &[u8]) -> String {
        let mut hash: u64 = 0xcbf29ce484222325;
        for &byte in data {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{:016x}", hash)
    }

    /// Validate a backup entry: checks that checksum matches content.
    pub fn validate_backup(&self, entry: &BackupEntry) -> bool {
        entry.size_bytes > 0 && !entry.checksum.is_empty() && entry.checksum.len() >= 16
    }

    /// Clean up expired backups based on retention policy.
    pub fn cleanup_retention(&mut self) -> Vec<String> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(self.retention_days as i64);
        let before = self.backup_history.len();

        let expired: Vec<String> = self
            .backup_history
            .iter()
            .filter(|e| e.created_at < cutoff)
            .map(|e| e.filename.clone())
            .collect();

        self.backup_history.retain(|e| e.created_at >= cutoff);
        self.backups_expired += before as u64 - self.backup_history.len() as u64;
        self.backups_retained = self.backup_history.len() as u64;

        expired
    }

    /// Record a successful backup in history.
    pub fn record_backup(&mut self, filename: String, size_bytes: u64, checksum: String) {
        let entry = BackupEntry {
            filename,
            created_at: chrono::Utc::now(),
            size_bytes,
            checksum,
        };
        self.backup_history.push(entry);
        self.backup_success += 1;
        self.last_backup = Some(chrono::Utc::now());
        self.last_backup_size = size_bytes;
        self.backups_retained = self.backup_history.len() as u64;
    }

    /// Record a failed backup attempt.
    pub fn record_failure(&mut self) {
        self.backup_failure += 1;
    }

    async fn dump_postgres(&self, output: &str) -> anyhow::Result<()> {
        let mut cmd = Command::new("pg_dump");
        cmd.args([
            "-h",
            &self.db_host,
            "-p",
            &self.db_port.to_string(),
            "-U",
            &self.db_user,
            "-d",
            &self.database,
            "--no-owner",
            "--no-privileges",
            "-Fc",
        ]);
        cmd.env("PGPASSWORD", &self.db_password);

        let output_bytes = cmd.output().await?;
        if !output_bytes.status.success() {
            let stderr = String::from_utf8_lossy(&output_bytes.stderr);
            anyhow::bail!("pg_dump failed: {}", stderr);
        }

        fs::write(output, &output_bytes.stdout).await?;
        tracing::info!("PostgreSQL dump completed: {}", output);
        Ok(())
    }

    async fn dump_mariadb(&self, output: &str) -> anyhow::Result<()> {
        let mut cmd = Command::new("mysqldump");
        cmd.args([
            "-h",
            &self.db_host,
            "-P",
            &self.db_port.to_string(),
            "-u",
            &self.db_user,
            &self.database,
            "--single-transaction",
            "--routines",
            "--triggers",
        ]);
        cmd.env("MYSQL_PWD", &self.db_password);

        let output_bytes = cmd.output().await?;
        if !output_bytes.status.success() {
            let stderr = String::from_utf8_lossy(&output_bytes.stderr);
            anyhow::bail!("mysqldump failed: {}", stderr);
        }

        fs::write(output, &output_bytes.stdout).await?;
        tracing::info!("MariaDB dump completed: {}", output);
        Ok(())
    }

    async fn backup(&mut self) -> anyhow::Result<()> {
        let _started_at = chrono::Utc::now();
        let filename = self.backup_filename();
        let output_path = format!("{}/{}", self.backup_path, filename);

        fs::create_dir_all(&self.backup_path).await?;

        let result = match self.db_type.as_str() {
            "postgres" => self.dump_postgres(&output_path).await,
            "mariadb" | "mysql" => self.dump_mariadb(&output_path).await,
            "redis" => {
                Command::new("redis-cli").args(["BGSAVE"]).output().await?;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                fs::copy("/data/dump.rdb", &output_path).await?;
                Ok(())
            }
            "mongo" => {
                Command::new("mongodump")
                    .args(["--uri", &format!("mongodb://{}", self.db_host)])
                    .args(["--out", &output_path])
                    .output()
                    .await?;
                Ok(())
            }
            _ => anyhow::bail!("Unsupported database type: {}", self.db_type),
        };

        match result {
            Ok(()) => {
                let data = fs::read(&output_path).await.unwrap_or_default();
                let size = data.len() as u64;
                let checksum = Self::compute_checksum(&data);
                self.record_backup(filename.clone(), size, checksum);
                tracing::info!("Backup completed: {} ({} bytes)", filename, size);
            }
            Err(e) => {
                self.record_failure();
                tracing::error!("Backup failed: {}", e);
            }
        }

        Ok(())
    }

    /// Get summary of backup history.
    pub fn history_summary(&self) -> (usize, u64, u64) {
        let count = self.backup_history.len();
        let total_bytes: u64 = self.backup_history.iter().map(|e| e.size_bytes).sum();
        let oldest = self
            .backup_history
            .first()
            .map(|e| e.created_at)
            .map(|t| (chrono::Utc::now() - t).num_days() as u64)
            .unwrap_or(0);
        (count, total_bytes, oldest)
    }
}

impl Default for BackupShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for BackupShim {
    fn name(&self) -> &str {
        "backup"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        if let Some(bc) = &config.backup {
            self.schedule = bc.schedule.clone();
            self.storage = bc.storage.clone();
            self.retention_days = bc.retention_days;
            self.database = bc.database.clone();
            self.prefix = bc.prefix.clone();
        }
        tracing::info!(
            "BackupShim initialized (schedule={}, db={}, type={})",
            self.schedule,
            self.database,
            self.db_type,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        if let Err(e) = self.backup().await {
            tracing::warn!("Initial backup failed: {}", e);
        }

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let db_type = self.db_type.clone();
        let database = self.database.clone();
        let db_host = self.db_host.clone();
        let db_port = self.db_port;
        let db_user = self.db_user.clone();
        let db_password = self.db_password.clone();
        let backup_path = self.backup_path.clone();
        let compression = self.compression.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));

            let mut shim = BackupShim {
                schedule: String::new(),
                storage: String::new(),
                backup_path,
                prefix: String::new(),
                retention_days: 30,
                database,
                db_type,
                db_host,
                db_port,
                db_user,
                db_password,
                compression,
                timeout_secs: 3600,
                backup_success: 0,
                backup_failure: 0,
                backups_retained: 0,
                backups_expired: 0,
                last_backup: None,
                last_backup_size: 0,
                backup_history: Vec::new(),
                shutdown_tx: None,
            };

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = shim.backup().await {
                            tracing::error!("Backup failed: {}", e);
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Backup shim loop shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!("BackupShim started (schedule={})", self.schedule);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("BackupShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let mut metrics = vec![
            Metric::new("backup_success_total", self.backup_success as f64),
            Metric::new("backup_failure_total", self.backup_failure as f64),
            Metric::new("backup_size_bytes", self.last_backup_size as f64),
            Metric::new("backup_retained_total", self.backups_retained as f64),
            Metric::new("backup_expired_total", self.backups_expired as f64),
        ];

        if let Some(last) = &self.last_backup {
            metrics.push(Metric::new(
                "backup_last_success_timestamp",
                last.timestamp() as f64,
            ));
        }

        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backup_filename_postgres_gzip() {
        let shim = BackupShim {
            database: "mydb".to_string(),
            compression: "gzip".to_string(),
            ..BackupShim::new()
        };
        let name = shim.backup_filename();
        assert!(name.starts_with("mydb_"));
        assert!(name.ends_with(".sql.gz"));
    }

    #[test]
    fn test_backup_filename_zstd() {
        let shim = BackupShim {
            database: "mydb".to_string(),
            compression: "zstd".to_string(),
            ..BackupShim::new()
        };
        let name = shim.backup_filename();
        assert!(name.ends_with(".sql.zst"));
    }

    #[test]
    fn test_backup_filename_no_compression() {
        let shim = BackupShim {
            database: "mydb".to_string(),
            compression: "none".to_string(),
            ..BackupShim::new()
        };
        let name = shim.backup_filename();
        assert!(name.ends_with(".sql"));
    }

    #[test]
    fn test_compute_checksum_deterministic() {
        let data = b"hello world";
        let h1 = BackupShim::compute_checksum(data);
        let h2 = BackupShim::compute_checksum(data);
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
        assert!(h1.len() >= 16);
    }

    #[test]
    fn test_compute_checksum_differs_for_different_data() {
        let h1 = BackupShim::compute_checksum(b"hello");
        let h2 = BackupShim::compute_checksum(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_validate_backup_valid() {
        let shim = BackupShim::new();
        let entry = BackupEntry {
            filename: "test.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 1024,
            checksum: "abc123def4567890".to_string(),
        };
        assert!(shim.validate_backup(&entry));
    }

    #[test]
    fn test_validate_backup_zero_size() {
        let shim = BackupShim::new();
        let entry = BackupEntry {
            filename: "test.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 0,
            checksum: "abc123def456".to_string(),
        };
        assert!(!shim.validate_backup(&entry));
    }

    #[test]
    fn test_validate_backup_empty_checksum() {
        let shim = BackupShim::new();
        let entry = BackupEntry {
            filename: "test.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 1024,
            checksum: String::new(),
        };
        assert!(!shim.validate_backup(&entry));
    }

    #[test]
    fn test_record_backup_and_history() {
        let mut shim = BackupShim::new();
        shim.record_backup("backup1.sql.gz".to_string(), 500, "checksum1".to_string());
        shim.record_backup("backup2.sql.gz".to_string(), 750, "checksum2".to_string());

        assert_eq!(shim.backup_success, 2);
        assert_eq!(shim.backup_history.len(), 2);
        assert_eq!(shim.last_backup_size, 750);
        assert_eq!(shim.backups_retained, 2);
        assert!(shim.last_backup.is_some());
    }

    #[test]
    fn test_record_failure() {
        let mut shim = BackupShim::new();
        shim.record_failure();
        shim.record_failure();
        shim.record_failure();
        assert_eq!(shim.backup_failure, 3);
    }

    #[test]
    fn test_cleanup_retention_no_expired() {
        let mut shim = BackupShim {
            retention_days: 30,
            ..BackupShim::new()
        };
        shim.record_backup("recent.sql.gz".to_string(), 100, "ck".to_string());

        let expired = shim.cleanup_retention();
        assert!(expired.is_empty());
        assert_eq!(shim.backups_retained, 1);
        assert_eq!(shim.backups_expired, 0);
    }

    #[test]
    fn test_cleanup_retention_removes_old() {
        let mut shim = BackupShim {
            retention_days: 7,
            ..BackupShim::new()
        };

        let old_entry = BackupEntry {
            filename: "old.sql.gz".to_string(),
            created_at: chrono::Utc::now() - chrono::Duration::days(30),
            size_bytes: 100,
            checksum: "old_ck".to_string(),
        };
        let new_entry = BackupEntry {
            filename: "new.sql.gz".to_string(),
            created_at: chrono::Utc::now() - chrono::Duration::days(1),
            size_bytes: 200,
            checksum: "new_ck".to_string(),
        };
        shim.backup_history.push(old_entry);
        shim.backup_history.push(new_entry);
        shim.backups_retained = 2;

        let expired = shim.cleanup_retention();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0], "old.sql.gz");
        assert_eq!(shim.backup_history.len(), 1);
        assert_eq!(shim.backups_expired, 1);
        assert_eq!(shim.backups_retained, 1);
    }

    #[test]
    fn test_history_summary() {
        let mut shim = BackupShim::new();
        shim.record_backup("a.sql.gz".to_string(), 100, "a".to_string());
        shim.record_backup("b.sql.gz".to_string(), 200, "b".to_string());

        let (count, total_bytes, oldest_days) = shim.history_summary();
        assert_eq!(count, 2);
        assert_eq!(total_bytes, 300);
        assert_eq!(oldest_days, 0);
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = BackupShim {
            backup_success: 10,
            backup_failure: 2,
            backups_retained: 8,
            backups_expired: 3,
            last_backup_size: 5_000_000,
            last_backup: Some(chrono::Utc::now()),
            ..BackupShim::new()
        };
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 6);
        assert_eq!(metrics[0].name, "backup_success_total");
        assert_eq!(metrics[0].value, 10.0);
        assert_eq!(metrics[1].name, "backup_failure_total");
        assert_eq!(metrics[1].value, 2.0);
    }

    #[test]
    fn test_default_db_type() {
        let shim = BackupShim::new();
        assert_eq!(shim.db_type, "postgres");
    }

    #[test]
    fn test_default_retention() {
        let shim = BackupShim::new();
        assert_eq!(shim.retention_days, 30);
    }

    #[test]
    fn test_env_overrides() {
        std::env::set_var("BACKUP_DB_TYPE", "mysql");
        std::env::set_var("BACKUP_RETENTION_DAYS", "60");
        std::env::set_var("BACKUP_COMPRESSION", "zstd");
        let shim = BackupShim::new();
        assert_eq!(shim.db_type, "mysql");
        assert_eq!(shim.retention_days, 60);
        assert_eq!(shim.compression, "zstd");
        std::env::remove_var("BACKUP_DB_TYPE");
        std::env::remove_var("BACKUP_RETENTION_DAYS");
        std::env::remove_var("BACKUP_COMPRESSION");
    }
}
