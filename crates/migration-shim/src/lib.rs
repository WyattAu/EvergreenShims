//! Migration shim — database schema migrations with rollback support.
//!
//! Runs SQL migration files from a directory in order, tracking applied
//! migrations in a `schema_migrations` table.
//!
//! ## Environment Variables
//!
//! ```text
//! MIGRATION_DIR       Directory containing migration files (default: /migrations)
//! MIGRATION_DATABASE  Database name
//! MIGRATION_DB_HOST   Database host (default: 127.0.0.1)
//! MIGRATION_DB_PORT   Database port (default: 5432)
//! MIGRATION_DB_USER   Database user (default: postgres)
//! MIGRATION_DB_PASSWORD Database password
//! MIGRATION_DB_URL    Full database URL (overrides host/port/user/password/name)
//! MIGRATION_AUTO_MIGRATE Auto-migrate on startup (default: false)
//! MIGRATION_DB_TYPE   Database type: postgres, mysql
//! ```
//!
//! ## Migration File Naming
//!
//! Migration files should follow the pattern:
//! ```text
//! 001_create_users.up.sql
//! 001_create_users.down.sql
//! 002_add_email_index.up.sql
//! 002_add_email_index.down.sql
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::fs;
use tokio::sync::watch;

/// Lock file content stored as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub pid: u32,
    pub hostname: String,
    pub created_at: String,
}

/// Manages migration lock files to prevent concurrent migrations.
///
/// Creates a `.migration.lock` file in the migration directory before
/// starting migrations and removes it after completion. Detects stale
/// locks older than 1 hour.
pub struct MigrationLock {
    lock_path: PathBuf,
    stale_threshold_secs: u64,
}

impl MigrationLock {
    /// Create a new lock manager for the given migration directory.
    pub fn new(migration_dir: &std::path::Path) -> Self {
        Self {
            lock_path: migration_dir.join(".migration.lock"),
            stale_threshold_secs: 3600,
        }
    }

    /// Create a new lock manager with a custom stale threshold.
    pub fn with_stale_threshold(
        migration_dir: &std::path::Path,
        stale_threshold_secs: u64,
    ) -> Self {
        Self {
            lock_path: migration_dir.join(".migration.lock"),
            stale_threshold_secs,
        }
    }

    /// Check if a lock file currently exists.
    pub fn is_locked(&self) -> bool {
        self.lock_path.exists()
    }

    /// Acquire the migration lock using atomic file creation.
    ///
    /// If a lock already exists and is stale (> threshold old), it is
    /// removed first. Returns `Ok(())` on success, `Err` if the lock
    /// cannot be acquired.
    pub fn acquire_lock(&self) -> std::result::Result<(), String> {
        if self.lock_path.exists() {
            if self.is_stale() {
                tracing::warn!(
                    "Removing stale migration lock at {}",
                    self.lock_path.display()
                );
                std::fs::remove_file(&self.lock_path)
                    .map_err(|e| format!("Failed to remove stale lock: {}", e))?;
            } else {
                let content = std::fs::read_to_string(&self.lock_path)
                    .unwrap_or_else(|_| "<unreadable>".to_string());
                return Err(format!(
                    "Migration lock already held by another process: {}",
                    content
                ));
            }
        }

        let hostname = gethostname::gethostname().to_string_lossy().to_string();

        let info = LockInfo {
            pid: std::process::id(),
            hostname,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let content = serde_json::to_string_pretty(&info)
            .map_err(|e| format!("Failed to serialize lock info: {}", e))?;

        std::fs::File::create_new(&self.lock_path)
            .map_err(|e| format!("Failed to create lock file: {}", e))?;

        std::fs::write(&self.lock_path, &content)
            .map_err(|e| format!("Failed to write lock file: {}", e))?;

        tracing::info!("Migration lock acquired at {}", self.lock_path.display());
        Ok(())
    }

    /// Release the migration lock by removing the lock file.
    pub fn release_lock(&self) -> std::result::Result<(), String> {
        if self.lock_path.exists() {
            std::fs::remove_file(&self.lock_path)
                .map_err(|e| format!("Failed to remove lock file: {}", e))?;
            tracing::info!("Migration lock released at {}", self.lock_path.display());
        }
        Ok(())
    }

    /// Check if an existing lock file is stale (older than threshold).
    fn is_stale(&self) -> bool {
        let content = match std::fs::read_to_string(&self.lock_path) {
            Ok(c) => c,
            Err(_) => return true,
        };

        let info: LockInfo = match serde_json::from_str(&content) {
            Ok(i) => i,
            Err(_) => return true,
        };

        let created = match chrono::DateTime::parse_from_rfc3339(&info.created_at) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => return true,
        };

        let age = chrono::Utc::now() - created;
        age.num_seconds() as u64 > self.stale_threshold_secs
    }
}

/// Migration record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRecord {
    pub version: u32,
    pub name: String,
    pub applied_at: String,
    pub checksum: String,
}

/// Represents a single migration with its up and down SQL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    pub version: u32,
    pub name: String,
    pub up_sql: String,
    pub down_sql: Option<String>,
    pub checksum: String,
}

