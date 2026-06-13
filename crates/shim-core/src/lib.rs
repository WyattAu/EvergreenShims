//! Core types and traits for EvergreenShims.
//!
//! This crate provides the foundational abstractions that all shims implement.
//! Includes the `ShimBus` event system for cross-shim communication.

/// In-process event broadcast bus for cross-shim communication.
pub mod bus;
/// Configuration types and validation for all shims.
pub mod config;
/// Error types and the [`Result`] alias for shim operations.
pub mod error;
/// Typed event system: [`ShimEvent`], [`EventType`], and [`EventHandler`] trait.
pub mod event;
/// Health check traits, status types, and startup probes.
pub mod health;
/// Config hot-reload via filesystem watching.
pub mod hotreload;
/// Prometheus metrics collector and HTTP server for shim observability.
pub mod metrics;
/// Prometheus metrics export server with per-shim metrics (port 9101).
pub mod metrics_export;
/// AlertManager webhook integration for ShimBus events.
pub mod alerting;
/// Child process lifecycle management.
pub mod process;
/// Resource quota monitoring and enforcement.
pub mod resource;
/// Graceful shutdown sequences for different database types.
pub mod shutdown;
/// Signal handling for SIGTERM, SIGINT, and SIGHUP.
pub mod signal;
/// Structured logging initialization (JSON or human-readable).
pub mod structured_logging;
/// Multi-tenancy isolation, quota enforcement, and per-tenant metrics.
pub mod tenant;
/// Pre-built cross-shim event wiring handlers.
pub mod wiring;

#[cfg(feature = "redis-bus")]
pub mod redis_bridge;

#[cfg(feature = "otel")]
pub mod otel;

#[cfg(feature = "wasm")]
pub mod wasm;

pub use bus::ShimBus;
pub use config::{
    AuditConfig, BackupConfig, Config, ConfigValidationError, FailoverConfig, HealthConfig,
    MigrationConfig, ProcessConfig, ReplicationConfig, ResourceQuota, TenantConfig, TlsConfig,
    VaultConfig,
};
pub use error::{Error, Result};
pub use event::{EventHandler, EventType, Severity, ShimEvent};
pub use health::{CommandHealthCheck, HealthCheck, HealthStatus, StartupProbe};
pub use metrics::Metric;
pub use process::ChildProcess;
pub use resource::{ResourceMonitor, ResourceUsage};
pub use shutdown::{
    graceful_shutdown, DatabaseType, GracefulShutdown, ShutdownManager, ShutdownResult,
    ShutdownStrategy,
};
pub use signal::{Signal, SignalHandler};
pub use tenant::{
    AuditEntry, CpuTimeTracker, InvalidTenantId, TenantIsolator, TenantMetrics, TenantQuotaResult,
    TenantUsage, TokenBucket,
};

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
