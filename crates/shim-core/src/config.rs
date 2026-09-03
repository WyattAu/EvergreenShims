//! Configuration types for shims.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A single configuration validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationError {
    /// Dotted path to the offending field (e.g. `"health.listen"`).
    pub field: String,
    /// Human-readable description of the problem.
    pub message: String,
}

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ConfigValidationError {}

/// Check whether config validation is enabled via the `SHIM_VALIDATE_CONFIG`
/// environment variable. Defaults to `true`.
pub fn validation_enabled() -> bool {
    match std::env::var("SHIM_VALIDATE_CONFIG") {
        Ok(val) => !matches!(val.as_str(), "0" | "false" | "False" | "FALSE"),
        Err(_) => true,
    }
}

/// Supported configuration schema versions.
const SUPPORTED_VERSIONS: &[&str] = &["1.0"];

/// Main configuration for the shim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Schema version of this configuration file.
    #[serde(default = "default_config_version")]
    pub version: Option<String>,

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

    /// Resource quota configuration.
    #[serde(default)]
    pub resource_quota: ResourceQuota,

    /// Multi-tenancy configuration.
    #[serde(default)]
    pub tenants: Vec<TenantConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: default_config_version(),
            health: HealthConfig::default(),
            vault: None,
            backup: None,
            migration: None,
            audit: None,
            tls: None,
            failover: None,
            replication: None,
            process: ProcessConfig::default(),
            resource_quota: ResourceQuota::default(),
            tenants: Vec::new(),
        }
    }
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

fn default_config_version() -> Option<String> {
    Some("1.0".to_string())
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

/// Per-tenant configuration for multi-tenancy isolation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantConfig {
    /// Unique tenant identifier.
    pub tenant_id: String,

    /// Maximum memory in bytes (None = no limit).
    #[serde(default)]
    pub max_memory_bytes: Option<u64>,

    /// Maximum CPU usage as a percentage (None = no limit).
    #[serde(default)]
    pub max_cpu_percent: Option<f64>,

    /// Maximum requests per second (None = no limit).
    #[serde(default)]
    pub max_requests_per_sec: Option<u32>,

    /// Features this tenant is allowed to use.
    #[serde(default)]
    pub allowed_features: Vec<String>,

    /// Resource quota for this tenant (reuses global ResourceQuota).
    #[serde(default)]
    pub quota_config: ResourceQuota,

    /// Period in seconds after which the request counter resets (default: 1).
    #[serde(default = "default_reset_period_secs")]
    pub reset_period_secs: u64,
}

fn default_reset_period_secs() -> u64 {
    1
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

    /// Startup grace period in seconds. If the child exits during this
    /// period, the shim restarts it once instead of shutting down.
    /// This prevents crash loops when apps briefly exit during init
    /// (e.g., Rust binaries, Java JVM startup).
    #[serde(default = "default_startup_grace")]
    pub startup_grace_secs: u64,
}

/// Resource quota limits for the shim process.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceQuota {
    /// Maximum memory in bytes (None = no limit).
    #[serde(default)]
    pub max_memory_bytes: Option<u64>,

    /// Maximum CPU usage as a percentage (None = no limit).
    #[serde(default)]
    pub max_cpu_percent: Option<f64>,

    /// Maximum open file descriptors (None = no limit).
    #[serde(default)]
    pub max_open_files: Option<u32>,

    /// Maximum network connections (None = no limit).
    #[serde(default)]
    pub max_connections: Option<u32>,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: vec![],
            working_dir: None,
            shutdown_timeout_secs: default_shutdown_timeout(),
            startup_grace_secs: default_startup_grace(),
        }
    }
}

fn default_shutdown_timeout() -> u64 {
    30
}

