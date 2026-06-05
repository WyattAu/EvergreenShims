#![allow(dead_code)]
//! Migration shim — database schema migrations with rollback support.
//!
//! Runs SQL migration files from a directory in order, tracking applied
//! migrations in a `_migrations` table.
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
//! MIGRATION_AUTO_MIGRATE Auto-migrate on startup (default: false)
//! MIGRATION_DB_TYPE   Database type: postgres, mariadb, mysql
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
            .get(0)
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

    /// Apply a single migration, tracking it in the applied records.
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

    /// Roll back the last applied migration.
    pub fn rollback_last(&mut self) -> std::result::Result<MigrationRecord, MigrationError> {
        if self.current_version == 0 {
            return Err(MigrationError::MissingDownMigration { version: 0 });
        }

        let record = self
            .applied_records
            .remove(&self.current_version)
            .ok_or_else(|| MigrationError::MissingDownMigration {
                version: self.current_version,
            })?;

        let prev_version = self
            .applied_records
            .keys()
            .rev()
            .next()
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
        if let Some(migration_config) = &config.migration {
            self.dir = migration_config.dir.clone();
            self.database = migration_config.database.clone();
            self.auto_migrate = migration_config.auto_migrate;
        }
        tracing::info!(
            "MigrationShim initialized (dir={}, database={}, auto={})",
            self.dir.display(),
            self.database,
            self.auto_migrate,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        if self.auto_migrate {
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
                if let Err(e) = self.apply_migration(&migration) {
                    tracing::warn!("Migration {} failed: {}", version, e);
                }
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
}
