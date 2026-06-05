//! Configuration types for shims.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Main configuration for the shim.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Health check configuration.
    #[serde(default)]
    pub health: HealthConfig,

    /// Vault configuration.
    #[serde(default)]
    pub vault: Option<VaultConfig>,

    /// Backup configuration.
    #[serde(default)]
    pub backup: Option<BackupConfig>,

    /// Migration configuration.
    #[serde(default)]
    pub migration: Option<MigrationConfig>,

    /// Audit configuration.
    #[serde(default)]
    pub audit: Option<AuditConfig>,

    /// TLS configuration.
    #[serde(default)]
    pub tls: Option<TlsConfig>,

    /// Failover configuration.
    #[serde(default)]
    pub failover: Option<FailoverConfig>,

    /// Replication configuration.
    #[serde(default)]
    pub replication: Option<ReplicationConfig>,

    /// Process configuration.
    #[serde(default)]
    pub process: ProcessConfig,
}

/// Health check configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Command to check liveness.
    #[serde(default = "default_liveness_cmd")]
    pub liveness_cmd: String,

    /// Command to check readiness.
    #[serde(default = "default_readiness_cmd")]
    pub readiness_cmd: String,

    /// Listen address for health endpoint.
    #[serde(default = "default_health_listen")]
    pub listen: String,

    /// Check interval in seconds.
    #[serde(default = "default_check_interval")]
    pub interval_secs: u64,

    /// Timeout for health checks in seconds.
    #[serde(default = "default_check_timeout")]
    pub timeout_secs: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            liveness_cmd: default_liveness_cmd(),
            readiness_cmd: default_readiness_cmd(),
            listen: default_health_listen(),
            interval_secs: default_check_interval(),
            timeout_secs: default_check_timeout(),
        }
    }
}

fn default_liveness_cmd() -> String {
    "exec:true".to_string()
}

fn default_readiness_cmd() -> String {
    "exec:true".to_string()
}

fn default_health_listen() -> String {
    "0.0.0.0:9101".to_string()
}

fn default_check_interval() -> u64 {
    10
}

fn default_check_timeout() -> u64 {
    5
}

/// Vault configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    /// Vault address.
    pub addr: String,

    /// Vault role.
    pub role: String,

    /// Secret path.
    pub secret: String,

    /// Rotation interval in seconds.
    #[serde(default = "default_rotation_interval")]
    pub rotation_secs: u64,
}

fn default_rotation_interval() -> u64 {
    3600
}

/// Backup configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Cron schedule for backups.
    pub schedule: String,

    /// Storage backend (s3, local).
    #[serde(default)]
    pub storage: String,

    /// Retention period in days.
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,

    /// Database to backup.
    #[serde(default)]
    pub database: String,

    /// Backup prefix/path.
    #[serde(default)]
    pub prefix: String,
}

fn default_retention_days() -> u32 {
    30
}

/// Migration configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    /// Migration directory.
    pub dir: PathBuf,

    /// Database to migrate.
    pub database: String,

    /// Auto-migrate on startup.
    #[serde(default)]
    pub auto_migrate: bool,

    /// Database host (for SQL execution).
    #[serde(default = "default_migration_db_host")]
    pub db_host: String,

    /// Database port (for SQL execution).
    #[serde(default = "default_migration_db_port")]
    pub db_port: u16,

    /// Database user (for SQL execution).
    #[serde(default = "default_migration_db_user")]
    pub db_user: String,

    /// Database password (for SQL execution).
    #[serde(default)]
    pub db_password: String,

    /// Database type: postgres, mysql.
    #[serde(default = "default_migration_db_type")]
    pub db_type: String,
}

fn default_migration_db_host() -> String {
    "127.0.0.1".to_string()
}

fn default_migration_db_port() -> u16 {
    5432
}

fn default_migration_db_user() -> String {
    "postgres".to_string()
}

fn default_migration_db_type() -> String {
    "postgres".to_string()
}

/// Audit configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Database to audit.
    pub database: String,

    /// Tables to audit (empty = all).
    #[serde(default)]
    pub tables: Vec<String>,

    /// Output format (json, syslog).
    #[serde(default = "default_audit_format")]
    pub format: String,
}

fn default_audit_format() -> String {
    "json".to_string()
}

/// TLS configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// TLS provider (letsencrypt, internal-ca, vault-pki).
    pub provider: String,

    /// Domain for the certificate.
    pub domain: String,

    /// Email for Let's Encrypt.
    #[serde(default)]
    pub email: String,

    /// Renew certificate before expiry.
    #[serde(default = "default_renew_before")]
    pub renew_before_secs: u64,
}

fn default_renew_before() -> u64 {
    72 * 3600 // 72 hours
}

/// Failover configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    /// Primary database address.
    pub primary: String,

    /// Replica database address.
    pub replica: String,

    /// Check interval in seconds.
    #[serde(default = "default_failover_interval")]
    pub check_interval_secs: u64,

    /// Timeout for failover checks in seconds.
    #[serde(default = "default_failover_timeout")]
    pub timeout_secs: u64,

    /// Consecutive failures before failover.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,

    /// Webhook URL for notifications.
    #[serde(default)]
    pub webhook: Option<String>,

    /// Database type: postgres, mysql.
    #[serde(default = "default_failover_db_type")]
    pub db_type: String,
}

fn default_failover_interval() -> u64 {
    5
}

fn default_failover_timeout() -> u64 {
    10
}

fn default_failure_threshold() -> u32 {
    3
}

fn default_failover_db_type() -> String {
    "postgres".to_string()
}