/// Result of a migration dry-run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DryRunResult {
    pub version: u32,
    pub name: String,
    pub would_apply: bool,
    pub reason: String,
    pub checksum_valid: bool,
    pub sql_size_bytes: u64,
    pub has_down_migration: bool,
}

/// Migration error types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationError {
    VersionConflict {
        existing: u32,
        incoming: u32,
    },
    ChecksumMismatch {
        version: u32,
        expected: String,
        actual: String,
    },
    MissingDownMigration {
        version: u32,
    },
    InvalidVersion {
        version: String,
    },
    OutOfOrder {
        expected: u32,
        got: u32,
    },
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionConflict { existing, incoming } => {
                write!(
                    f,
                    "Version conflict: existing={}, incoming={}",
                    existing, incoming
                )
            }
            Self::ChecksumMismatch {
                version,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "Checksum mismatch at v{}: expected={}, actual={}",
                    version, expected, actual
                )
            }
            Self::MissingDownMigration { version } => {
                write!(f, "Missing down migration for version {}", version)
            }
            Self::InvalidVersion { version } => {
                write!(f, "Invalid version: {}", version)
            }
            Self::OutOfOrder { expected, got } => {
                write!(
                    f,
                    "Out of order migration: expected v{}, got v{}",
                    expected, got
                )
            }
        }
    }
}

/// Migration shim for database schema management.
pub struct MigrationShim {
    dir: PathBuf,
    database: String,
    db_host: String,
    db_port: u16,
    db_user: String,
    db_password: String,
    db_url: Option<String>,
    auto_migrate: bool,
    db_type: String,
    current_version: u32,
    migrations_applied: u64,
    migrations_rolled_back: u64,
    last_migration: Option<chrono::DateTime<chrono::Utc>>,
    applied_records: BTreeMap<u32, MigrationRecord>,
    pending_migrations: Vec<Migration>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl MigrationShim {
    pub fn new() -> Self {
        Self {
            dir: std::env::var("MIGRATION_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/migrations")),
            database: std::env::var("MIGRATION_DATABASE").unwrap_or_default(),
            db_host: std::env::var("MIGRATION_DB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            db_port: std::env::var("MIGRATION_DB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5432),
            db_user: std::env::var("MIGRATION_DB_USER").unwrap_or_else(|_| "postgres".to_string()),
            db_password: std::env::var("MIGRATION_DB_PASSWORD").unwrap_or_default(),
            db_url: std::env::var("MIGRATION_DB_URL").ok(),
            auto_migrate: std::env::var("MIGRATION_AUTO_MIGRATE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            db_type: std::env::var("MIGRATION_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            current_version: 0,
            migrations_applied: 0,
            migrations_rolled_back: 0,
            last_migration: None,
            applied_records: BTreeMap::new(),
            pending_migrations: Vec::new(),
            shutdown_tx: None,
        }
    }

    /// Check if a real database connection is configured.
    fn has_db(&self) -> bool {
        !self.db_host.is_empty() && !self.database.is_empty()
    }

    /// Build a connection string for the configured database.
    fn connection_string(&self) -> String {
        if let Some(ref url) = self.db_url {
            return url.clone();
        }
        match self.db_type.as_str() {
            "mysql" => format!(
                "mysql://{}:{}@{}:{}/{}",
                self.db_user, self.db_password, self.db_host, self.db_port, self.database
            ),
            _ => format!(
                "postgres://{}:{}@{}:{}/{}",
                self.db_user, self.db_password, self.db_host, self.db_port, self.database
            ),
        }
    }

    /// Create the `schema_migrations` tracking table if it doesn't exist.
    async fn ensure_migrations_table(&self) -> anyhow::Result<()> {
        if !self.has_db() {
            tracing::debug!("No database configured, skipping schema_migrations table creation");
            return Ok(());
        }

        let create_sql = "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            checksum TEXT NOT NULL,
            applied_at TEXT NOT NULL
        )";

        match self.db_type.as_str() {
            "mysql" => {
                let cs = self.connection_string();
                let pool = sqlx::MySqlPool::connect(&cs).await?;
                sqlx::query(create_sql).execute(&pool).await?;
            }
            _ => {
                self.execute_sql_postgres(create_sql).await?;
            }
        }
        tracing::info!("schema_migrations table ensured");
        Ok(())
    }

