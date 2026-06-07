//! Error types for shims.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Backup error with structured details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub struct BackupError {
    /// The backup command that was executed.
    pub command: String,
    /// Error message from the backup operation.
    pub message: String,
    /// Process exit code, if available.
    pub exit_code: Option<i32>,
}

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.exit_code {
            Some(code) => write!(
                f,
                "backup error: command `{}` failed (exit code {}): {}",
                self.command, code, self.message
            ),
            None => write!(
                f,
                "backup error: command `{}` failed: {}",
                self.command, self.message
            ),
        }
    }
}

impl BackupError {
    /// Creates a new `BackupError`.
    pub fn new(
        command: impl Into<String>,
        message: impl Into<String>,
        exit_code: Option<i32>,
    ) -> Self {
        Self {
            command: command.into(),
            message: message.into(),
            exit_code,
        }
    }
}

/// TLS error with structured details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("tls error: {provider}: {message}")]
pub struct TlsError {
    /// The TLS provider that encountered the error.
    pub provider: String,
    /// Error message from the TLS operation.
    pub message: String,
}

impl TlsError {
    /// Creates a new `TlsError`.
    pub fn new(provider: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            message: message.into(),
        }
    }
}

/// Failover error with structured details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("failover error: connector `{connector}` failed: {message}")]
pub struct FailoverError {
    /// The connector that failed.
    pub connector: String,
    /// Error message from the failover operation.
    pub message: String,
}

impl FailoverError {
    /// Creates a new `FailoverError`.
    pub fn new(connector: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            connector: connector.into(),
            message: message.into(),
        }
    }
}

/// Migration error with structured details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub struct MigrationError {
    /// The migration version that failed, if known.
    pub version: Option<u32>,
    /// Error message from the migration operation.
    pub message: String,
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.version {
            Some(v) => write!(f, "migration error at version {v}: {}", self.message),
            None => write!(f, "migration error: {}", self.message),
        }
    }
}

impl MigrationError {
    /// Creates a new `MigrationError`.
    pub fn new(version: Option<u32>, message: impl Into<String>) -> Self {
        Self {
            version,
            message: message.into(),
        }
    }
}

/// Error type for shim operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Configuration error.
    #[error("configuration error: {0}")]
    Config(String),

    /// Database connection error.
    #[error("database error: {0}")]
    Database(String),

    /// Network error.
    #[error("network error: {0}")]
    Network(String),

    /// Process error.
    #[error("process error: {0}")]
    Process(String),

    /// Vault error.
    #[error("vault error: {0}")]
    Vault(String),

    /// Backup error.
    #[error("backup error: {0}")]
    Backup(String),

    /// Structured backup error with command, message, and exit code.
    #[error(transparent)]
    BackupDetail(#[from] BackupError),

    /// Migration error.
    #[error("migration error: {0}")]
    Migration(String),

    /// Structured migration error with version and message.
    #[error(transparent)]
    MigrationDetail(#[from] MigrationError),

    /// TLS error.
    #[error("tls error: {0}")]
    Tls(String),

    /// Structured TLS error with provider and message.
    #[error(transparent)]
    TlsDetail(#[from] TlsError),

    /// Failover error.
    #[error("failover error: {0}")]
    Failover(String),

    /// Structured failover error with connector and message.
    #[error(transparent)]
    FailoverDetail(#[from] FailoverError),

    /// Timeout error.
    #[error("timeout: {0}")]
    Timeout(String),

    /// I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Connection error (Redis, database, etc.).
    #[error("connection error: {0}")]
    Connection(String),

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// TOML error.
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

    /// Plugin error.
    #[error("plugin error: {0}")]
    Plugin(String),

    /// Any other error.
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

/// Result type for shim operations.
pub type Result<T> = std::result::Result<T, Error>;

impl From<nix::errno::Errno> for Error {
    fn from(err: nix::errno::Errno) -> Self {
        Error::Process(err.to_string())
    }
}