/// Replication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationConfig {
    /// Primary database address.
    pub primary: String,

    /// Replica addresses (comma-separated or array).
    #[serde(default)]
    pub replicas: Vec<String>,

    /// Replication mode: synchronous, asynchronous.
    #[serde(default = "default_replication_mode")]
    pub mode: String,

    /// Health check interval in seconds.
    #[serde(default = "default_replication_check_secs")]
    pub check_interval_secs: u64,

    /// Database type: postgres, mysql.
    #[serde(default = "default_replication_db_type")]
    pub db_type: String,

    /// Replication slot name (PostgreSQL).
    #[serde(default)]
    pub slot_name: Option<String>,
}

fn default_replication_mode() -> String {
    "asynchronous".to_string()
}

fn default_replication_check_secs() -> u64 {
    10
}

fn default_replication_db_type() -> String {
    "postgres".to_string()
}

/// Process configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessConfig {
    /// Command to run as child process.
    #[serde(default)]
    pub command: String,

    /// Arguments for the command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Working directory.
    #[serde(default)]
    pub working_dir: Option<PathBuf>,

    /// Graceful shutdown timeout in seconds.
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout_secs: u64,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: vec![],
            working_dir: None,
            shutdown_timeout_secs: default_shutdown_timeout(),
        }
    }
}

fn default_shutdown_timeout() -> u64 {
    30
}

impl Config {
    /// Load configuration from file.
    pub fn from_file(path: &str) -> Result<Self, anyhow::Error> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config: {}", e))?;
        toml::from_str(&content).map_err(|e| anyhow::anyhow!("Failed to parse config: {}", e))
    }

    /// Load configuration: file first, then env var overrides (12-factor).
    ///
    /// If `SHIM_CONFIG` env var is set, load that file. Otherwise try
    /// `./shim.toml`, then fall back to env-only config.
    pub fn load() -> Self {
        let mut config = Self::from_env();

        let config_path = std::env::var("SHIM_CONFIG")
            .unwrap_or_else(|_| "shim.toml".to_string());

        if std::path::Path::new(&config_path).exists() {
            match Self::from_file(&config_path) {
                Ok(file_config) => {
                    // File is base, env overrides
                    config = file_config;
                    // Re-apply env overrides
                    let env_config = Self::from_env();
                    config.merge(env_config);
                    tracing::info!("loaded config from {}", config_path);
                }
                Err(e) => {
                    tracing::warn!("failed to load {}: {}", config_path, e);
                }
            }
        } else {
            tracing::info!("no config file found at {}, using env vars", config_path);
        }

        config
    }

    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let mut config = Config {
            health: HealthConfig::default(),
            vault: None,
            backup: None,
            migration: None,
            audit: None,
            tls: None,
            failover: None,
            replication: None,
            process: ProcessConfig::default(),
        };

        // Override with env vars
        if let Ok(cmd) = std::env::var("HEALTH_CMD") {
            config.health.liveness_cmd = cmd.clone();
            config.health.readiness_cmd = cmd;
        }
        if let Ok(addr) = std::env::var("HEALTH_LISTEN") {
            config.health.listen = addr;
        }
        if let Ok(interval) = std::env::var("HEALTH_INTERVAL_SECS") {
            match interval.parse() {
                Ok(v) => config.health.interval_secs = v,
                Err(e) => tracing::warn!("invalid HEALTH_INTERVAL_SECS value: {}", e),
            }
        }
        if let Ok(cmd) = std::env::var("PROCESS_COMMAND") {
            config.process.command = cmd;
        }
        if let Ok(args) = std::env::var("PROCESS_ARGS") {
            config.process.args = args.split_whitespace().map(String::from).collect();
        }

        config
    }

    /// Merge another config into this one (env overrides file).
    ///
    /// Uses `std::env::var().is_ok()` to check whether an env var is present,
    /// so that explicitly-set-to-same-as-default values still override.
    pub fn merge(&mut self, other: Config) {
        // Health — override if the env var is actually set
        if std::env::var("HEALTH_CMD").is_ok() {
            self.health.liveness_cmd = other.health.liveness_cmd;
            self.health.readiness_cmd = other.health.readiness_cmd;
        }
        if std::env::var("HEALTH_LISTEN").is_ok() {
            self.health.listen = other.health.listen;
        }
        if std::env::var("HEALTH_INTERVAL_SECS").is_ok() {
            self.health.interval_secs = other.health.interval_secs;
        }
        if std::env::var("HEALTH_TIMEOUT_SECS").is_ok() {
            self.health.timeout_secs = other.health.timeout_secs;
        }

        // Process — override if the env var is actually set
        if std::env::var("PROCESS_COMMAND").is_ok() {
            self.process.command = other.process.command;
        }
        if std::env::var("PROCESS_ARGS").is_ok() {
            self.process.args = other.process.args;
        }
        if other.process.working_dir.is_some() {
            self.process.working_dir = other.process.working_dir;
        }
        if std::env::var("SHUTDOWN_TIMEOUT_SECS").is_ok() {
            self.process.shutdown_timeout_secs = other.process.shutdown_timeout_secs;
        }

        // Optional configs (merge if present)
        if other.vault.is_some() {
            self.vault = other.vault;
        }
        if other.backup.is_some() {
            self.backup = other.backup;
        }
        if other.migration.is_some() {
            self.migration = other.migration;
        }
        if other.audit.is_some() {
            self.audit = other.audit;
        }
        if other.tls.is_some() {
            self.tls = other.tls;
        }
        if other.failover.is_some() {
            self.failover = other.failover;
        }
        if other.replication.is_some() {
            self.replication = other.replication;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_config_default_command_is_empty() {
        let config = ProcessConfig::default();
        assert_eq!(config.command, "");
    }

    #[test]
    fn test_config_from_file_invalid() {
        let result = Config::from_file("/nonexistent/path.toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_default_has_empty_process_command() {
        let config = Config::default();
        assert_eq!(config.process.command, "");
    }
}
