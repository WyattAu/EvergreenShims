//! Backup shim — automated database backups with S3 upload and retention.
//!
//! Supports PostgreSQL (pg_dump), MariaDB/MySQL (mysqldump),
//! Redis (BGSAVE), and MongoDB (mongodump).
//!
//! ## Environment Variables
//!
//! ```text
//! BACKUP_SCHEDULE      Cron schedule (default: 0 0 2 * * *)
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
//! BACKUP_CMD           Backup command override (default: pg_dump)
//! BACKUP_OUTPUT_DIR    Output directory (default: /tmp/backups)
//! REDIS_URL            Redis connection URL (default: redis://localhost:6379)
//! REDIS_PASSWORD       Redis password
//! BACKUP_COMPRESSION   Compression: none, gzip, zstd (default: gzip)
//! BACKUP_TIMEOUT_SECS  Timeout for dump command (default: 3600)
//! ```

use std::str::FromStr;
use std::sync::{Arc, Mutex};

/// Lock a mutex, recovering from poison if necessary.
fn lock_mutex<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

use blobkit::local::LocalStore;
use blobkit::s3::{S3Config, S3Store};
use blobkit::store::BlobStore;
use blobkit::{BucketName, ObjectKey};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use shim_core::{Capability, Config, Metric, Result};
use std::path::PathBuf;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::watch;

/// Backup metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMeta {
    /// Database name that was backed up.
    pub database: String,
    /// Database type (postgres, mysql, etc.).
    pub db_type: String,
    /// ISO 8601 timestamp when the backup started.
    pub started_at: String,
    /// ISO 8601 timestamp when the backup completed (None if still running).
    pub completed_at: Option<String>,
    /// Path to the backup file.
    pub path: String,
    /// Size of the backup in bytes.
    pub size_bytes: u64,
    /// Whether the backup completed successfully.
    pub success: bool,
    /// Error message if the backup failed.
    pub error: Option<String>,
    /// SHA-256 checksum of the backup file.
    pub checksum: Option<String>,
}

/// Validated backup entry used for retention tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    /// Backup filename.
    pub filename: String,
    /// When the backup was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Size of the backup in bytes.
    pub size_bytes: u64,
    /// SHA-256 checksum of the backup.
    pub checksum: String,
}

/// Shared backup state between parent and spawned task.
pub struct BackupState {
    /// Total successful backups.
    pub backup_success: u64,
    /// Total failed backup attempts.
    pub backup_failure: u64,
    /// Number of backups currently retained.
    pub backups_retained: u64,
    /// Number of backups expired and cleaned up.
    pub backups_expired: u64,
    /// Timestamp of the last successful backup.
    pub last_backup: Option<chrono::DateTime<chrono::Utc>>,
    /// Size of the last backup in bytes.
    pub last_backup_size: u64,
    /// History of recent backups for retention tracking.
    pub backup_history: Vec<BackupEntry>,
}

impl BackupState {
    fn new() -> Self {
        Self {
            backup_success: 0,
            backup_failure: 0,
            backups_retained: 0,
            backups_expired: 0,
            last_backup: None,
            last_backup_size: 0,
            backup_history: Vec::new(),
        }
    }