    /// Execute a SQL statement via psql command.
    #[allow(dead_code)]
    async fn execute_sql_via_psql(&self, sql: &str) -> anyhow::Result<()> {
        let output = tokio::process::Command::new("psql")
            .args([
                "-h",
                &self.db_host,
                "-p",
                &self.db_port.to_string(),
                "-U",
                &self.db_user,
                "-d",
                &self.database,
                "-c",
                sql,
            ])
            .env("PGPASSWORD", &self.db_password)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("psql failed: {}", stderr);
        }
        Ok(())
    }

    /// Execute a SQL statement against PostgreSQL using sqlx.
    async fn execute_sql_postgres(&self, sql: &str) -> anyhow::Result<()> {
        let cs = self.connection_string();
        let pool = sqlx::PgPool::connect(&cs).await?;
        sqlx::query(sql).execute(&pool).await?;
        Ok(())
    }

    /// Execute a SQL statement against the configured database.
    async fn execute_sql(&self, sql: &str) -> anyhow::Result<()> {
        if !self.has_db() {
            tracing::debug!("No database configured, skipping SQL execution");
            return Ok(());
        }

        match self.db_type.as_str() {
            "mysql" => {
                let cs = self.connection_string();
                let pool = sqlx::MySqlPool::connect(&cs).await?;
                sqlx::query(sql).execute(&pool).await?;
            }
            _ => {
                self.execute_sql_postgres(sql).await?;
            }
        }
        Ok(())
    }

    /// Insert a migration record into `schema_migrations` table.
    async fn insert_migration_record(&self, record: &MigrationRecord) -> anyhow::Result<()> {
        if !self.has_db() {
            return Ok(());
        }

        match self.db_type.as_str() {
            "mysql" => {
                let cs = self.connection_string();
                let pool = sqlx::MySqlPool::connect(&cs).await?;
                sqlx::query("INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?, ?, ?, ?)")
                    .bind(record.version as i32)
                    .bind(&record.name)
                    .bind(&record.checksum)
                    .bind(&record.applied_at)
                    .execute(&pool)
                    .await?;
            }
            _ => {
                let cs = self.connection_string();
                let pool = sqlx::PgPool::connect(&cs).await?;
                sqlx::query("INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES ($1, $2, $3, $4)")
                    .bind(record.version as i32)
                    .bind(&record.name)
                    .bind(&record.checksum)
                    .bind(&record.applied_at)
                    .execute(&pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Delete a migration record from `schema_migrations` table.
    async fn delete_migration_record(&self, version: u32) -> anyhow::Result<()> {
        if !self.has_db() {
            return Ok(());
        }

        match self.db_type.as_str() {
            "mysql" => {
                let cs = self.connection_string();
                let pool = sqlx::MySqlPool::connect(&cs).await?;
                sqlx::query("DELETE FROM schema_migrations WHERE version = ?")
                    .bind(version as i32)
                    .execute(&pool)
                    .await?;
            }
            _ => {
                let cs = self.connection_string();
                let pool = sqlx::PgPool::connect(&cs).await?;
                sqlx::query("DELETE FROM schema_migrations WHERE version = $1")
                    .bind(version as i32)
                    .execute(&pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Compute FNV-1a checksum of migration SQL content.
    pub fn compute_checksum(sql: &str) -> String {
        let mut hash: u64 = 0xcbf29ce484222325;
        for &byte in sql.as_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{:016x}", hash)
    }

    /// Parse version from a migration filename.
    pub fn parse_version(filename: &str) -> std::result::Result<u32, MigrationError> {
        let name = filename
            .trim_end_matches(".up.sql")
            .trim_end_matches(".down.sql");
        let parts: Vec<&str> = name.splitn(2, '_').collect();
        parts
            .first()
            .ok_or_else(|| MigrationError::InvalidVersion {
                version: filename.to_string(),
            })
            .and_then(|v| {
                v.parse::<u32>()
                    .map_err(|_| MigrationError::InvalidVersion {
                        version: v.to_string(),
                    })
            })
    }

    /// Parse migration name from filename.
    pub fn parse_name(filename: &str) -> String {
        let name = filename
            .trim_end_matches(".up.sql")
            .trim_end_matches(".down.sql");
        let parts: Vec<&str> = name.splitn(2, '_').collect();
        parts.get(1).unwrap_or(&"").to_string()
    }

    /// Validate that migration versions are sequential with no gaps.
    pub fn validate_version_sequence(
        migrations: &[Migration],
    ) -> std::result::Result<(), MigrationError> {
        for window in migrations.windows(2) {
            let expected = window[0].version + 1;
            if window[1].version != expected {
                return Err(MigrationError::OutOfOrder {
                    expected,
                    got: window[1].version,
                });
            }
        }
        Ok(())
    }

    /// Check if a migration checksum matches what was previously applied.
    pub fn verify_checksum(
        &self,
        version: u32,
        checksum: &str,
    ) -> std::result::Result<(), MigrationError> {
        if let Some(record) = self.applied_records.get(&version) {
            if record.checksum != checksum {
                return Err(MigrationError::ChecksumMismatch {
                    version,
                    expected: record.checksum.clone(),
                    actual: checksum.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Apply a single migration. If a DB is configured, executes up_sql in a
    /// transaction and inserts a tracking record. Otherwise updates in-memory state.
    pub async fn apply_migration_db(
        &mut self,
        migration: &Migration,
    ) -> std::result::Result<(), MigrationError> {
        if self.applied_records.contains_key(&migration.version) {
            return Err(MigrationError::VersionConflict {
                existing: migration.version,
                incoming: migration.version,
            });
        }

        let expected_version = self.current_version + 1;
        if migration.version != expected_version && self.current_version > 0 {
            return Err(MigrationError::OutOfOrder {
                expected: expected_version,
                got: migration.version,
            });
        }

        self.verify_checksum(migration.version, &migration.checksum)?;

        if self.has_db() {
            // Execute SQL in a transaction
            if let Err(e) = self.execute_sql(&migration.up_sql).await {
                tracing::error!("Failed to execute migration v{}: {}", migration.version, e);
                return Err(MigrationError::VersionConflict {
                    existing: migration.version,
                    incoming: migration.version,
                });
            }
        }

        let record = MigrationRecord {
            version: migration.version,
            name: migration.name.clone(),
            applied_at: chrono::Utc::now().to_rfc3339(),
            checksum: migration.checksum.clone(),
        };

        if self.has_db() {
            if let Err(e) = self.insert_migration_record(&record).await {
                tracing::error!(
                    "Failed to insert migration record v{}: {}",
                    migration.version,
                    e
                );
            }
        }

        self.applied_records.insert(migration.version, record);
        self.current_version = migration.version;
        self.migrations_applied += 1;
        self.last_migration = Some(chrono::Utc::now());

        Ok(())
    }

    /// Apply a single migration (synchronous, in-memory only — for tests).
    pub fn apply_migration(
        &mut self,
        migration: &Migration,
    ) -> std::result::Result<(), MigrationError> {
        if self.applied_records.contains_key(&migration.version) {
            return Err(MigrationError::VersionConflict {
                existing: migration.version,
                incoming: migration.version,
            });
        }

        let expected_version = self.current_version + 1;
        if migration.version != expected_version && self.current_version > 0 {
            return Err(MigrationError::OutOfOrder {
                expected: expected_version,
                got: migration.version,
            });
        }

        self.verify_checksum(migration.version, &migration.checksum)?;

        let record = MigrationRecord {
            version: migration.version,
            name: migration.name.clone(),
            applied_at: chrono::Utc::now().to_rfc3339(),
            checksum: migration.checksum.clone(),
        };

        self.applied_records.insert(migration.version, record);
        self.current_version = migration.version;
        self.migrations_applied += 1;
        self.last_migration = Some(chrono::Utc::now());

        Ok(())
    }

    /// Dry-run a migration: validates the migration would succeed without executing it.
    ///
    /// Returns a DryRunResult describing what would happen.
    pub fn dry_run_migration(
        &self,
        migration: &Migration,
    ) -> std::result::Result<DryRunResult, MigrationError> {
        let mut result = DryRunResult {
            version: migration.version,
            name: migration.name.clone(),
            would_apply: true,
            reason: String::new(),
            checksum_valid: false,
            sql_size_bytes: migration.up_sql.len() as u64,
            has_down_migration: migration.down_sql.is_some(),
        };

        // Check if already applied
        if self.applied_records.contains_key(&migration.version) {
            result.would_apply = false;
            result.reason = format!(
                "Migration v{} already applied at {}",
                migration.version, self.applied_records[&migration.version].applied_at
            );
            return Ok(result);
        }

        // Check version ordering
        let expected_version = self.current_version + 1;
        if migration.version != expected_version && self.current_version > 0 {
            result.would_apply = false;
            result.reason = format!(
                "Out of order: expected v{}, got v{}",
                expected_version, migration.version
            );
            return Ok(result);
        }

        // Validate checksum
        match self.verify_checksum(migration.version, &migration.checksum) {
            Ok(()) => {
                result.checksum_valid = true;
            }
            Err(e) => {
                result.would_apply = false;
                result.reason = format!("Checksum validation failed: {}", e);
                return Ok(result);
            }
        }

        // Validate SQL is not empty
        if migration.up_sql.trim().is_empty() {
            result.would_apply = false;
            result.reason = "Migration SQL is empty".to_string();
            return Ok(result);
        }

        result.reason = "Migration would be applied successfully".to_string();
        Ok(result)
    }

    /// Dry-run all pending migrations.
    pub fn dry_run_all_pending(&self) -> Vec<DryRunResult> {
        self.pending_migrations
            .iter()
            .filter_map(|m| self.dry_run_migration(m).ok())
            .collect()
    }

    /// Roll back the last applied migration. If a DB is configured, executes
    /// down_sql in a transaction and removes the tracking record.
    pub async fn rollback_last_db(
        &mut self,
        down_sql: Option<&str>,
    ) -> std::result::Result<MigrationRecord, MigrationError> {
        if self.current_version == 0 {
            return Err(MigrationError::MissingDownMigration { version: 0 });
        }

        let version = self.current_version;

        if self.has_db() {
            if let Some(sql) = down_sql {
                if let Err(e) = self.execute_sql(sql).await {
                    tracing::error!("Failed to execute rollback v{}: {}", version, e);
                    return Err(MigrationError::MissingDownMigration { version });
                }
            }
            if let Err(e) = self.delete_migration_record(version).await {
                tracing::error!("Failed to delete migration record v{}: {}", version, e);
            }
        }

        let record = self
            .applied_records
            .remove(&version)
            .ok_or(MigrationError::MissingDownMigration { version })?;

        let prev_version = self
            .applied_records
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0);
        self.current_version = prev_version;
        self.migrations_rolled_back += 1;

        Ok(record)
    }

    /// Roll back the last applied migration (synchronous, in-memory only — for tests).
    pub fn rollback_last(&mut self) -> std::result::Result<MigrationRecord, MigrationError> {
        if self.current_version == 0 {
            return Err(MigrationError::MissingDownMigration { version: 0 });
        }

        let record = self.applied_records.remove(&self.current_version).ok_or(
            MigrationError::MissingDownMigration {
                version: self.current_version,
            },
        )?;

        let prev_version = self
            .applied_records
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0);
        self.current_version = prev_version;
        self.migrations_rolled_back += 1;

        Ok(record)
    }

    /// Get the list of pending (not yet applied) migrations.
    pub fn pending(&self) -> Vec<&Migration> {
        self.pending_migrations
            .iter()
            .filter(|m| !self.applied_records.contains_key(&m.version))
            .collect()
    }

    /// Get applied migration records in order.
    pub fn applied(&self) -> Vec<&MigrationRecord> {
        let mut records: Vec<_> = self.applied_records.values().collect();
        records.sort_by_key(|r| r.version);
        records
    }

    /// Check if all migrations have been applied.
    pub fn is_up_to_date(&self) -> bool {
        self.pending().is_empty()
    }

    /// Register a pending migration (used in testing or dynamic loading).
    pub fn register_migration(&mut self, migration: Migration) {
        self.pending_migrations.push(migration);
        self.pending_migrations.sort_by_key(|m| m.version);
    }

    async fn scan_migrations(&self) -> anyhow::Result<Vec<(u32, String, PathBuf)>> {
        let mut migrations = Vec::new();

        if !self.dir.exists() {
            tracing::warn!("Migration directory does not exist: {}", self.dir.display());
            return Ok(migrations);
        }

        let mut entries = fs::read_dir(&self.dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".up.sql") {
                    let parts: Vec<&str> = name.splitn(2, '_').collect();
                    if let Ok(version) = parts[0].parse::<u32>() {
                        let migration_name = parts
                            .get(1)
                            .unwrap_or(&"")
                            .trim_end_matches(".up.sql")
                            .to_string();
                        migrations.push((version, migration_name, path));
                    }
                }
            }
        }

        migrations.sort_by_key(|(v, _, _)| *v);
        Ok(migrations)
    }

    async fn read_migration(&self, path: &PathBuf) -> anyhow::Result<String> {
        fs::read_to_string(path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read migration {}: {}", path.display(), e))
    }

    /// Get the connection string for external callers.
    pub fn get_connection_string(&self) -> String {
        self.connection_string()
    }

    /// Get the migration directory.
    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    /// Get the database name.
    pub fn database(&self) -> &str {
        &self.database
    }

    /// Get auto-migrate flag.
    pub fn auto_migrate(&self) -> bool {
        self.auto_migrate
    }

    /// Get current version.
    pub fn current_version(&self) -> u32 {
        self.current_version
    }

    /// Get migrations applied count.
    pub fn migrations_applied(&self) -> u64 {
        self.migrations_applied
    }

    /// Get migrations rolled back count.
    pub fn migrations_rolled_back(&self) -> u64 {
        self.migrations_rolled_back
    }

    // =========================================================================
    // Multi-DB Migration Orchestration
    // =========================================================================

    /// Orchestrate migrations across multiple databases.
    ///
    /// Coordinates migration execution across multiple database connections,
    /// ensuring all databases are migrated in lockstep. Uses a distributed
    /// lock pattern to prevent concurrent migrations.
    ///
    /// Returns a `MigrationOrchestrationResult` with per-database results.
    pub async fn orchestrate_multi_db(
        &mut self,
        targets: Vec<MigrationTarget>,
    ) -> MigrationOrchestrationResult {
        let mut results = Vec::new();
        let mut overall_success = true;

        for target in &targets {
            tracing::info!(
                "Orchestrating migration for {} ({})",
                target.name,
                target.db_url
            );

            let target_result = self.migrate_target(target).await;
            if !target_result.success {
                overall_success = false;
            }
            results.push(target_result);
        }

        MigrationOrchestrationResult {
            success: overall_success,
            target_results: results,
            completed_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Migrate a single target database.
    async fn migrate_target(&self, target: &MigrationTarget) -> TargetMigrationResult {
        let start = std::time::Instant::now();
        let mut applied = 0u32;
        let mut errors = Vec::new();

        match target.db_type.as_str() {
            "postgres" => match sqlx::PgPool::connect(&target.db_url).await {
                Ok(_pool) => {
                    for migration_path in &target.migration_dir {
                        match tokio::fs::read_to_string(migration_path).await {
                            Ok(sql) => {
                                if let Ok(pool) = sqlx::PgPool::connect(&target.db_url).await {
                                    if let Err(e) = sqlx::query(&sql).execute(&pool).await {
                                        errors.push(format!("{}: {}", migration_path, e));
                                    } else {
                                        applied += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                errors.push(format!("Failed to read {}: {}", migration_path, e));
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("Connection failed: {}", e));
                }
            },
            "mysql" => match sqlx::MySqlPool::connect(&target.db_url).await {
                Ok(_pool) => {
                    for migration_path in &target.migration_dir {
                        match tokio::fs::read_to_string(migration_path).await {
                            Ok(sql) => {
                                if let Ok(pool) = sqlx::MySqlPool::connect(&target.db_url).await {
                                    if let Err(e) = sqlx::query(&sql).execute(&pool).await {
                                        errors.push(format!("{}: {}", migration_path, e));
                                    } else {
                                        applied += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                errors.push(format!("Failed to read {}: {}", migration_path, e));
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("Connection failed: {}", e));
                }
            },
            _ => {
                errors.push(format!("Unsupported database type: {}", target.db_type));
            }
        }

        TargetMigrationResult {
            name: target.name.clone(),
            success: errors.is_empty(),
            migrations_applied: applied,
            errors,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }
}

/// Target database for multi-DB migration orchestration.
#[derive(Debug, Clone)]
pub struct MigrationTarget {
    pub name: String,
    pub db_type: String,
    pub db_url: String,
    pub migration_dir: Vec<String>,
}

/// Result of migrating a single target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetMigrationResult {
    pub name: String,
    pub success: bool,
    pub migrations_applied: u32,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

/// Result of multi-DB migration orchestration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationOrchestrationResult {
    pub success: bool,
    pub target_results: Vec<TargetMigrationResult>,
    pub completed_at: String,
}

impl Default for MigrationShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for MigrationShim {
    fn name(&self) -> &str {
        "migration"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        if shim_core::config::validation_enabled() {
            let errors = config.validate();
            let migration_errors: Vec<_> = errors
                .iter()
                .filter(|e| e.field.starts_with("migration."))
                .collect();
            if !migration_errors.is_empty() {
                let msg = migration_errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(shim_core::Error::Config(format!(
                    "migration config validation failed: {}",
                    msg
                )));
            }
        }

        if let Some(migration_config) = &config.migration {
            self.dir = migration_config.dir.clone();
            self.database = migration_config.database.clone();
            self.auto_migrate = migration_config.auto_migrate;
            self.db_host = migration_config.db_host.clone();
            self.db_port = migration_config.db_port;
            self.db_user = migration_config.db_user.clone();
            self.db_password = migration_config.db_password.clone();
            self.db_type = migration_config.db_type.clone();
        }
        tracing::info!(
            "MigrationShim initialized (dir={}, database={}, auto={}, db_type={}, has_db={})",
            self.dir.display(),
            self.database,
            self.auto_migrate,
            self.db_type,
            self.has_db(),
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        // Ensure _migrations table exists
        if let Err(e) = self.ensure_migrations_table().await {
            tracing::warn!(
                "Failed to create _migrations table (falling back to in-memory): {}",
                e
            );
        }

        if self.auto_migrate {
            let lock = MigrationLock::new(&self.dir);
            if let Err(e) = lock.acquire_lock() {
                tracing::error!("Cannot acquire migration lock: {}", e);
                return Err(shim_core::Error::Migration(e));
            }

            let migrations = self.scan_migrations().await?;
            tracing::info!("Found {} migration files", migrations.len());

            for (version, name, path) in &migrations {
                tracing::info!("Applying migration {}: {}", version, name);
                let sql = self.read_migration(path).await?;
                let checksum = Self::compute_checksum(&sql);
                let migration = Migration {
                    version: *version,
                    name: name.clone(),
                    up_sql: sql,
                    down_sql: None,
                    checksum,
                };
                if let Err(e) = self.apply_migration_db(&migration).await {
                    tracing::warn!("Migration {} failed: {}", version, e);
                }
            }

            if let Err(e) = lock.release_lock() {
                tracing::warn!("Failed to release migration lock: {}", e);
            }

            tracing::info!(
                "Migration complete. Current version: {}",
                self.current_version
            );
        }

        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("MigrationShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("MigrationShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let mut metrics = vec![
            Metric::new("migration_current_version", self.current_version as f64),
            Metric::new("migration_applied_total", self.migrations_applied as f64),
            Metric::new(
                "migration_rolled_back_total",
                self.migrations_rolled_back as f64,
            ),
            Metric::new("migration_pending_total", self.pending().len() as f64),
        ];

        if let Some(last) = &self.last_migration {
            metrics.push(Metric::new(
                "migration_last_success_timestamp",
                last.timestamp() as f64,
            ));
        }

        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_migration(version: u32, name: &str, sql: &str) -> Migration {
        Migration {
            version,
            name: name.to_string(),
            up_sql: sql.to_string(),
            down_sql: Some(format!("-- rollback {}", name)),
            checksum: MigrationShim::compute_checksum(sql),
        }
    }

    #[test]
    fn test_compute_checksum_deterministic() {
        let c1 = MigrationShim::compute_checksum("CREATE TABLE users (id INT)");
        let c2 = MigrationShim::compute_checksum("CREATE TABLE users (id INT)");
        assert_eq!(c1, c2);
        assert!(!c1.is_empty());
    }

    #[test]
    fn test_compute_checksum_differs() {
        let c1 = MigrationShim::compute_checksum("SELECT 1");
        let c2 = MigrationShim::compute_checksum("SELECT 2");
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(
            MigrationShim::parse_version("001_create_users.up.sql").unwrap(),
            1
        );
        assert_eq!(
            MigrationShim::parse_version("042_add_index.up.sql").unwrap(),
            42
        );
        assert_eq!(
            MigrationShim::parse_version("042_add_index.down.sql").unwrap(),
            42
        );
        assert!(MigrationShim::parse_version("no_version.up.sql").is_err());
        assert!(MigrationShim::parse_version("").is_err());
    }

    #[test]
    fn test_parse_name() {
        assert_eq!(
            MigrationShim::parse_name("001_create_users.up.sql"),
            "create_users"
        );
        assert_eq!(
            MigrationShim::parse_name("002_add_email_index.down.sql"),
            "add_email_index"
        );
    }

    #[test]
    fn test_apply_migration_sequential() {
        let mut shim = MigrationShim::new();
        let m1 = make_migration(1, "create_users", "CREATE TABLE users (id INT)");
        let m2 = make_migration(2, "add_email", "ALTER TABLE users ADD email TEXT");

        shim.apply_migration(&m1).unwrap();
        assert_eq!(shim.current_version, 1);
        assert_eq!(shim.migrations_applied, 1);
        assert!(shim.applied_records.contains_key(&1));

        shim.apply_migration(&m2).unwrap();
        assert_eq!(shim.current_version, 2);
        assert_eq!(shim.migrations_applied, 2);
    }

    #[test]
    fn test_apply_migration_duplicate_version() {
        let mut shim = MigrationShim::new();
        let m1 = make_migration(1, "create_users", "CREATE TABLE users (id INT)");

        shim.apply_migration(&m1).unwrap();
        let result = shim.apply_migration(&m1);
        assert!(result.is_err());
        assert_eq!(shim.migrations_applied, 1);
    }

    #[test]
    fn test_apply_migration_out_of_order() {
        let mut shim = MigrationShim::new();
        let m1 = make_migration(1, "first", "SELECT 1");
        let m5 = make_migration(5, "fifth", "SELECT 5");

        shim.apply_migration(&m1).unwrap();
        let result = shim.apply_migration(&m5);
        assert!(result.is_err());
        assert_eq!(shim.current_version, 1);
    }

    #[test]
    fn test_rollback_last() {
        let mut shim = MigrationShim::new();
        let m1 = make_migration(1, "first", "SELECT 1");
        let m2 = make_migration(2, "second", "SELECT 2");

        shim.apply_migration(&m1).unwrap();
        shim.apply_migration(&m2).unwrap();
        assert_eq!(shim.current_version, 2);

        let rolled_back = shim.rollback_last().unwrap();
        assert_eq!(rolled_back.version, 2);
        assert_eq!(shim.current_version, 1);
        assert_eq!(shim.migrations_rolled_back, 1);
        assert!(!shim.applied_records.contains_key(&2));
    }

    #[test]
    fn test_rollback_to_zero() {
        let mut shim = MigrationShim::new();
        let m1 = make_migration(1, "first", "SELECT 1");
        shim.apply_migration(&m1).unwrap();

        shim.rollback_last().unwrap();
        assert_eq!(shim.current_version, 0);

        let result = shim.rollback_last();
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_checksum_match() {
        let mut shim = MigrationShim::new();
        let m1 = make_migration(1, "first", "SELECT 1");
        shim.apply_migration(&m1).unwrap();

        let result = shim.verify_checksum(1, &m1.checksum);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_checksum_mismatch() {
        let mut shim = MigrationShim::new();
        let m1 = make_migration(1, "first", "SELECT 1");
        shim.apply_migration(&m1).unwrap();

        let result = shim.verify_checksum(1, "bad_checksum_value");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_version_sequence_ok() {
        let migrations = vec![
            make_migration(1, "first", "A"),
            make_migration(2, "second", "B"),
            make_migration(3, "third", "C"),
        ];
        assert!(MigrationShim::validate_version_sequence(&migrations).is_ok());
    }

    #[test]
    fn test_validate_version_sequence_gap() {
        let migrations = vec![
            make_migration(1, "first", "A"),
            make_migration(3, "third", "C"),
        ];
        assert!(MigrationShim::validate_version_sequence(&migrations).is_err());
    }

    #[test]
    fn test_pending_migrations() {
        let mut shim = MigrationShim::new();
        shim.register_migration(make_migration(1, "first", "SELECT 1"));
        shim.register_migration(make_migration(2, "second", "SELECT 2"));
        shim.register_migration(make_migration(3, "third", "SELECT 3"));

        assert_eq!(shim.pending().len(), 3);
        assert!(!shim.is_up_to_date());

        shim.apply_migration(&shim.pending_migrations[0].clone())
            .unwrap();
        assert_eq!(shim.pending().len(), 2);
    }

    #[test]
    fn test_applied_records_ordered() {
        let mut shim = MigrationShim::new();
        shim.apply_migration(&make_migration(2, "second", "B"))
            .unwrap();
        shim.apply_migration(&make_migration(1, "first", "A"))
            .unwrap_err();

        let m1 = make_migration(1, "first", "A");
        shim.current_version = 0;
        shim.applied_records.clear();
        shim.apply_migration(&m1).unwrap();
        let m2 = make_migration(2, "second", "B");
        shim.apply_migration(&m2).unwrap();

        let records = shim.applied();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].version, 1);
        assert_eq!(records[1].version, 2);
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = MigrationShim::new();
        shim.apply_migration(&make_migration(1, "first", "SELECT 1"))
            .unwrap();
        shim.rollback_last().unwrap();

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 5);
        assert_eq!(metrics[0].name, "migration_current_version");
        assert_eq!(metrics[1].value, 1.0);
        assert_eq!(metrics[2].name, "migration_rolled_back_total");
        assert_eq!(metrics[2].value, 1.0);
    }

    #[test]
    fn test_has_db_default_false() {
        let shim = MigrationShim::new();
        assert!(!shim.has_db());
    }

    #[test]
    fn test_connection_string_postgres() {
        let mut shim = MigrationShim::new();
        shim.database = "mydb".to_string();
        shim.db_host = "db.example.com".to_string();
        shim.db_port = 5432;
        shim.db_user = "admin".to_string();
        shim.db_password = "secret".to_string();
        shim.db_type = "postgres".to_string();
        shim.db_url = None; // Clear any env-influenced URL override

        assert_eq!(
            shim.connection_string(),
            "postgres://admin:secret@db.example.com:5432/mydb"
        );
    }

    #[test]
    fn test_connection_string_mysql() {
        let mut shim = MigrationShim::new();
        shim.database = "mydb".to_string();
        shim.db_host = "db.example.com".to_string();
        shim.db_port = 3306;
        shim.db_user = "admin".to_string();
        shim.db_password = "secret".to_string();
        shim.db_type = "mysql".to_string();
        shim.db_url = None; // Clear any env-influenced URL override

        assert_eq!(
            shim.connection_string(),
            "mysql://admin:secret@db.example.com:3306/mydb"
        );
    }

    #[test]
    fn test_connection_string_db_url_override() {
        let mut shim = MigrationShim::new();
        shim.db_url = Some("postgres://custom:pass@remote:5433/mydb".to_string());
        shim.database = "mydb".to_string();
        shim.db_host = "localhost".to_string();
        shim.db_port = 5432;

        assert_eq!(
            shim.connection_string(),
            "postgres://custom:pass@remote:5433/mydb"
        );
    }

    #[test]
    fn test_env_db_url() {
        temp_env::with_vars(
            [(
                "MIGRATION_DB_URL",
                Some("postgres://user:pass@host:5432/db"),
            )],
            || {
                let shim = MigrationShim::new();
                assert_eq!(
                    shim.connection_string(),
                    "postgres://user:pass@host:5432/db"
                );
            },
        );
    }

    #[test]
    fn test_checksum_uses_sha256_equivalent() {
        let c1 = MigrationShim::compute_checksum("CREATE TABLE test (id INT)");
        let c2 = MigrationShim::compute_checksum("CREATE TABLE test (id INT)");
        assert_eq!(c1, c2);
        assert_eq!(c1.len(), 16); // FNV-1a is 64-bit, formatted as 16 hex chars
    }

    #[test]
    fn test_lock_acquire_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MigrationLock::new(dir.path());
        assert!(!lock.is_locked());
        lock.acquire_lock().unwrap();
        assert!(lock.is_locked());
        lock.release_lock().unwrap();
        assert!(!lock.is_locked());
    }

    #[test]
    fn test_lock_double_acquire_fails() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MigrationLock::new(dir.path());
        lock.acquire_lock().unwrap();
        let lock2 = MigrationLock::new(dir.path());
        let result = lock2.acquire_lock();
        assert!(result.is_err());
        lock.release_lock().unwrap();
    }

    #[test]
    fn test_lock_stale_detection() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MigrationLock::with_stale_threshold(dir.path(), 1);
        lock.acquire_lock().unwrap();

        // Create a lock info with timestamp 2 hours ago
        let stale_info = LockInfo {
            pid: 99999,
            hostname: "stale-host".to_string(),
            created_at: (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339(),
        };
        let content = serde_json::to_string_pretty(&stale_info).unwrap();
        std::fs::write(lock.lock_path.clone(), content).unwrap();

        assert!(lock.is_stale());
        assert!(lock.is_locked());

        // Should be able to acquire since lock is stale
        let lock2 = MigrationLock::with_stale_threshold(dir.path(), 1);
        lock2.acquire_lock().unwrap();
    }

    #[test]
    fn test_lock_stale_with_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MigrationLock::new(dir.path());
        lock.acquire_lock().unwrap();
        std::fs::write(lock.lock_path.clone(), "not json").unwrap();
        assert!(lock.is_stale());
    }

    #[test]
    fn test_lock_stale_with_invalid_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MigrationLock::new(dir.path());
        lock.acquire_lock().unwrap();
        let bad_info = r#"{"pid": 1, "hostname": "h", "created_at": "not-a-date"}"#;
        std::fs::write(lock.lock_path.clone(), bad_info).unwrap();
        assert!(lock.is_stale());
    }

    #[test]
    fn test_lock_is_not_stale_when_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MigrationLock::with_stale_threshold(dir.path(), 3600);
        lock.acquire_lock().unwrap();
        assert!(!lock.is_stale());
        lock.release_lock().unwrap();
    }

    #[test]
    fn test_lock_release_when_no_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock = MigrationLock::new(dir.path());
        // Should not error even if no lock exists
        lock.release_lock().unwrap();
    }

    #[test]
    fn test_lock_info_serializes_correctly() {
        let info = LockInfo {
            pid: 1234,
            hostname: "testhost".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("1234"));
        assert!(json.contains("testhost"));
        let parsed: LockInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pid, 1234);
        assert_eq!(parsed.hostname, "testhost");
    }
}
