//! Error types for shims.

/// Error type for shim operations.
#[derive(Debug, thiserror::Error)]
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

    /// Migration error.
    #[error("migration error: {0}")]
    Migration(String),

    /// TLS error.
    #[error("tls error: {0}")]
    Tls(String),

    /// Failover error.
    #[error("failover error: {0}")]
    Failover(String),

    /// Timeout error.
    #[error("timeout: {0}")]
    Timeout(String),

    /// I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// TOML error.
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

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
