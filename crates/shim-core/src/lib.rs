//! Core types and traits for EvergreenShims.
//!
//! This crate provides the foundational abstractions that all shims implement.
//! Includes the `ShimBus` event system for cross-shim communication.

pub mod bus;
pub mod config;
pub mod error;
pub mod event;
pub mod health;
pub mod hotreload;
pub mod metrics;
pub mod process;
pub mod shutdown;
pub mod signal;
pub mod wiring;

#[cfg(feature = "redis-bus")]
pub mod redis_bridge;

pub use bus::ShimBus;
pub use config::{
    AuditConfig, BackupConfig, Config, FailoverConfig, HealthConfig, MigrationConfig,
    ProcessConfig, ReplicationConfig, TlsConfig, VaultConfig,
};
pub use error::{Error, Result};
pub use event::{EventHandler, EventType, Severity, ShimEvent};
pub use health::{CommandHealthCheck, HealthCheck, HealthStatus, StartupProbe};
pub use metrics::Metric;
pub use process::ChildProcess;
pub use shutdown::{
    graceful_shutdown, DatabaseType, GracefulShutdown, ShutdownManager, ShutdownResult,
    ShutdownStrategy,
};
pub use signal::{Signal, SignalHandler};

/// A shim capability that can be enabled/disabled.
#[async_trait::async_trait]
pub trait Capability: Send + Sync {
    /// Name of the capability (e.g., "health", "vault").
    fn name(&self) -> &str;

    /// Initialize the capability with config.
    async fn init(&mut self, config: &Config) -> Result<()>;

    /// Attach the ShimBus for cross-shim event communication.
    /// Called after `init`, before `start`.
    fn set_bus(&mut self, _bus: ShimBus) {}

    /// Start background tasks (if any).
    async fn start(&mut self) -> Result<()>;

    /// Stop gracefully.
    async fn stop(&mut self) -> Result<()>;

    /// Collect metrics.
    fn metrics(&self) -> Vec<Metric>;
}