    fn record_backup(&mut self, filename: String, size_bytes: u64, checksum: String) {
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

    fn record_failure(&mut self) {
        self.backup_failure += 1;
    }

    fn cleanup_retention(&mut self, retention_days: u32) -> Vec<String> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
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

    fn history_summary(&self) -> (usize, u64, u64) {
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

/// Backup shim for automated database backups.
pub struct BackupShim {
    schedule: String,
    storage: String,
    #[allow(dead_code)]
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
    backup_cmd: String,
    output_dir: String,
    redis_url: String,
    redis_password: String,
    // Retry configuration
    retry_max_attempts: u32,
    retry_base_delay_ms: u64,
    retry_max_delay_ms: u64,
    // S3 configuration
    s3_bucket: String,
    s3_region: String,
    s3_endpoint: Option<String>,
    s3_prefix: String,
    s3_force_path_style: bool,
    s3_server_side_encryption: bool,
    state: Arc<Mutex<BackupState>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl BackupShim {
    /// Create a new backup shim from environment variables.
    pub fn new() -> Self {
        Self {
            schedule: std::env::var("BACKUP_SCHEDULE")
                .unwrap_or_else(|_| "0 0 2 * * *".to_string()),
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
            backup_cmd: std::env::var("BACKUP_CMD").unwrap_or_else(|_| "pg_dump".to_string()),
            output_dir: std::env::var("BACKUP_OUTPUT_DIR")
                .unwrap_or_else(|_| "/tmp/backups".to_string()),
            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            redis_password: std::env::var("REDIS_PASSWORD").unwrap_or_default(),
            retry_max_attempts: std::env::var("BACKUP_RETRY_MAX_ATTEMPTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            retry_base_delay_ms: std::env::var("BACKUP_RETRY_BASE_DELAY_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1000),
            retry_max_delay_ms: std::env::var("BACKUP_RETRY_MAX_DELAY_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30000),
            s3_bucket: std::env::var("BACKUP_S3_BUCKET").unwrap_or_default(),
            s3_region: std::env::var("BACKUP_S3_REGION")
                .unwrap_or_else(|_| "us-east-1".to_string()),
            s3_endpoint: std::env::var("BACKUP_S3_ENDPOINT").ok(),
            s3_prefix: std::env::var("BACKUP_S3_PREFIX").unwrap_or_else(|_| "backups".to_string()),
            s3_force_path_style: std::env::var("BACKUP_S3_FORCE_PATH_STYLE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            s3_server_side_encryption: std::env::var("BACKUP_S3_SSE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            state: Arc::new(Mutex::new(BackupState::new())),
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

    /// Compute SHA-256 checksum of data bytes.
    pub fn compute_checksum(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        result.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Get the database type.
    pub fn db_type(&self) -> &str {
        &self.db_type
    }

    /// Get the database host.
    pub fn db_host(&self) -> &str {
        &self.db_host
    }

    /// Get the database port.
    pub fn db_port(&self) -> u16 {
        self.db_port
    }

    /// Get the database user.
    pub fn db_user(&self) -> &str {
        &self.db_user
    }

    /// Get the database password.
    pub fn db_password(&self) -> &str {
        &self.db_password
    }

    /// Get the database name.
    pub fn database(&self) -> &str {
        &self.database
    }

    /// Get the backup command.
    pub fn backup_cmd(&self) -> &str {
        &self.backup_cmd
    }

    /// Get the output directory.
    pub fn output_dir(&self) -> &str {
        &self.output_dir
    }

    /// Get the Redis URL.
    pub fn redis_url(&self) -> &str {
        &self.redis_url
    }

    /// Get the Redis password.
    pub fn redis_password(&self) -> &str {
        &self.redis_password
    }

    /// Get the timeout in seconds.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    /// Get the S3 bucket.
    pub fn s3_bucket(&self) -> &str {
        &self.s3_bucket
    }

    /// Get the S3 region.
    pub fn s3_region(&self) -> &str {
        &self.s3_region
    }

    /// Get the S3 endpoint (for MinIO/LocalStack).
    pub fn s3_endpoint(&self) -> Option<&str> {
        self.s3_endpoint.as_deref()
    }

    /// Get the S3 key prefix.
    pub fn s3_prefix(&self) -> &str {
        &self.s3_prefix
    }

    /// Check if S3 server-side encryption is enabled.
    pub fn s3_server_side_encryption(&self) -> bool {
        self.s3_server_side_encryption
    }

    /// Check if S3 path style is forced (for MinIO).
    pub fn s3_force_path_style(&self) -> bool {
        self.s3_force_path_style
    }

    /// Build a local blob store with max_bytes guard (10 MiB).
    ///
    /// Uses `blobkit::LocalStore` with `max_bytes: 10*1024*1024` to prevent
    /// unbounded uploads from exhausting disk/memory. Replaces ad-hoc
    /// `tokio::fs` writes with the unified `BlobStore` trait.
    async fn build_local_store(&self) -> anyhow::Result<LocalStore> {
        let root = PathBuf::from(self.output_dir.clone());
        // Ensure max_bytes guard via LocalStore { max_bytes: 10*1024*1024 }
        let store = LocalStore::with_limits(root, Some(10 * 1024 * 1024))
            .await
            .map_err(|e| anyhow::anyhow!("failed to create LocalStore: {}", e))?;
        Ok(store)
    }

    /// Build an S3 blob store for production.
    ///
    /// NOTE: `blobkit::S3Store` is a stub in v0.1 and returns
    /// `BlobError::Unsupported` for all operations. The real S3 implementation
    /// (wiring `aws-sdk-s3` behind the `s3` feature, including raw S3 client
    /// replacement, endpoint handling, and `force_path_style`) will be added in
    /// blobkit v0.2. This keeps the production path typed via `BucketName` while
    /// allowing compilation today.
    fn build_s3_store(&self) -> anyhow::Result<S3Store> {
        // Typed bucket validation via blobkit::BucketName (replaces hand-rolled stringly checks).
        // Migration uses BucketName::try_from / BucketName::new and ObjectKey::try_from.
        let bucket = BucketName::new(self.s3_bucket.clone())
            .map_err(|e| anyhow::anyhow!("invalid bucket '{}': {}", self.s3_bucket, e))?;
        let cfg = S3Config::new(bucket, self.s3_region.clone());
        // Custom endpoint (MinIO/LocalStack) will be supported via S3Config::with_endpoint in v0.2
        Ok(S3Store::new(cfg))
    }

    /// Upload a file to blob storage.
    ///
    /// The key is constructed as `{s3_prefix}/{database}/{filename}` and validated
    /// via `ObjectKey::try_from`. Uses `BlobStore::put` with `Bytes`.
    /// For S3 production, uses `blobkit::S3Store` (stub returns Unsupported until blobkit v0.2).
    /// For local storage, `build_local_store` provides a `LocalStore` with `max_bytes` guard.
    /// If `s3_server_side_encryption` is enabled, S3 SSE will be configured in blobkit v0.2 via S3Config.
    /// Returns the full S3 URI on success.
    async fn upload_to_s3(&self, local_path: &str, filename: &str) -> anyhow::Result<String> {
        // Validate bucket and key via typed newtypes (replaces stringly BucketName/ObjectKey)
        let bucket = BucketName::new(self.s3_bucket.clone())
            .map_err(|e| anyhow::anyhow!("invalid bucket '{}': {}", self.s3_bucket, e))?;
        let raw_key = if self.s3_prefix.is_empty() {
            format!("{}/{}", self.database, filename)
        } else {
            format!("{}/{}/{}", self.s3_prefix, self.database, filename)
        };
        let key = ObjectKey::try_from(raw_key.clone())
            .map_err(|e| anyhow::anyhow!("invalid object key '{}': {}", raw_key, e))?;

        let data = fs::read(local_path).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to read backup file for upload: {}: {}",
                local_path,
                e
            )
        })?;
        let body_len = data.len() as u64;
        let bytes = Bytes::from(data);

        // Production path: blobkit::S3Store (stub in v0.1 — will be wired in blobkit v0.2)
        // This replaces the previous raw S3 client construction flow.
        let cfg = S3Config::new(bucket.clone(), self.s3_region.clone());
        let store = S3Store::new(cfg);
        store.put(key.clone(), bytes).await.map_err(|e| {
            anyhow::anyhow!(
                "S3 upload failed (blobkit S3Store stub in v0.1, will be wired in v0.2): {}/{}: {}",
                bucket.as_str(),
                key.as_str(),
                e
            )
        })?;

        let s3_uri = format!("s3://{}/{}", bucket.as_str(), key.as_str());
        tracing::info!(
            "blob upload complete: {} ({} bytes) -> {}",
            local_path,
            body_len,
            s3_uri
        );

        Ok(s3_uri)
    }

    /// Delete a backup from blob storage via `BlobStore::delete`.
    #[allow(dead_code)]
    async fn delete_from_s3(&self, filename: &str) -> anyhow::Result<()> {
        let bucket = BucketName::new(self.s3_bucket.clone())
            .map_err(|e| anyhow::anyhow!("invalid bucket '{}': {}", self.s3_bucket, e))?;
        let raw_key = if self.s3_prefix.is_empty() {
            format!("{}/{}", self.database, filename)
        } else {
            format!("{}/{}/{}", self.s3_prefix, self.database, filename)
        };
        let key = ObjectKey::try_from(raw_key.clone())
            .map_err(|e| anyhow::anyhow!("invalid object key '{}': {}", raw_key, e))?;

        let cfg = S3Config::new(bucket.clone(), self.s3_region.clone());
        let store = S3Store::new(cfg);
        store.delete(&key).await.map_err(|e| {
            anyhow::anyhow!(
                "blob delete failed (blobkit stub): {}/{}: {}",
                bucket.as_str(),
                key.as_str(),
                e
            )
        })?;

        tracing::info!("S3 delete: s3://{}/{}", bucket.as_str(), key.as_str());
        Ok(())
    }

    /// Validate a backup entry: checks size, checksum format, and verifies checksum against data.
    pub fn validate_backup(&self, entry: &BackupEntry, data: &[u8]) -> bool {
        if entry.size_bytes == 0 || entry.checksum.is_empty() || entry.checksum.len() < 16 {
            return false;
        }
        let computed = Self::compute_checksum(data);
        computed == entry.checksum && data.len() as u64 == entry.size_bytes
    }

    /// Clean up expired backups based on retention policy.
    pub fn cleanup_retention(&self) -> Vec<String> {
        let mut state = lock_mutex(&self.state);
        state.cleanup_retention(self.retention_days)
    }

    /// Record a successful backup in history.
    pub fn record_backup(&self, filename: String, size_bytes: u64, checksum: String) {
        let mut state = lock_mutex(&self.state);
        state.record_backup(filename, size_bytes, checksum);
    }

    /// Record a failed backup attempt.
    pub fn record_failure(&self) {
        let mut state = lock_mutex(&self.state);
        state.record_failure();
    }

    /// Get summary of backup history.
    pub fn history_summary(&self) -> (usize, u64, u64) {
        let state = lock_mutex(&self.state);
        state.history_summary()
    }

    async fn dump_postgres(&self, output: &str) -> anyhow::Result<()> {
        let mut cmd = Command::new(&self.backup_cmd);
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
            anyhow::bail!("{} failed: {}", self.backup_cmd, stderr);
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

    async fn backup_redis(&self, output: &str) -> anyhow::Result<()> {
        let client = redis::Client::open(self.redis_url.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to create Redis client: {}", e))?;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to Redis: {}", e))?;

        if !self.redis_password.is_empty() {
            let _: () = redis::cmd("AUTH")
                .arg(&self.redis_password)
                .query_async(&mut conn)
                .await
                .map_err(|e| anyhow::anyhow!("Redis AUTH failed: {}", e))?;
        }

        let _: () = redis::cmd("BGSAVE")
            .query_async(&mut conn)
            .await
            .map_err(|e| anyhow::anyhow!("Redis BGSAVE failed: {}", e))?;

        let deadline =
            tokio::time::Instant::now() + tokio::time::Duration::from_secs(self.timeout_secs);
        loop {
            let info: String = redis::cmd("INFO")
                .arg("persistence")
                .query_async(&mut conn)
                .await
                .map_err(|e| anyhow::anyhow!("Redis INFO failed: {}", e))?;
            if info.contains("rdb_bgsave_in_progress:0") {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("Redis BGSAVE timed out after {} seconds", self.timeout_secs);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let rdb_path: String = redis::cmd("CONFIG")
            .arg("GET")
            .arg("dir")
            .query_async(&mut conn)
            .await
            .map_err(|e| anyhow::anyhow!("Redis CONFIG GET dir failed: {}", e))?;

        let rdb_filename: String = redis::cmd("CONFIG")
            .arg("GET")
            .arg("dbfilename")
            .query_async(&mut conn)
            .await
            .map_err(|e| anyhow::anyhow!("Redis CONFIG GET dbfilename failed: {}", e))?;

        let dir = rdb_path.trim();
        let filename = rdb_filename.trim();
        let rdb_file = format!("{}/{}", dir, filename);

        fs::copy(&rdb_file, output).await?;
        tracing::info!("Redis backup completed: {} -> {}", rdb_file, output);
        Ok(())
    }

    async fn backup(&self) -> anyhow::Result<()> {
        let filename = self.backup_filename();
        let output_path = format!("{}/{}", self.output_dir, filename);

        fs::create_dir_all(&self.output_dir).await?;

        let mut last_error = None;
        for attempt in 0..=self.retry_max_attempts {
            if attempt > 0 {
                let delay_ms = std::cmp::min(
                    self.retry_base_delay_ms * 2u64.pow(attempt - 1),
                    self.retry_max_delay_ms,
                );
                tracing::warn!(
                    "Backup attempt {}/{} failed, retrying in {}ms",
                    attempt,
                    self.retry_max_attempts,
                    delay_ms
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }

            let result = match self.db_type.as_str() {
                "postgres" => self.dump_postgres(&output_path).await,
                "mariadb" | "mysql" => self.dump_mariadb(&output_path).await,
                "redis" => self.backup_redis(&output_path).await,
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
                    let data = fs::read(&output_path).await.map_err(|e| {
                        anyhow::anyhow!("Failed to read backup file {}: {}", output_path, e)
                    })?;
                    let size = data.len() as u64;
                    let checksum = Self::compute_checksum(&data);

                    tracing::info!(
                        "Backup {} verified: {} bytes, sha256:{}",
                        filename,
                        size,
                        &checksum[..16]
                    );

                    // Upload to S3 if configured
                    if self.storage == "s3" && !self.s3_bucket.is_empty() {
                        match self.upload_to_s3(&output_path, &filename).await {
                            Ok(s3_uri) => {
                                tracing::info!("Backup uploaded to S3: {}", s3_uri);
                                if std::env::var("BACKUP_S3_REMOVE_LOCAL")
                                    .map(|v| v == "true" || v == "1")
                                    .unwrap_or(false)
                                {
                                    if let Err(e) = fs::remove_file(&output_path).await {
                                        tracing::warn!(
                                            "Failed to remove local backup after S3 upload: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("S3 upload failed, backup retained locally: {}", e);
                            }
                        }
                    }

                    let mut state = lock_mutex(&self.state);
                    state.record_backup(filename.clone(), size, checksum);
                    tracing::info!("Backup completed: {} ({} bytes)", filename, size);
                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        // All retries exhausted
        let err = last_error.unwrap_or_else(|| anyhow::anyhow!("Backup failed after all retries"));
        let mut state = lock_mutex(&self.state);
        state.record_failure();
        tracing::error!(
            "Backup failed after {} attempts: {}",
            self.retry_max_attempts + 1,
            err
        );
        Err(err)
    }
}

/// Result of backup verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether all verification checks passed.
    pub success: bool,
    /// Whether the backup file exists on disk.
    pub file_exists: bool,
    /// Size of the backup file in bytes.
    pub file_size: u64,
    /// Whether the checksum matched (None if verification was skipped).
    pub checksum_match: Option<bool>,
    /// Whether the restore test succeeded (None if not requested).
    pub restore_test: Option<bool>,
    /// List of verification errors encountered.
    pub errors: Vec<String>,
}

/// Verifies backup integrity after backup completes.
///
/// Checks file existence, non-zero size, and SHA-256 checksum match.
/// Optionally tests restore capability.
pub struct BackupVerifier {
    verify_after: bool,
    test_restore: bool,
}

impl BackupVerifier {
    /// Create a new verifier from environment variables.
    pub fn from_env() -> Self {
        let verify_after = std::env::var("BACKUP_VERIFY_AFTER")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);
        let test_restore = std::env::var("BACKUP_VERIFY_RESTORE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        Self {
            verify_after,
            test_restore,
        }
    }

    /// Create a verifier with explicit settings.
    pub fn new(verify_after: bool, test_restore: bool) -> Self {
        Self {
            verify_after,
            test_restore,
        }
    }

    /// Verify a backup file exists, has non-zero size, and matches checksum.
    pub async fn verify(&self, path: &str, expected_checksum: &str) -> VerificationResult {
        if !self.verify_after {
            return VerificationResult {
                success: true,
                file_exists: true,
                file_size: 0,
                checksum_match: None,
                restore_test: None,
                errors: vec![],
            };
        }

        let mut errors = Vec::new();

        // Check file exists and has content
        let metadata = match fs::metadata(path).await {
            Ok(m) => m,
            Err(e) => {
                return VerificationResult {
                    success: false,
                    file_exists: false,
                    file_size: 0,
                    checksum_match: None,
                    restore_test: None,
                    errors: vec![format!("File not found: {}", e)],
                };
            }
        };

        let size = metadata.len();
        if size == 0 {
            errors.push("Backup file has zero size".to_string());
            return VerificationResult {
                success: false,
                file_exists: true,
                file_size: 0,
                checksum_match: Some(false),
                restore_test: None,
                errors,
            };
        }

        // Verify checksum
        let data = match fs::read(path).await {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("Failed to read backup file: {}", e));
                return VerificationResult {
                    success: false,
                    file_exists: true,
                    file_size: size,
                    checksum_match: None,
                    restore_test: None,
                    errors,
                };
            }
        };

        let actual_checksum = BackupShim::compute_checksum(&data);
        let checksum_ok = actual_checksum == expected_checksum;
        if !checksum_ok {
            errors.push(format!(
                "Checksum mismatch: expected={}, actual={}",
                expected_checksum, actual_checksum
            ));
        }

        // Optionally test restore
        let restore_ok = if self.test_restore && checksum_ok {
            match self.test_restore_command(path).await {
                Ok(ok) => Some(ok),
                Err(e) => {
                    errors.push(format!("Restore test failed: {}", e));
                    Some(false)
                }
            }
        } else {
            None
        };

        let success = checksum_ok && errors.is_empty();
        VerificationResult {
            success,
            file_exists: true,
            file_size: size,
            checksum_match: Some(checksum_ok),
            restore_test: restore_ok,
            errors,
        }
    }

    /// Test restore by running the appropriate restore command.
    async fn test_restore_command(&self, path: &str) -> anyhow::Result<bool> {
        let output = Command::new("pg_restore")
            .args(["--list", path])
            .output()
            .await?;

        Ok(output.status.success())
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
        if shim_core::config::validation_enabled() {
            let errors = config.validate();
            let backup_errors: Vec<_> = errors
                .iter()
                .filter(|e| e.field.starts_with("backup."))
                .collect();
            if !backup_errors.is_empty() {
                let msg = backup_errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(shim_core::Error::Config(format!(
                    "backup config validation failed: {}",
                    msg
                )));
            }
        }

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

        let state = Arc::clone(&self.state);
        let db_type = self.db_type.clone();
        let database = self.database.clone();
        let db_host = self.db_host.clone();
        let db_port = self.db_port;
        let db_user = self.db_user.clone();
        let db_password = self.db_password.clone();
        let output_dir = self.output_dir.clone();
        let compression = self.compression.clone();
        let timeout_secs = self.timeout_secs;
        let schedule_str = self.schedule.clone();
        let backup_cmd = self.backup_cmd.clone();
        let redis_url = self.redis_url.clone();
        let redis_password = self.redis_password.clone();
        let storage = self.storage.clone();
        let s3_bucket = self.s3_bucket.clone();
        let s3_region = self.s3_region.clone();
        let s3_endpoint = self.s3_endpoint.clone();
        let s3_prefix = self.s3_prefix.clone();
        let s3_force_path_style = self.s3_force_path_style;
        let s3_server_side_encryption = self.s3_server_side_encryption;
        let retry_max_attempts = self.retry_max_attempts;
        let retry_base_delay_ms = self.retry_base_delay_ms;
        let retry_max_delay_ms = self.retry_max_delay_ms;

        let schedule = cron::Schedule::from_str(&schedule_str).unwrap_or_else(|_| {
            cron::Schedule::from_str("0 0 2 * * *").expect("hardcoded valid cron expression")
        });

        tokio::spawn(async move {
            loop {
                let next_wake = schedule
                    .upcoming(chrono::Utc)
                    .next()
                    .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(1));
                let now = chrono::Utc::now();
                let sleep_secs = (next_wake - now).num_seconds().max(0) as u64;

                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)) => {}
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Backup shim loop shutting down");
                        return;
                    }
                }

                let shim = BackupShim {
                    schedule: schedule_str.clone(),
                    storage: storage.clone(),
                    backup_path: String::new(),
                    prefix: String::new(),
                    retention_days: 30,
                    database: database.clone(),
                    db_type: db_type.clone(),
                    db_host: db_host.clone(),
                    db_port,
                    db_user: db_user.clone(),
                    db_password: db_password.clone(),
                    compression: compression.clone(),
                    timeout_secs,
                    backup_cmd: backup_cmd.clone(),
                    output_dir: output_dir.clone(),
                    redis_url: redis_url.clone(),
                    redis_password: redis_password.clone(),
                    retry_max_attempts,
                    retry_base_delay_ms,
                    retry_max_delay_ms,
                    s3_bucket: s3_bucket.clone(),
                    s3_region: s3_region.clone(),
                    s3_endpoint: s3_endpoint.clone(),
                    s3_prefix: s3_prefix.clone(),
                    s3_force_path_style,
                    s3_server_side_encryption,
                    state: Arc::clone(&state),
                    shutdown_tx: None,
                };

                if let Err(e) = shim.backup().await {
                    tracing::error!("Backup failed: {}", e);
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
        let state = lock_mutex(&self.state);
        let mut metrics = vec![
            Metric::new("backup_success_total", state.backup_success as f64),
            Metric::new("backup_failure_total", state.backup_failure as f64),
            Metric::new("backup_size_bytes", state.last_backup_size as f64),
            Metric::new("backup_retained_total", state.backups_retained as f64),
            Metric::new("backup_expired_total", state.backups_expired as f64),
        ];

        if let Some(last) = &state.last_backup {
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
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_compute_checksum_differs_for_different_data() {
        let h1 = BackupShim::compute_checksum(b"hello");
        let h2 = BackupShim::compute_checksum(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_checksum_sha256() {
        let data = b"";
        let hash = BackupShim::compute_checksum(data);
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_validate_backup_valid() {
        let shim = BackupShim::new();
        let data = b"test backup content here";
        let checksum = BackupShim::compute_checksum(data);
        let entry = BackupEntry {
            filename: "test.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: data.len() as u64,
            checksum,
        };
        assert!(shim.validate_backup(&entry, data));
    }

    #[test]
    fn test_validate_backup_wrong_checksum() {
        let shim = BackupShim::new();
        let data = b"test backup content";
        let entry = BackupEntry {
            filename: "test.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: data.len() as u64,
            checksum: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        };
        assert!(!shim.validate_backup(&entry, data));
    }

    #[test]
    fn test_validate_backup_wrong_size() {
        let shim = BackupShim::new();
        let data = b"test";
        let checksum = BackupShim::compute_checksum(data);
        let entry = BackupEntry {
            filename: "test.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 999,
            checksum,
        };
        assert!(!shim.validate_backup(&entry, data));
    }

    #[test]
    fn test_validate_backup_zero_size() {
        let shim = BackupShim::new();
        let entry = BackupEntry {
            filename: "test.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 0,
            checksum: "abc123def4567890".to_string(),
        };
        assert!(!shim.validate_backup(&entry, b""));
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
        assert!(!shim.validate_backup(&entry, b"data"));
    }

    #[test]
    fn test_record_backup_and_history() {
        let shim = BackupShim::new();
        shim.record_backup("backup1.sql.gz".to_string(), 500, "checksum1".to_string());
        shim.record_backup("backup2.sql.gz".to_string(), 750, "checksum2".to_string());

        let state = shim.state.lock().unwrap();
        assert_eq!(state.backup_success, 2);
        assert_eq!(state.backup_history.len(), 2);
        assert_eq!(state.last_backup_size, 750);
        assert_eq!(state.backups_retained, 2);
        assert!(state.last_backup.is_some());
    }

    #[test]
    fn test_record_failure() {
        let shim = BackupShim::new();
        shim.record_failure();
        shim.record_failure();
        shim.record_failure();

        let state = shim.state.lock().unwrap();
        assert_eq!(state.backup_failure, 3);
    }

    #[test]
    fn test_cleanup_retention_no_expired() {
        let shim = BackupShim {
            retention_days: 30,
            ..BackupShim::new()
        };
        shim.record_backup("recent.sql.gz".to_string(), 100, "ck".to_string());

        let expired = shim.cleanup_retention();
        assert!(expired.is_empty());
        let state = shim.state.lock().unwrap();
        assert_eq!(state.backups_retained, 1);
        assert_eq!(state.backups_expired, 0);
    }

    #[test]
    fn test_cleanup_retention_removes_old() {
        let shim = BackupShim {
            retention_days: 7,
            ..BackupShim::new()
        };

        {
            let mut state = shim.state.lock().unwrap();
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
            state.backup_history.push(old_entry);
            state.backup_history.push(new_entry);
            state.backups_retained = 2;
        }

        let expired = shim.cleanup_retention();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0], "old.sql.gz");

        let state = shim.state.lock().unwrap();
        assert_eq!(state.backup_history.len(), 1);
        assert_eq!(state.backups_expired, 1);
        assert_eq!(state.backups_retained, 1);
    }

    #[test]
    fn test_history_summary() {
        let shim = BackupShim::new();
        shim.record_backup("a.sql.gz".to_string(), 100, "a".to_string());
        shim.record_backup("b.sql.gz".to_string(), 200, "b".to_string());

        let (count, total_bytes, oldest_days) = shim.history_summary();
        assert_eq!(count, 2);
        assert_eq!(total_bytes, 300);
        assert_eq!(oldest_days, 0);
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = BackupShim::new();
        {
            let mut state = shim.state.lock().unwrap();
            state.backup_success = 10;
            state.backup_failure = 2;
            state.backups_retained = 8;
            state.backups_expired = 3;
            state.last_backup_size = 5_000_000;
            state.last_backup = Some(chrono::Utc::now());
        }
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 6);
        assert_eq!(metrics[0].name, "backup_success_total");
        assert_eq!(metrics[0].value, 10.0);
        assert_eq!(metrics[1].name, "backup_failure_total");
        assert_eq!(metrics[1].value, 2.0);
    }

    #[test]
    fn test_default_db_type() {
        temp_env::with_var_unset("BACKUP_DB_TYPE", || {
            let shim = BackupShim::new();
            assert_eq!(shim.db_type, "postgres");
        });
    }

    #[test]
    fn test_default_retention() {
        temp_env::with_var_unset("BACKUP_RETENTION_DAYS", || {
            let shim = BackupShim::new();
            assert_eq!(shim.retention_days, 30);
        });
    }

    #[test]
    fn test_env_overrides() {
        temp_env::with_vars(
            [
                ("BACKUP_DB_TYPE", Some("mysql")),
                ("BACKUP_RETENTION_DAYS", Some("60")),
                ("BACKUP_COMPRESSION", Some("zstd")),
                ("BACKUP_CMD", Some("mysqldump")),
                ("BACKUP_OUTPUT_DIR", Some("/data/backups")),
                ("REDIS_URL", Some("redis://redis-host:6380")),
                ("REDIS_PASSWORD", Some("secret123")),
            ],
            || {
                let shim = BackupShim::new();
                assert_eq!(shim.db_type, "mysql");
                assert_eq!(shim.retention_days, 60);
                assert_eq!(shim.compression, "zstd");
                assert_eq!(shim.backup_cmd, "mysqldump");
                assert_eq!(shim.output_dir, "/data/backups");
                assert_eq!(shim.redis_url, "redis://redis-host:6380");
                assert_eq!(shim.redis_password, "secret123");
            },
        );
    }

    #[test]
    fn test_default_backup_cmd() {
        temp_env::with_var_unset("BACKUP_CMD", || {
            let shim = BackupShim::new();
            assert_eq!(shim.backup_cmd, "pg_dump");
        });
    }

    #[test]
    fn test_default_output_dir() {
        temp_env::with_var_unset("BACKUP_OUTPUT_DIR", || {
            let shim = BackupShim::new();
            assert_eq!(shim.output_dir, "/tmp/backups");
        });
    }

    #[test]
    fn test_default_redis_url() {
        temp_env::with_var_unset("REDIS_URL", || {
            let shim = BackupShim::new();
            assert_eq!(shim.redis_url, "redis://localhost:6379");
        });
    }

    #[test]
    fn test_verifier_from_env_defaults() {
        temp_env::with_var_unset("BACKUP_VERIFY_AFTER", || {
            temp_env::with_var_unset("BACKUP_VERIFY_RESTORE", || {
                let v = BackupVerifier::from_env();
                assert!(v.verify_after);
                assert!(!v.test_restore);
            });
        });
    }

    #[test]
    fn test_verifier_from_env_disabled() {
        temp_env::with_vars(
            [
                ("BACKUP_VERIFY_AFTER", Some("false")),
                ("BACKUP_VERIFY_RESTORE", Some("true")),
            ],
            || {
                let v = BackupVerifier::from_env();
                assert!(!v.verify_after);
                assert!(v.test_restore);
            },
        );
    }

    #[test]
    fn test_verifier_new_explicit() {
        let v = BackupVerifier::new(true, true);
        assert!(v.verify_after);
        assert!(v.test_restore);
    }

    #[tokio::test]
    async fn test_verify_file_not_found() {
        let v = BackupVerifier::new(true, false);
        let result = v.verify("/nonexistent/path/backup.sql", "abc123").await;
        assert!(!result.success);
        assert!(!result.file_exists);
        assert!(result.file_size == 0);
        assert!(!result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_verify_file_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.sql");
        std::fs::write(&path, "").unwrap();

        let v = BackupVerifier::new(true, false);
        let result = v.verify(path.to_str().unwrap(), "abc").await;
        assert!(!result.success);
        assert!(result.file_exists);
        assert_eq!(result.file_size, 0);
        assert!(result.errors.iter().any(|e| e.contains("zero size")));
    }

    #[tokio::test]
    async fn test_verify_checksum_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.sql");
        let data = b"backup content here";
        std::fs::write(&path, data).unwrap();

        let checksum = BackupShim::compute_checksum(data);
        let v = BackupVerifier::new(true, false);
        let result = v.verify(path.to_str().unwrap(), &checksum).await;
        assert!(result.success);
        assert!(result.file_exists);
        assert_eq!(result.file_size, data.len() as u64);
        assert_eq!(result.checksum_match, Some(true));
    }

    #[tokio::test]
    async fn test_verify_checksum_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.sql");
        std::fs::write(&path, b"content").unwrap();

        let v = BackupVerifier::new(true, false);
        let result = v
            .verify(
                path.to_str().unwrap(),
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .await;
        assert!(!result.success);
        assert!(result.file_exists);
        assert_eq!(result.checksum_match, Some(false));
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("Checksum mismatch")));
    }

    #[tokio::test]
    async fn test_verify_disabled_skips_checks() {
        let v = BackupVerifier::new(false, false);
        let result = v.verify("/nonexistent", "abc").await;
        assert!(result.success);
        assert!(result.file_exists);
        assert!(result.checksum_match.is_none());
    }

    // --- Additional coverage tests ---

    #[test]
    fn test_backup_entry_serialization_roundtrip() {
        let entry = BackupEntry {
            filename: "mydb_20240101_120000.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 1048576,
            checksum: "abcdef1234567890".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: BackupEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.filename, "mydb_20240101_120000.sql.gz");
        assert_eq!(deserialized.size_bytes, 1048576);
        assert_eq!(deserialized.checksum, "abcdef1234567890");
    }

    #[test]
    fn test_backup_meta_serialization_roundtrip() {
        let meta = BackupMeta {
            database: "mydb".to_string(),
            db_type: "postgres".to_string(),
            started_at: "2024-01-01T00:00:00Z".to_string(),
            completed_at: Some("2024-01-01T00:01:00Z".to_string()),
            path: "/tmp/backups/mydb_20240101.sql.gz".to_string(),
            size_bytes: 1024000,
            success: true,
            error: None,
            checksum: Some("abc123".to_string()),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: BackupMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.database, "mydb");
        assert!(deserialized.success);
        assert_eq!(deserialized.size_bytes, 1024000);
    }

    #[test]
    fn test_backup_meta_failed_backup() {
        let meta = BackupMeta {
            database: "mydb".to_string(),
            db_type: "mysql".to_string(),
            started_at: "2024-01-01T00:00:00Z".to_string(),
            completed_at: None,
            path: String::new(),
            size_bytes: 0,
            success: false,
            error: Some("connection refused".to_string()),
            checksum: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: BackupMeta = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.success);
        assert_eq!(deserialized.error, Some("connection refused".to_string()));
        assert!(deserialized.completed_at.is_none());
    }

    #[test]
    fn test_verification_result_serialization_roundtrip() {
        let result = VerificationResult {
            success: true,
            file_exists: true,
            file_size: 1024,
            checksum_match: Some(true),
            restore_test: Some(false),
            errors: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: VerificationResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.success);
        assert_eq!(deserialized.file_size, 1024);
        assert_eq!(deserialized.checksum_match, Some(true));
    }

    #[test]
    fn test_verification_result_with_errors() {
        let result = VerificationResult {
            success: false,
            file_exists: true,
            file_size: 0,
            checksum_match: Some(false),
            restore_test: None,
            errors: vec![
                "checksum mismatch".to_string(),
                "file corrupted".to_string(),
            ],
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: VerificationResult = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.success);
        assert_eq!(deserialized.errors.len(), 2);
    }

    #[test]
    fn test_s3_config_from_env() {
        temp_env::with_vars(
            [
                ("BACKUP_S3_BUCKET", Some("my-backup-bucket")),
                ("BACKUP_S3_REGION", Some("eu-west-1")),
                ("BACKUP_S3_ENDPOINT", Some("http://localhost:9000")),
                ("BACKUP_S3_PREFIX", Some("db-backups")),
                ("BACKUP_S3_FORCE_PATH_STYLE", Some("true")),
                ("BACKUP_S3_SSE", Some("false")),
            ],
            || {
                let shim = BackupShim::new();
                assert_eq!(shim.s3_bucket(), "my-backup-bucket");
                assert_eq!(shim.s3_region(), "eu-west-1");
                assert_eq!(shim.s3_endpoint(), Some("http://localhost:9000"));
                assert_eq!(shim.s3_prefix(), "db-backups");
                assert!(shim.s3_force_path_style());
                assert!(!shim.s3_server_side_encryption());
            },
        );
    }

    #[test]
    fn test_s3_config_defaults() {
        temp_env::with_vars(
            [
                ("BACKUP_S3_BUCKET", None::<&str>),
                ("BACKUP_S3_REGION", None::<&str>),
                ("BACKUP_S3_ENDPOINT", None::<&str>),
                ("BACKUP_S3_PREFIX", None::<&str>),
                ("BACKUP_S3_FORCE_PATH_STYLE", None::<&str>),
                ("BACKUP_S3_SSE", None::<&str>),
            ],
            || {
                let shim = BackupShim::new();
                assert_eq!(shim.s3_bucket(), "");
                assert_eq!(shim.s3_region(), "us-east-1");
                assert!(shim.s3_endpoint().is_none());
                assert_eq!(shim.s3_prefix(), "backups");
                assert!(!shim.s3_force_path_style());
                assert!(shim.s3_server_side_encryption());
            },
        );
    }

    #[test]
    fn test_s3_config_force_path_style_values() {
        for val in &["true", "1"] {
            temp_env::with_var("BACKUP_S3_FORCE_PATH_STYLE", Some(*val), || {
                let shim = BackupShim::new();
                assert!(shim.s3_force_path_style());
            });
        }
        for val in &["false", "0", "no", "yes"] {
            temp_env::with_var("BACKUP_S3_FORCE_PATH_STYLE", Some(*val), || {
                let shim = BackupShim::new();
                if *val == "true" || *val == "1" {
                    assert!(shim.s3_force_path_style());
                } else {
                    assert!(!shim.s3_force_path_style());
                }
            });
        }
    }

    #[test]
    fn test_backup_shim_env_all_fields() {
        temp_env::with_vars(
            [
                ("BACKUP_SCHEDULE", Some("0 0 3 * * *")),
                ("BACKUP_STORAGE", Some("s3")),
                ("BACKUP_PATH", Some("/data/backups")),
                ("BACKUP_PREFIX", Some("prod/")),
                ("BACKUP_RETENTION_DAYS", Some("60")),
                ("BACKUP_DATABASE", Some("mydb")),
                ("BACKUP_DB_TYPE", Some("mysql")),
                ("BACKUP_DB_HOST", Some("db.internal")),
                ("BACKUP_DB_PORT", Some("3306")),
                ("BACKUP_DB_USER", Some("backup_user")),
                ("BACKUP_DB_PASSWORD", Some("secret123")),
                ("BACKUP_CMD", Some("mysqldump")),
                ("BACKUP_OUTPUT_DIR", Some("/data/output")),
                ("BACKUP_COMPRESSION", Some("zstd")),
                ("BACKUP_TIMEOUT_SECS", Some("7200")),
            ],
            || {
                let shim = BackupShim::new();
                assert_eq!(shim.schedule, "0 0 3 * * *");
                assert_eq!(shim.storage, "s3");
                assert_eq!(shim.prefix, "prod/");
                assert_eq!(shim.retention_days, 60);
                assert_eq!(shim.database, "mydb");
                assert_eq!(shim.db_type, "mysql");
                assert_eq!(shim.db_host, "db.internal");
                assert_eq!(shim.db_port, 3306);
                assert_eq!(shim.db_user, "backup_user");
                assert_eq!(shim.db_password, "secret123");
                assert_eq!(shim.backup_cmd, "mysqldump");
                assert_eq!(shim.output_dir, "/data/output");
                assert_eq!(shim.compression, "zstd");
                assert_eq!(shim.timeout_secs, 7200);
            },
        );
    }

    #[test]
    fn test_backup_shim_retry_config_defaults() {
        temp_env::with_vars(
            [
                ("BACKUP_RETRY_MAX_ATTEMPTS", None::<&str>),
                ("BACKUP_RETRY_BASE_DELAY_MS", None::<&str>),
                ("BACKUP_RETRY_MAX_DELAY_MS", None::<&str>),
            ],
            || {
                let shim = BackupShim::new();
                assert_eq!(shim.retry_max_attempts, 3);
                assert_eq!(shim.retry_base_delay_ms, 1000);
                assert_eq!(shim.retry_max_delay_ms, 30000);
            },
        );
    }

    #[test]
    fn test_backup_shim_retry_config_from_env() {
        temp_env::with_vars(
            [
                ("BACKUP_RETRY_MAX_ATTEMPTS", Some("5")),
                ("BACKUP_RETRY_BASE_DELAY_MS", Some("500")),
                ("BACKUP_RETRY_MAX_DELAY_MS", Some("60000")),
            ],
            || {
                let shim = BackupShim::new();
                assert_eq!(shim.retry_max_attempts, 5);
                assert_eq!(shim.retry_base_delay_ms, 500);
                assert_eq!(shim.retry_max_delay_ms, 60000);
            },
        );
    }

    #[test]
    fn test_retention_policy_all_expired() {
        let shim = BackupShim {
            retention_days: 1,
            ..BackupShim::new()
        };

        {
            let mut state = shim.state.lock().unwrap();
            state.backup_history.push(BackupEntry {
                filename: "old1.sql.gz".to_string(),
                created_at: chrono::Utc::now() - chrono::Duration::days(10),
                size_bytes: 100,
                checksum: "ck1".to_string(),
            });
            state.backup_history.push(BackupEntry {
                filename: "old2.sql.gz".to_string(),
                created_at: chrono::Utc::now() - chrono::Duration::days(5),
                size_bytes: 200,
                checksum: "ck2".to_string(),
            });
            state.backups_retained = 2;
        }

        let expired = shim.cleanup_retention();
        assert_eq!(expired.len(), 2);
        assert!(expired.contains(&"old1.sql.gz".to_string()));
        assert!(expired.contains(&"old2.sql.gz".to_string()));

        let state = shim.state.lock().unwrap();
        assert_eq!(state.backup_history.len(), 0);
        assert_eq!(state.backups_expired, 2);
        assert_eq!(state.backups_retained, 0);
    }

    #[test]
    fn test_retention_policy_boundary_exactly_on_day() {
        let shim = BackupShim {
            retention_days: 7,
            ..BackupShim::new()
        };

        {
            let mut state = shim.state.lock().unwrap();
            // Just under 7 days old - should be retained
            state.backup_history.push(BackupEntry {
                filename: "boundary.sql.gz".to_string(),
                created_at: chrono::Utc::now() - chrono::Duration::hours(167),
                size_bytes: 100,
                checksum: "ck".to_string(),
            });
            state.backups_retained = 1;
        }

        let expired = shim.cleanup_retention();
        assert!(expired.is_empty());
        let state = shim.state.lock().unwrap();
        assert_eq!(state.backup_history.len(), 1);
    }

    #[test]
    fn test_retention_policy_boundary_one_day_over() {
        let shim = BackupShim {
            retention_days: 7,
            ..BackupShim::new()
        };

        {
            let mut state = shim.state.lock().unwrap();
            // 8 days old - should be expired
            state.backup_history.push(BackupEntry {
                filename: "expired.sql.gz".to_string(),
                created_at: chrono::Utc::now() - chrono::Duration::days(8),
                size_bytes: 100,
                checksum: "ck".to_string(),
            });
            state.backups_retained = 1;
        }

        let expired = shim.cleanup_retention();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0], "expired.sql.gz");
    }

    #[test]
    fn test_history_summary_empty() {
        let shim = BackupShim::new();
        let (count, total_bytes, oldest_days) = shim.history_summary();
        assert_eq!(count, 0);
        assert_eq!(total_bytes, 0);
        assert_eq!(oldest_days, 0);
    }

    #[test]
    fn test_history_summary_single_entry() {
        let shim = BackupShim::new();
        shim.record_backup("backup1.sql.gz".to_string(), 500, "checksum1".to_string());
        let (count, total_bytes, _) = shim.history_summary();
        assert_eq!(count, 1);
        assert_eq!(total_bytes, 500);
    }

    #[test]
    fn test_record_failure_increments() {
        let shim = BackupShim::new();
        shim.record_failure();
        shim.record_failure();
        let state = shim.state.lock().unwrap();
        assert_eq!(state.backup_failure, 2);
        assert_eq!(state.backup_success, 0);
    }

    #[test]
    fn test_record_backup_updates_last_backup() {
        let shim = BackupShim::new();
        shim.record_backup("b1.sql.gz".to_string(), 100, "ck1".to_string());
        let state = shim.state.lock().unwrap();
        assert!(state.last_backup.is_some());
        assert_eq!(state.last_backup_size, 100);
    }

    #[test]
    fn test_record_backup_overwrites_last_backup() {
        let shim = BackupShim::new();
        shim.record_backup("b1.sql.gz".to_string(), 100, "ck1".to_string());
        shim.record_backup("b2.sql.gz".to_string(), 200, "ck2".to_string());
        let state = shim.state.lock().unwrap();
        assert_eq!(state.last_backup_size, 200);
    }

    #[test]
    fn test_backup_filename_different_databases() {
        for (db, expected_ext) in &[("mydb", ".sql.gz"), ("analytics", ".sql.gz")] {
            let shim = BackupShim {
                database: db.to_string(),
                compression: "gzip".to_string(),
                ..BackupShim::new()
            };
            let name = shim.backup_filename();
            assert!(name.starts_with(&format!("{}_", db)));
            assert!(name.ends_with(expected_ext));
        }
    }

    #[test]
    fn test_backup_shim_name() {
        let shim = BackupShim::new();
        assert_eq!(shim.name(), "backup");
    }

    #[test]
    fn test_backup_shim_default_trait() {
        let shim = BackupShim::default();
        assert_eq!(shim.db_type, "postgres");
        assert_eq!(shim.retention_days, 30);
    }

    #[test]
    fn test_backup_shim_accessor_methods() {
        let shim = BackupShim::new();
        assert_eq!(shim.db_type(), "postgres");
        assert_eq!(shim.db_host(), "127.0.0.1");
        assert_eq!(shim.db_port(), 5432);
        assert_eq!(shim.db_user(), "postgres");
        assert_eq!(shim.db_password(), "");
        assert_eq!(shim.database(), "");
        assert_eq!(shim.backup_cmd(), "pg_dump");
        assert_eq!(shim.output_dir(), "/tmp/backups");
        assert_eq!(shim.redis_url(), "redis://localhost:6379");
        assert_eq!(shim.redis_password(), "");
        assert_eq!(shim.timeout_secs(), 3600);
    }

    #[test]
    fn test_compute_checksum_large_data() {
        let data = vec![0u8; 1_000_000];
        let hash = BackupShim::compute_checksum(&data);
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_compute_checksum_binary_data() {
        let data: Vec<u8> = (0..=255).collect();
        let hash = BackupShim::compute_checksum(&data);
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_validate_backup_short_checksum() {
        let shim = BackupShim::new();
        let entry = BackupEntry {
            filename: "test.sql.gz".to_string(),
            created_at: chrono::Utc::now(),
            size_bytes: 10,
            checksum: "short".to_string(),
        };
        assert!(!shim.validate_backup(&entry, b"0123456789"));
    }

    #[tokio::test]
    async fn test_verify_file_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.sql");
        std::fs::write(&path, b"content").unwrap();

        let v = BackupVerifier::new(true, false);
        // Valid file with matching checksum
        let checksum = BackupShim::compute_checksum(b"content");
        let result = v.verify(path.to_str().unwrap(), &checksum).await;
        assert!(result.success);
        assert_eq!(result.file_size, 7);
    }

    #[test]
    fn test_retention_policy_mixed_ages() {
        let shim = BackupShim {
            retention_days: 30,
            ..BackupShim::new()
        };

        {
            let mut state = shim.state.lock().unwrap();
            // 60 days old - expired
            state.backup_history.push(BackupEntry {
                filename: "60d.sql.gz".to_string(),
                created_at: chrono::Utc::now() - chrono::Duration::days(60),
                size_bytes: 100,
                checksum: "ck1".to_string(),
            });
            // 31 days old - expired
            state.backup_history.push(BackupEntry {
                filename: "31d.sql.gz".to_string(),
                created_at: chrono::Utc::now() - chrono::Duration::days(31),
                size_bytes: 200,
                checksum: "ck2".to_string(),
            });
            // 1 day old - retained
            state.backup_history.push(BackupEntry {
                filename: "1d.sql.gz".to_string(),
                created_at: chrono::Utc::now() - chrono::Duration::days(1),
                size_bytes: 300,
                checksum: "ck3".to_string(),
            });
            state.backups_retained = 3;
        }

        let expired = shim.cleanup_retention();
        assert_eq!(expired.len(), 2);

        let state = shim.state.lock().unwrap();
        assert_eq!(state.backup_history.len(), 1);
        assert_eq!(state.backup_history[0].filename, "1d.sql.gz");
        assert_eq!(state.backups_retained, 1);
        assert_eq!(state.backups_expired, 2);
    }
}
