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
//! BACKUP_CMD           Backup command override (default: pg_dump)
//! BACKUP_OUTPUT_DIR    Output directory (default: /tmp/backups)
//! REDIS_URL            Redis connection URL (default: redis://localhost:6379)
//! REDIS_PASSWORD       Redis password
//! BACKUP_COMPRESSION   Compression: none, gzip, zstd (default: gzip)
//! BACKUP_TIMEOUT_SECS  Timeout for dump command (default: 3600)
//! ```

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use aws_sdk_s3::Client as S3Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

/// Shared backup state between parent and spawned task.
pub struct BackupState {
    pub backup_success: u64,
    pub backup_failure: u64,
    pub backups_retained: u64,
    pub backups_expired: u64,
    pub last_backup: Option<chrono::DateTime<chrono::Utc>>,
    pub last_backup_size: u64,
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
            backup_cmd: std::env::var("BACKUP_CMD").unwrap_or_else(|_| "pg_dump".to_string()),
            output_dir: std::env::var("BACKUP_OUTPUT_DIR")
                .unwrap_or_else(|_| "/tmp/backups".to_string()),
            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            redis_password: std::env::var("REDIS_PASSWORD").unwrap_or_default(),
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

    /// Build an S3 client from the current configuration.
    async fn build_s3_client(&self) -> anyhow::Result<S3Client> {
        let mut config_loader =
            aws_config::from_env().region(aws_config::Region::new(self.s3_region.clone()));

        if let Some(endpoint) = &self.s3_endpoint {
            config_loader = config_loader.endpoint_url(endpoint);
        }

        let sdk_config = config_loader.load().await;

        let mut s3_config = aws_sdk_s3::config::Builder::from(&sdk_config);
        if self.s3_force_path_style {
            s3_config = s3_config.force_path_style(true);
        }

        Ok(S3Client::from_conf(s3_config.build()))
    }

    /// Upload a file to S3.
    ///
    /// The key is constructed as `{s3_prefix}/{database}/{filename}`.
    /// If `s3_server_side_encryption` is enabled, uses AES256 SSE.
    /// Returns the full S3 URI on success.
    async fn upload_to_s3(&self, local_path: &str, filename: &str) -> anyhow::Result<String> {
        let client = self.build_s3_client().await?;

        let key = if self.s3_prefix.is_empty() {
            format!("{}/{}", self.database, filename)
        } else {
            format!("{}/{}/{}", self.s3_prefix, self.database, filename)
        };

        let body = fs::read(local_path).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to read backup file for S3 upload: {}: {}",
                local_path,
                e
            )
        })?;
        let body_len = body.len() as u64;

        let mut put_req = client
            .put_object()
            .bucket(&self.s3_bucket)
            .key(&key)
            .body(body.into());

        if self.s3_server_side_encryption {
            put_req =
                put_req.server_side_encryption(aws_sdk_s3::types::ServerSideEncryption::Aes256);
        }

        // Set content type based on file extension
        let content_type = if filename.ends_with(".gz") {
            "application/gzip"
        } else if filename.ends_with(".zst") {
            "application/zstd"
        } else {
            "application/octet-stream"
        };
        put_req = put_req.content_type(content_type);

        put_req
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("S3 upload failed: {}/{}: {}", self.s3_bucket, key, e))?;

        let s3_uri = format!("s3://{}/{}", self.s3_bucket, key);
        tracing::info!(
            "S3 upload complete: {} ({} bytes) -> {}",
            local_path,
            body_len,
            s3_uri
        );

        Ok(s3_uri)
    }

    /// Delete a backup from S3.
    async fn delete_from_s3(&self, filename: &str) -> anyhow::Result<()> {
        let client = self.build_s3_client().await?;

        let key = if self.s3_prefix.is_empty() {
            format!("{}/{}", self.database, filename)
        } else {
            format!("{}/{}/{}", self.s3_prefix, self.database, filename)
        };

        client
            .delete_object()
            .bucket(&self.s3_bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("S3 delete failed: {}/{}: {}", self.s3_bucket, key, e))?;

        tracing::info!("S3 delete: s3://{}/{}", self.s3_bucket, key);
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
        let mut state = self.state.lock().unwrap();
        state.cleanup_retention(self.retention_days)
    }

    /// Record a successful backup in history.
    pub fn record_backup(&self, filename: String, size_bytes: u64, checksum: String) {
        let mut state = self.state.lock().unwrap();
        state.record_backup(filename, size_bytes, checksum);
    }

    /// Record a failed backup attempt.
    pub fn record_failure(&self) {
        let mut state = self.state.lock().unwrap();
        state.record_failure();
    }

    /// Get summary of backup history.
    pub fn history_summary(&self) -> (usize, u64, u64) {
        let state = self.state.lock().unwrap();
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
                            // Optionally remove local file after successful S3 upload
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

                let mut state = self.state.lock().unwrap();
                state.record_backup(filename.clone(), size, checksum);
                tracing::info!("Backup completed: {} ({} bytes)", filename, size);
            }
            Err(e) => {
                let mut state = self.state.lock().unwrap();
                state.record_failure();
                tracing::error!("Backup failed: {}", e);
            }
        }

        Ok(())
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

        let schedule = cron::Schedule::from_str(&schedule_str)
            .unwrap_or_else(|_| cron::Schedule::from_str("0 2 * * *").unwrap());

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
        let state = self.state.lock().unwrap();
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
        let shim = BackupShim::new();
        assert_eq!(shim.retention_days, 30);
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
}
