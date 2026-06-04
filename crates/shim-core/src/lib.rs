//! Core types and traits for EvergreenShims.
//!
//! This crate provides the foundational abstractions that all shims implement.

pub mod config;
pub mod error;
pub mod health;
pub mod metrics;
pub mod process;
pub mod signal;

pub use config::{
    AuditConfig, BackupConfig, Config, FailoverConfig, HealthConfig, MigrationConfig,
    ProcessConfig, TlsConfig, VaultConfig,
};
pub use error::{Error, Result};
pub use health::{CommandHealthCheck, HealthCheck, HealthStatus};
pub use metrics::Metric;
pub use process::ChildProcess;
pub use signal::{Signal, SignalHandler};

/// A shim capability that can be enabled/disabled.
#[async_trait::async_trait]
pub trait Capability: Send + Sync {
    /// Name of the capability (e.g., "health", "vault").
    fn name(&self) -> &str;

    /// Initialize the capability with config.
    async fn init(&mut self, config: &Config) -> Result<()>;

    /// Start background tasks (if any).
    async fn start(&mut self) -> Result<()>;

    /// Stop gracefully.
    async fn stop(&mut self) -> Result<()>;

    /// Collect metrics.
    fn metrics(&self) -> Vec<Metric>;
}