fn default_startup_grace() -> u64 {
    5
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

        let config_path = std::env::var("SHIM_CONFIG").unwrap_or_else(|_| "shim.toml".to_string());

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
            version: default_config_version(),
            health: HealthConfig::default(),
            vault: None,
            backup: None,
            migration: None,
            audit: None,
            tls: None,
            failover: None,
            replication: None,
            process: ProcessConfig::default(),
            resource_quota: ResourceQuota::default(),
            tenants: Vec::new(),
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

        // Resource quota env vars
        if let Ok(val) = std::env::var("SHIM_MAX_MEMORY_BYTES") {
            match val.parse() {
                Ok(v) => config.resource_quota.max_memory_bytes = Some(v),
                Err(e) => tracing::warn!("invalid SHIM_MAX_MEMORY_BYTES value: {}", e),
            }
        }
        if let Ok(val) = std::env::var("SHIM_MAX_CPU_PERCENT") {
            match val.parse() {
                Ok(v) => config.resource_quota.max_cpu_percent = Some(v),
                Err(e) => tracing::warn!("invalid SHIM_MAX_CPU_PERCENT value: {}", e),
            }
        }
        if let Ok(val) = std::env::var("SHIM_MAX_OPEN_FILES") {
            match val.parse() {
                Ok(v) => config.resource_quota.max_open_files = Some(v),
                Err(e) => tracing::warn!("invalid SHIM_MAX_OPEN_FILES value: {}", e),
            }
        }

        // Tenant env vars — single-tenant override from environment
        if let Ok(tenant_id) = std::env::var("SHIM_TENANT_ID") {
            let mut tenant = TenantConfig {
                tenant_id,
                max_memory_bytes: None,
                max_cpu_percent: None,
                max_requests_per_sec: None,
                allowed_features: Vec::new(),
                quota_config: ResourceQuota::default(),
                reset_period_secs: 1,
            };
            if let Ok(val) = std::env::var("SHIM_TENANT_MAX_MEMORY") {
                match val.parse() {
                    Ok(v) => tenant.max_memory_bytes = Some(v),
                    Err(e) => tracing::warn!("invalid SHIM_TENANT_MAX_MEMORY value: {}", e),
                }
            }
            if let Ok(val) = std::env::var("SHIM_TENANT_MAX_CPU") {
                match val.parse() {
                    Ok(v) => tenant.max_cpu_percent = Some(v),
                    Err(e) => tracing::warn!("invalid SHIM_TENANT_MAX_CPU value: {}", e),
                }
            }
            config.tenants.push(tenant);
        }

        config
    }

    /// Validate the entire configuration.
    ///
    /// Returns an empty `Vec` when everything is valid. Each violation is
    /// described by a [`ConfigValidationError`] carrying the dotted field path
    /// and a human-readable message.
    pub fn validate(&self) -> Vec<ConfigValidationError> {
        let mut errors = Vec::new();

        // --- version ---
        if let Some(ref version) = self.version {
            if !SUPPORTED_VERSIONS.contains(&version.as_str()) {
                errors.push(ConfigValidationError {
                    field: "version".into(),
                    message: format!(
                        "unknown config version '{}'; supported versions: {:?}",
                        version, SUPPORTED_VERSIONS
                    ),
                });
            }
        }

        // --- health ---
        if self.health.listen.parse::<std::net::SocketAddr>().is_err() {
            errors.push(ConfigValidationError {
                field: "health.listen".into(),
                message: format!(
                    "'{}' is not a valid socket address (expected e.g. 0.0.0.0:9101)",
                    self.health.listen
                ),
            });
        }
        if self.health.interval_secs == 0 {
            errors.push(ConfigValidationError {
                field: "health.interval_secs".into(),
                message: "must be greater than 0".into(),
            });
        }
        if self.health.timeout_secs == 0 {
            errors.push(ConfigValidationError {
                field: "health.timeout_secs".into(),
                message: "must be greater than 0".into(),
            });
        }

        // --- vault ---
        if let Some(ref vault) = self.vault {
            if validkit::HttpsUrl::try_from(vault.addr.as_str()).is_err() {
                errors.push(ConfigValidationError {
                    field: "vault.addr".into(),
                    message: format!(
                        "'{}' is not a valid URL (must be https:// without credentials)",
                        vault.addr
                    ),
                });
            }
            if vault.rotation_secs == 0 {
                errors.push(ConfigValidationError {
                    field: "vault.rotation_secs".into(),
                    message: "must be greater than 0".into(),
                });
            }
        }

        // --- backup ---
        if let Some(ref backup) = self.backup {
            if backup.retention_days == 0 {
                errors.push(ConfigValidationError {
                    field: "backup.retention_days".into(),
                    message: "must be greater than 0".into(),
                });
            }
            // --- backup.schedule: validate cron is actually parseable ---
            // `cron::Schedule` (6/7-field, seconds-precision) is the same
            // parser the scheduler uses at runtime. Do not also validate with
            // `validkit::CronExpr` (strict 5-field): no expression satisfies
            // both grammars.
            if !backup.schedule.is_empty() {
                if let Err(e) = backup.schedule.parse::<cron::Schedule>() {
                    errors.push(ConfigValidationError {
                        field: "backup.schedule".into(),
                        message: format!("cron parse error: {}", e),
                    });
                }
            }
        }

        // --- migration ---
        if let Some(ref migration) = self.migration {
            let path = &migration.dir;
            // NOTE: Directory creation is the caller's responsibility; validate() only checks existence.
            if !path.exists() {
                errors.push(ConfigValidationError {
                    field: "migration.dir".into(),
                    message: format!("directory '{}' does not exist", path.display()),
                });
            }
            if migration.db_port == 0 {
                errors.push(ConfigValidationError {
                    field: "migration.db_port".into(),
                    message: "must be greater than 0".into(),
                });
            }
        }

        // --- tls ---
        if let Some(ref tls) = self.tls {
            if tls.domain.is_empty() {
                errors.push(ConfigValidationError {
                    field: "tls.domain".into(),
                    message: "must not be empty".into(),
                });
            }
            if tls.renew_before_secs == 0 {
                errors.push(ConfigValidationError {
                    field: "tls.renew_before_secs".into(),
                    message: "must be greater than 0".into(),
                });
            }
        }

        // --- failover ---
        if let Some(ref failover) = self.failover {
            if failover.check_interval_secs == 0 {
                errors.push(ConfigValidationError {
                    field: "failover.check_interval_secs".into(),
                    message: "must be greater than 0".into(),
                });
            }
            if failover.timeout_secs == 0 {
                errors.push(ConfigValidationError {
                    field: "failover.timeout_secs".into(),
                    message: "must be greater than 0".into(),
                });
            }
            if failover.failure_threshold == 0 {
                errors.push(ConfigValidationError {
                    field: "failover.failure_threshold".into(),
                    message: "must be greater than 0".into(),
                });
            }
        }

        // --- tenants ---
        let mut seen_tenant_ids = std::collections::HashSet::new();
        for (i, tenant) in self.tenants.iter().enumerate() {
            if tenant.tenant_id.is_empty() {
                errors.push(ConfigValidationError {
                    field: format!("tenants[{}].tenant_id", i),
                    message: "must not be empty".into(),
                });
            }
            if !seen_tenant_ids.insert(&tenant.tenant_id) {
                errors.push(ConfigValidationError {
                    field: format!("tenants[{}].tenant_id", i),
                    message: format!("duplicate tenant_id '{}'", tenant.tenant_id),
                });
            }
            if let Some(cpu) = tenant.max_cpu_percent {
                if !(0.0..=100.0).contains(&cpu) {
                    errors.push(ConfigValidationError {
                        field: format!("tenants[{}].max_cpu_percent", i),
                        message: format!("{} is not in range 0.0-100.0", cpu),
                    });
                }
            }
        }

        errors
    }

    /// Validate the configuration schema with strict type and range checks.
    ///
    /// This method validates beyond what [`validate`](Self::validate) checks:
    /// - Port numbers are within 1..=65535
    /// - Cron expressions are parseable (not just 5 fields)
    /// - File paths are non-empty
    /// - URL schemes are strictly http or https
    /// - Numeric ranges are within valid bounds
    pub fn validate_schema(&self) -> Vec<ConfigValidationError> {
        let mut errors = self.validate();

        // --- health.listen: extract and validate port range ---
        if let Ok(addr) = self.health.listen.parse::<std::net::SocketAddr>() {
            let port = addr.port();
            if port == 0 {
                errors.push(ConfigValidationError {
                    field: "health.listen".into(),
                    message: format!("port {} is out of range 1-65535", port),
                });
            }
        }

        // --- migration.db_port: strict range check ---
        if let Some(ref migration) = self.migration {
            if migration.db_port == 0 {
                errors.push(ConfigValidationError {
                    field: "migration.db_port".into(),
                    message: format!("port {} is out of valid range 1-65535", migration.db_port),
                });
            }
        }

        // --- backup.storage: must not be empty ---
        if let Some(ref backup) = self.backup {
            if backup.storage.is_empty() {
                errors.push(ConfigValidationError {
                    field: "backup.storage".into(),
                    message: "storage backend must not be empty".into(),
                });
            }
        }

        // --- vault.addr: strict URL scheme validation (migrated to validkit) ---
        if let Some(ref vault) = self.vault {
            if let Err(e) = validkit::HttpsUrl::try_from(vault.addr.as_str()) {
                errors.push(ConfigValidationError {
                    field: "vault.addr".into(),
                    message: format!("invalid vault URL: {}", e),
                });
            }
        }

        // --- failover: webhook URL scheme validation (migrated to validkit) ---
        if let Some(ref failover) = self.failover {
            if let Some(ref webhook) = failover.webhook {
                if !webhook.is_empty() {
                    if let Err(e) = validkit::HttpsUrl::try_from(webhook.as_str()) {
                        errors.push(ConfigValidationError {
                            field: "failover.webhook".into(),
                            message: format!("invalid webhook URL: {}", e),
                        });
                    }
                }
            }
        }

        // --- migration.dir: must be non-empty path ---
        if let Some(ref migration) = self.migration {
            if migration.dir.as_os_str().is_empty() {
                errors.push(ConfigValidationError {
                    field: "migration.dir".into(),
                    message: "migration directory path must not be empty".into(),
                });
            }
        }

        // --- process.command: must not be empty ---
        if self.process.command.is_empty() && self.process.working_dir.is_some() {
            errors.push(ConfigValidationError {
                field: "process.command".into(),
                message: "command must not be empty when working_dir is set".into(),
            });
        }

        errors
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

        // Resource quota — override if any env var is set
        if std::env::var("SHIM_MAX_MEMORY_BYTES").is_ok() {
            self.resource_quota.max_memory_bytes = other.resource_quota.max_memory_bytes;
        }
        if std::env::var("SHIM_MAX_CPU_PERCENT").is_ok() {
            self.resource_quota.max_cpu_percent = other.resource_quota.max_cpu_percent;
        }
        if std::env::var("SHIM_MAX_OPEN_FILES").is_ok() {
            self.resource_quota.max_open_files = other.resource_quota.max_open_files;
        }

        // Tenants — merge: env-sourced tenants append to file-sourced tenants,
        // deduplicating by tenant_id (env wins on conflict).
        if std::env::var("SHIM_TENANT_ID").is_ok() {
            for tenant in other.tenants {
                self.tenants.retain(|t| t.tenant_id != tenant.tenant_id);
                self.tenants.push(tenant);
            }
        } else if !other.tenants.is_empty() && self.tenants.is_empty() {
            self.tenants = other.tenants;
        }
    }
}

#[allow(dead_code)]
fn is_valid_url(s: &str) -> bool {
    // Migrated to validkit: HttpsUrl enforces https-only, no credentials, host required.
    // Kept as bool wrapper for backwards compat; callers should prefer HttpsUrl::try_from directly.
    validkit::HttpsUrl::try_from(s).is_ok()
}

#[allow(dead_code)]
fn is_valid_cron(expr: &str) -> bool {
    // Migrated to validkit: CronExpr validates 5-field cron with regex/syntax checks.
    // ValidError is mapped to bool for backwards compat.
    validkit::CronExpr::try_from(expr).is_ok()
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use serial_test::serial;

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

    // --- validation tests ---

    #[test]
    fn test_valid_default_config() {
        let config = Config::default();
        assert!(config.validate().is_empty());
    }

    #[test]
    fn test_invalid_health_listen() {
        let mut config = Config::default();
        config.health.listen = "not-an-address".into();
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "health.listen"));
    }

    #[test]
    fn test_invalid_health_interval_zero() {
        let mut config = Config::default();
        config.health.interval_secs = 0;
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "health.interval_secs"));
    }

    #[test]
    fn test_invalid_health_timeout_zero() {
        let mut config = Config::default();
        config.health.timeout_secs = 0;
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "health.timeout_secs"));
    }

    #[test]
    fn test_invalid_vault_addr_not_url() {
        let mut config = Config::default();
        config.vault = Some(VaultConfig {
            addr: "ftp://vault.local".into(),
            role: "test".into(),
            secret: "secret/data".into(),
            rotation_secs: 3600,
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "vault.addr"));
    }

    #[test]
    fn test_invalid_vault_rotation_zero() {
        let mut config = Config::default();
        config.vault = Some(VaultConfig {
            addr: "https://vault.local".into(),
            role: "test".into(),
            secret: "secret/data".into(),
            rotation_secs: 0,
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "vault.rotation_secs"));
    }

    #[test]
    fn test_valid_vault_config() {
        let mut config = Config::default();
        config.vault = Some(VaultConfig {
            addr: "https://vault.local".into(),
            role: "test".into(),
            secret: "secret/data".into(),
            rotation_secs: 60,
        });
        assert!(config.validate().is_empty());
    }

    #[test]
    fn test_invalid_backup_retention_zero() {
        let mut config = Config::default();
        config.backup = Some(BackupConfig {
            schedule: "0 2 * * *".into(),
            storage: "s3".into(),
            retention_days: 0,
            database: "mydb".into(),
            prefix: "".into(),
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "backup.retention_days"));
    }

    #[test]
    fn test_invalid_backup_schedule_bad_cron() {
        let mut config = Config::default();
        config.backup = Some(BackupConfig {
            schedule: "not-a-cron".into(),
            storage: "local".into(),
            retention_days: 7,
            database: "mydb".into(),
            prefix: "".into(),
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "backup.schedule"));
    }

    #[test]
    fn test_valid_backup_empty_schedule() {
        let mut config = Config::default();
        config.backup = Some(BackupConfig {
            schedule: "".into(),
            storage: "local".into(),
            retention_days: 30,
            database: "mydb".into(),
            prefix: "".into(),
        });
        assert!(config.validate().is_empty());
    }

    #[test]
    fn test_invalid_migration_db_port_zero() {
        let mut config = Config::default();
        config.migration = Some(MigrationConfig {
            dir: PathBuf::from("/tmp/test-migrations"),
            database: "mydb".into(),
            auto_migrate: false,
            db_host: "127.0.0.1".into(),
            db_port: 0,
            db_user: "postgres".into(),
            db_password: "".into(),
            db_type: "postgres".into(),
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "migration.db_port"));
    }

    #[test]
    fn test_invalid_tls_domain_empty() {
        let mut config = Config::default();
        config.tls = Some(TlsConfig {
            provider: "letsencrypt".into(),
            domain: "".into(),
            email: "test@example.com".into(),
            renew_before_secs: 86400,
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "tls.domain"));
    }

    #[test]
    fn test_invalid_tls_renew_before_zero() {
        let mut config = Config::default();
        config.tls = Some(TlsConfig {
            provider: "letsencrypt".into(),
            domain: "example.com".into(),
            email: "test@example.com".into(),
            renew_before_secs: 0,
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "tls.renew_before_secs"));
    }

    #[test]
    fn test_invalid_failover_check_interval_zero() {
        let mut config = Config::default();
        config.failover = Some(FailoverConfig {
            primary: "10.0.0.1:5432".into(),
            replica: "10.0.0.2:5432".into(),
            check_interval_secs: 0,
            timeout_secs: 10,
            failure_threshold: 3,
            webhook: None,
            db_type: "postgres".into(),
        });
        let errors = config.validate();
        assert!(errors
            .iter()
            .any(|e| e.field == "failover.check_interval_secs"));
    }

    #[test]
    fn test_invalid_failover_timeout_zero() {
        let mut config = Config::default();
        config.failover = Some(FailoverConfig {
            primary: "10.0.0.1:5432".into(),
            replica: "10.0.0.2:5432".into(),
            check_interval_secs: 5,
            timeout_secs: 0,
            failure_threshold: 3,
            webhook: None,
            db_type: "postgres".into(),
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "failover.timeout_secs"));
    }

    #[test]
    fn test_invalid_failover_threshold_zero() {
        let mut config = Config::default();
        config.failover = Some(FailoverConfig {
            primary: "10.0.0.1:5432".into(),
            replica: "10.0.0.2:5432".into(),
            check_interval_secs: 5,
            timeout_secs: 10,
            failure_threshold: 0,
            webhook: None,
            db_type: "postgres".into(),
        });
        let errors = config.validate();
        assert!(errors
            .iter()
            .any(|e| e.field == "failover.failure_threshold"));
    }

    #[test]
    fn test_multiple_errors_collected() {
        let mut config = Config::default();
        config.health.listen = "bad".into();
        config.health.interval_secs = 0;
        config.health.timeout_secs = 0;
        let errors = config.validate();
        assert_eq!(errors.len(), 3);
    }

    #[test]
    fn test_validation_error_display() {
        let err = ConfigValidationError {
            field: "health.listen".into(),
            message: "bad value".into(),
        };
        assert_eq!(err.to_string(), "health.listen: bad value");
    }

    #[test]
    fn test_vault_url_valid() {
        let mut config = Config::default();
        config.vault = Some(VaultConfig {
            addr: "https://vault.example.com:8200".into(),
            role: "test".into(),
            secret: "secret/data".into(),
            rotation_secs: 3600,
        });
        assert!(config.validate().is_empty());
    }

    #[test]
    fn test_health_valid_custom_listen() {
        let mut config = Config::default();
        config.health.listen = "127.0.0.1:8080".into();
        config.health.interval_secs = 30;
        config.health.timeout_secs = 15;
        assert!(config.validate().is_empty());
    }

    // --- resource quota tests ---

    #[test]
    fn test_resource_quota_default() {
        let quota = ResourceQuota::default();
        assert!(quota.max_memory_bytes.is_none());
        assert!(quota.max_cpu_percent.is_none());
        assert!(quota.max_open_files.is_none());
        assert!(quota.max_connections.is_none());
    }

    #[test]
    fn test_resource_quota_serialization() {
        let quota = ResourceQuota {
            max_memory_bytes: Some(1024 * 1024 * 1024),
            max_cpu_percent: Some(80.0),
            max_open_files: Some(1024),
            max_connections: Some(100),
        };
        let json = serde_json::to_string(&quota).unwrap();
        assert!(json.contains("max_memory_bytes"));
        assert!(json.contains("max_cpu_percent"));

        let deser: ResourceQuota = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.max_memory_bytes, Some(1024 * 1024 * 1024));
        assert_eq!(deser.max_cpu_percent, Some(80.0));
        assert_eq!(deser.max_open_files, Some(1024));
        assert_eq!(deser.max_connections, Some(100));
    }

    #[test]
    fn test_config_with_resource_quota() {
        let toml_str = r#"
[resource_quota]
max_memory_bytes = 1073741824
max_cpu_percent = 80.0
max_open_files = 1024
max_connections = 100
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.resource_quota.max_memory_bytes, Some(1073741824));
        assert_eq!(config.resource_quota.max_cpu_percent, Some(80.0));
        assert_eq!(config.resource_quota.max_open_files, Some(1024));
        assert_eq!(config.resource_quota.max_connections, Some(100));
    }

    #[test]
    #[serial]
    fn test_config_from_env_resource_quota() {
        temp_env::with_var("SHIM_MAX_MEMORY_BYTES", Some("2048"), || {
            temp_env::with_var("SHIM_MAX_CPU_PERCENT", Some("50.5"), || {
                temp_env::with_var("SHIM_MAX_OPEN_FILES", Some("512"), || {
                    let config = Config::from_env();
                    assert_eq!(config.resource_quota.max_memory_bytes, Some(2048));
                    assert_eq!(config.resource_quota.max_cpu_percent, Some(50.5));
                    assert_eq!(config.resource_quota.max_open_files, Some(512));
                });
            });
        });
    }

    #[test]
    #[serial]
    fn test_config_from_env_invalid_resource_quota() {
        temp_env::with_var("SHIM_MAX_MEMORY_BYTES", Some("not_a_number"), || {
            let config = Config::from_env();
            assert!(config.resource_quota.max_memory_bytes.is_none());
        });
    }

    #[test]
    #[serial]
    fn test_config_merge_resource_quota() {
        temp_env::with_var("SHIM_MAX_MEMORY_BYTES", Some("4096"), || {
            let mut base = Config::default();
            let mut overlay = Config::default();
            overlay.resource_quota.max_memory_bytes = Some(4096);

            base.merge(overlay);
            assert_eq!(base.resource_quota.max_memory_bytes, Some(4096));
        });
    }

    #[test]
    #[serial]
    fn test_config_merge_without_env_override() {
        // Ensure no env var pollutes this test when running in parallel
        temp_env::with_var("SHIM_MAX_MEMORY_BYTES", None::<&str>, || {
            let mut base = Config::default();
            base.resource_quota.max_memory_bytes = Some(1024);
            let mut overlay = Config::default();
            overlay.resource_quota.max_memory_bytes = Some(2048);

            base.merge(overlay);
            assert_eq!(base.resource_quota.max_memory_bytes, Some(1024));
        });
    }

    // --- tenant tests ---

    #[test]
    fn test_tenants_default_empty() {
        let config = Config::default();
        assert!(config.tenants.is_empty());
    }

    #[test]
    fn test_tenant_config_serialization() {
        let tenant = TenantConfig {
            tenant_id: "t1".into(),
            max_memory_bytes: Some(1024),
            max_cpu_percent: Some(50.0),
            max_requests_per_sec: Some(100),
            allowed_features: vec!["feature-x".into()],
            quota_config: ResourceQuota::default(),
            reset_period_secs: 1,
        };
        let json = serde_json::to_string(&tenant).unwrap();
        assert!(json.contains("t1"));

        let deser: TenantConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.tenant_id, "t1");
        assert_eq!(deser.max_memory_bytes, Some(1024));
        assert_eq!(deser.max_cpu_percent, Some(50.0));
    }

    #[test]
    fn test_config_from_toml_with_tenants() {
        let toml_str = r#"
[[tenants]]
tenant_id = "tenant-a"
max_memory_bytes = 1073741824
max_cpu_percent = 80.0
max_requests_per_sec = 500
allowed_features = ["feature-x", "feature-y"]

[[tenants]]
tenant_id = "tenant-b"
max_memory_bytes = 2147483648
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tenants.len(), 2);
        assert_eq!(config.tenants[0].tenant_id, "tenant-a");
        assert_eq!(config.tenants[0].max_memory_bytes, Some(1073741824));
        assert_eq!(config.tenants[0].allowed_features.len(), 2);
        assert_eq!(config.tenants[1].tenant_id, "tenant-b");
    }

    #[test]
    #[serial]
    fn test_config_from_env_tenant() {
        temp_env::with_var("SHIM_TENANT_ID", Some("env-tenant"), || {
            temp_env::with_var("SHIM_TENANT_MAX_MEMORY", Some("4096"), || {
                temp_env::with_var("SHIM_TENANT_MAX_CPU", Some("75.5"), || {
                    let config = Config::from_env();
                    assert_eq!(config.tenants.len(), 1);
                    assert_eq!(config.tenants[0].tenant_id, "env-tenant");
                    assert_eq!(config.tenants[0].max_memory_bytes, Some(4096));
                    assert_eq!(config.tenants[0].max_cpu_percent, Some(75.5));
                });
            });
        });
    }

    #[test]
    #[serial]
    fn test_config_from_env_no_tenant() {
        temp_env::with_var("SHIM_TENANT_ID", None::<String>, || {
            let config = Config::from_env();
            assert!(config.tenants.is_empty());
        });
    }

    #[test]
    fn test_validate_duplicate_tenant_ids() {
        let mut config = Config::default();
        config.tenants = vec![
            TenantConfig {
                tenant_id: "dup".into(),
                max_memory_bytes: None,
                max_cpu_percent: None,
                max_requests_per_sec: None,
                allowed_features: vec![],
                quota_config: ResourceQuota::default(),
                reset_period_secs: 1,
            },
            TenantConfig {
                tenant_id: "dup".into(),
                max_memory_bytes: None,
                max_cpu_percent: None,
                max_requests_per_sec: None,
                allowed_features: vec![],
                quota_config: ResourceQuota::default(),
                reset_period_secs: 1,
            },
        ];
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field.contains("tenant_id")));
    }

    #[test]
    fn test_validate_empty_tenant_id() {
        let mut config = Config::default();
        config.tenants = vec![TenantConfig {
            tenant_id: "".into(),
            max_memory_bytes: None,
            max_cpu_percent: None,
            max_requests_per_sec: None,
            allowed_features: vec![],
            quota_config: ResourceQuota::default(),
            reset_period_secs: 1,
        }];
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field.contains("tenant_id")));
    }

    #[test]
    fn test_validate_tenant_cpu_out_of_range() {
        let mut config = Config::default();
        config.tenants = vec![TenantConfig {
            tenant_id: "t1".into(),
            max_memory_bytes: None,
            max_cpu_percent: Some(150.0),
            max_requests_per_sec: None,
            allowed_features: vec![],
            quota_config: ResourceQuota::default(),
            reset_period_secs: 1,
        }];
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field.contains("max_cpu_percent")));
    }

    #[test]
    #[serial]
    fn test_config_merge_tenant_env_override() {
        temp_env::with_var("SHIM_TENANT_ID", Some("env-tenant"), || {
            let mut base = Config::default();
            base.tenants.push(TenantConfig {
                tenant_id: "file-tenant".into(),
                max_memory_bytes: None,
                max_cpu_percent: None,
                max_requests_per_sec: None,
                allowed_features: vec![],
                quota_config: ResourceQuota::default(),
                reset_period_secs: 1,
            });

            let mut overlay = Config::default();
            overlay.tenants.push(TenantConfig {
                tenant_id: "env-tenant".into(),
                max_memory_bytes: Some(4096),
                max_cpu_percent: None,
                max_requests_per_sec: None,
                allowed_features: vec![],
                quota_config: ResourceQuota::default(),
                reset_period_secs: 1,
            });

            base.merge(overlay);
            // env-tenant should be present, file-tenant should be retained
            assert!(base.tenants.iter().any(|t| t.tenant_id == "env-tenant"));
            assert!(base.tenants.iter().any(|t| t.tenant_id == "file-tenant"));
        });
    }

    // --- version tests ---

    #[test]
    fn test_config_default_version() {
        let config = Config::default();
        assert_eq!(config.version.as_deref(), Some("1.0"));
    }

    #[test]
    fn test_config_valid_version() {
        let toml_str = r#"
version = "1.0"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.version.as_deref(), Some("1.0"));
        assert!(config.validate().is_empty());
    }

    #[test]
    fn test_config_unknown_version_warns() {
        let mut config = Config::default();
        config.version = Some("9.9".to_string());
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.field == "version"));
    }

    #[test]
    fn test_config_missing_version_defaults() {
        let toml_str = r#"
[health]
listen = "127.0.0.1:8080"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.version.as_deref(), Some("1.0"));
    }

    // --- property-based: TOML roundtrip ---

    #[test]
    fn test_config_toml_roundtrip_minimal() {
        let config = Config::default();
        let toml_str = toml::to_string(&config).unwrap();
        let deser: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.health.listen, deser.health.listen);
        assert_eq!(config.health.interval_secs, deser.health.interval_secs);
        assert_eq!(config.health.timeout_secs, deser.health.timeout_secs);
        assert_eq!(config.process.command, deser.process.command);
    }

    #[test]
    fn test_config_toml_roundtrip_with_vault() {
        let mut config = Config::default();
        config.vault = Some(VaultConfig {
            addr: "https://vault.example.com:8200".into(),
            role: "pki".into(),
            secret: "secret/data/db".into(),
            rotation_secs: 7200,
        });
        let toml_str = toml::to_string(&config).unwrap();
        let deser: Config = toml::from_str(&toml_str).unwrap();
        let vault = deser.vault.unwrap();
        assert_eq!(vault.addr, "https://vault.example.com:8200");
        assert_eq!(vault.role, "pki");
        assert_eq!(vault.rotation_secs, 7200);
    }

    #[test]
    fn test_config_toml_roundtrip_with_tenants() {
        let mut config = Config::default();
        config.tenants = vec![
            TenantConfig {
                tenant_id: "t1".into(),
                max_memory_bytes: Some(1024),
                max_cpu_percent: Some(75.0),
                max_requests_per_sec: Some(100),
                allowed_features: vec!["f1".into(), "f2".into()],
                quota_config: ResourceQuota::default(),
                reset_period_secs: 1,
            },
            TenantConfig {
                tenant_id: "t2".into(),
                max_memory_bytes: Some(2048),
                max_cpu_percent: None,
                max_requests_per_sec: None,
                allowed_features: vec![],
                quota_config: ResourceQuota::default(),
                reset_period_secs: 1,
            },
        ];
        let toml_str = toml::to_string(&config).unwrap();
        let deser: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deser.tenants.len(), 2);
        assert_eq!(deser.tenants[0].tenant_id, "t1");
        assert_eq!(deser.tenants[0].max_memory_bytes, Some(1024));
        assert_eq!(deser.tenants[0].allowed_features, vec!["f1", "f2"]);
        assert_eq!(deser.tenants[1].tenant_id, "t2");
    }

    #[test]
    fn test_config_toml_roundtrip_all_optional_sections() {
        let mut config = Config::default();
        config.vault = Some(VaultConfig {
            addr: "https://vault.local".into(),
            role: "r".into(),
            secret: "s".into(),
            rotation_secs: 3600,
        });
        config.backup = Some(BackupConfig {
            schedule: "0 2 * * *".into(),
            storage: "s3".into(),
            retention_days: 7,
            database: "mydb".into(),
            prefix: "backups/".into(),
        });
        config.tls = Some(TlsConfig {
            provider: "letsencrypt".into(),
            domain: "example.com".into(),
            email: "admin@example.com".into(),
            renew_before_secs: 86400,
        });
        config.failover = Some(FailoverConfig {
            primary: "10.0.0.1:5432".into(),
            replica: "10.0.0.2:5432".into(),
            check_interval_secs: 5,
            timeout_secs: 10,
            failure_threshold: 3,
            webhook: Some("https://hooks.example.com".into()),
            db_type: "postgres".into(),
        });
        config.replication = Some(ReplicationConfig {
            primary: "10.0.0.1:5432".into(),
            replicas: vec!["10.0.0.2:5432".into(), "10.0.0.3:5432".into()],
            mode: "synchronous".into(),
            check_interval_secs: 5,
            db_type: "postgres".into(),
            slot_name: Some("my_slot".into()),
        });
        config.audit = Some(AuditConfig {
            database: "mydb".into(),
            tables: vec!["users".into(), "orders".into()],
            format: "json".into(),
        });
        config.resource_quota = ResourceQuota {
            max_memory_bytes: Some(4096),
            max_cpu_percent: Some(80.0),
            max_open_files: Some(1024),
            max_connections: Some(100),
        };

        let toml_str = toml::to_string(&config).unwrap();
        let deser: Config = toml::from_str(&toml_str).unwrap();
        assert!(deser.vault.is_some());
        assert!(deser.backup.is_some());
        assert!(deser.tls.is_some());
        assert!(deser.failover.is_some());
        assert!(deser.replication.is_some());
        assert!(deser.audit.is_some());
        assert_eq!(deser.resource_quota.max_memory_bytes, Some(4096));
    }

    #[test]
    fn test_config_toml_roundtrip_preserves_none_optionals() {
        let config = Config::default();
        assert!(config.vault.is_none());
        assert!(config.backup.is_none());
        assert!(config.tls.is_none());
        let toml_str = toml::to_string(&config).unwrap();
        let deser: Config = toml::from_str(&toml_str).unwrap();
        assert!(deser.vault.is_none());
        assert!(deser.backup.is_none());
        assert!(deser.tls.is_none());
    }

    #[test]
    fn test_config_toml_roundtrip_custom_values() {
        let mut config = Config::default();
        config.health.listen = "192.168.1.100:9999".into();
        config.health.interval_secs = 30;
        config.health.timeout_secs = 15;
        config.health.liveness_cmd = "curl -f http://localhost/health".into();
        config.health.readiness_cmd = "pg_isready".into();
        config.process.command = "my-app".into();
        config.process.args = vec!["--verbose".into(), "--port=8080".into()];
        config.process.shutdown_timeout_secs = 60;

        let toml_str = toml::to_string(&config).unwrap();
        let deser: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deser.health.listen, "192.168.1.100:9999");
        assert_eq!(deser.health.interval_secs, 30);
        assert_eq!(deser.health.timeout_secs, 15);
        assert_eq!(deser.health.liveness_cmd, "curl -f http://localhost/health");
        assert_eq!(deser.process.command, "my-app");
        assert_eq!(deser.process.args, vec!["--verbose", "--port=8080"]);
        assert_eq!(deser.process.shutdown_timeout_secs, 60);
    }

    // --- validate_schema tests ---

    #[test]
    fn test_validate_schema_default_config_passes() {
        let config = Config::default();
        assert!(config.validate_schema().is_empty());
    }

    #[test]
    fn test_validate_schema_port_out_of_range() {
        let mut config = Config::default();
        config.health.listen = "0.0.0.0:70000".into();
        let errors = config.validate_schema();
        assert!(errors.iter().any(|e| e.field == "health.listen"));
    }

    #[test]
    fn test_validate_schema_migration_port_zero() {
        let mut config = Config::default();
        config.migration = Some(MigrationConfig {
            dir: std::path::PathBuf::from("/tmp"),
            database: "mydb".into(),
            auto_migrate: false,
            db_host: "127.0.0.1".into(),
            db_port: 0,
            db_user: "postgres".into(),
            db_password: "".into(),
            db_type: "postgres".into(),
        });
        let errors = config.validate_schema();
        assert!(errors.iter().any(|e| e.field == "migration.db_port"));
    }

    #[test]
    fn test_validate_schema_invalid_cron_expression() {
        let mut config = Config::default();
        config.backup = Some(BackupConfig {
            schedule: "not-a-valid-cron".into(),
            storage: "s3".into(),
            retention_days: 7,
            database: "mydb".into(),
            prefix: "".into(),
        });
        let errors = config.validate_schema();
        assert!(errors.iter().any(|e| e.field == "backup.schedule"));
    }

    #[test]
    fn test_validate_schema_valid_cron_expression() {
        let mut config = Config::default();
        config.backup = Some(BackupConfig {
            // `cron::Schedule` uses 6/7-field (seconds-precision) expressions —
            // same format as backup-shim's default BACKUP_SCHEDULE.
            schedule: "0 0 2 * * *".into(),
            storage: "s3".into(),
            retention_days: 7,
            database: "mydb".into(),
            prefix: "".into(),
        });
        let errors = config.validate_schema();
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_schema_empty_backup_storage() {
        let mut config = Config::default();
        config.backup = Some(BackupConfig {
            schedule: "0 2 * * *".into(),
            storage: "".into(),
            retention_days: 7,
            database: "mydb".into(),
            prefix: "".into(),
        });
        let errors = config.validate_schema();
        assert!(errors.iter().any(|e| e.field == "backup.storage"));
    }

    #[test]
    fn test_validate_schema_vault_invalid_url_scheme() {
        let mut config = Config::default();
        config.vault = Some(VaultConfig {
            addr: "ftp://vault.local".into(),
            role: "test".into(),
            secret: "secret/data".into(),
            rotation_secs: 3600,
        });
        let errors = config.validate_schema();
        assert!(errors.iter().any(|e| e.field == "vault.addr"));
    }

    #[test]
    fn test_validate_schema_failover_webhook_invalid_scheme() {
        let mut config = Config::default();
        config.failover = Some(FailoverConfig {
            primary: "10.0.0.1:5432".into(),
            replica: "10.0.0.2:5432".into(),
            check_interval_secs: 5,
            timeout_secs: 10,
            failure_threshold: 3,
            webhook: Some("ftp://hooks.example.com".into()),
            db_type: "postgres".into(),
        });
        let errors = config.validate_schema();
        assert!(errors.iter().any(|e| e.field == "failover.webhook"));
    }

    #[test]
    fn test_validate_schema_failover_webhook_valid() {
        let mut config = Config::default();
        config.failover = Some(FailoverConfig {
            primary: "10.0.0.1:5432".into(),
            replica: "10.0.0.2:5432".into(),
            check_interval_secs: 5,
            timeout_secs: 10,
            failure_threshold: 3,
            webhook: Some("https://hooks.example.com".into()),
            db_type: "postgres".into(),
        });
        let errors = config.validate_schema();
        assert!(!errors.iter().any(|e| e.field == "failover.webhook"));
    }

    #[test]
    fn test_validate_schema_empty_migration_dir() {
        let mut config = Config::default();
        config.migration = Some(MigrationConfig {
            dir: std::path::PathBuf::new(),
            database: "mydb".into(),
            auto_migrate: false,
            db_host: "127.0.0.1".into(),
            db_port: 5432,
            db_user: "postgres".into(),
            db_password: "".into(),
            db_type: "postgres".into(),
        });
        let errors = config.validate_schema();
        assert!(errors.iter().any(|e| e.field == "migration.dir"));
    }

    #[test]
    fn test_validate_schema_multiple_errors_collected() {
        let mut config = Config::default();
        config.health.listen = "0.0.0.0:70000".into();
        config.backup = Some(BackupConfig {
            schedule: "bad".into(),
            storage: "".into(),
            retention_days: 0,
            database: "mydb".into(),
            prefix: "".into(),
        });
        let errors = config.validate_schema();
        assert!(errors.len() >= 3);
    }

    #[test]
    fn test_validate_schema_health_valid_custom_port() {
        let mut config = Config::default();
        config.health.listen = "127.0.0.1:8080".into();
        let errors = config.validate_schema();
        assert!(!errors.iter().any(|e| e.field == "health.listen"));
    }

    #[test]
    fn test_validate_schema_health_port_zero() {
        let mut config = Config::default();
        config.health.listen = "0.0.0.0:0".into();
        let errors = config.validate_schema();
        assert!(errors.iter().any(|e| e.field == "health.listen"));
    }
}
