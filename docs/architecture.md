# Architecture

## Overview

EvergreenShims is a Rust workspace containing individual shim crates and a unified binary that combines them via Cargo features.

```
evergreen-shims/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── shim-core/          # Shared types, traits, config
│   ├── health-shim/        # Health probes, metrics, process mgmt
│   ├── vault-shim/         # Secrets rotation
│   ├── backup-shim/        # Automated backups
│   ├── migration-shim/     # Schema migrations
│   ├── audit-shim/         # Query logging, SIEM
│   ├── proxy-shim/         # Connection pooling, retries
│   ├── tls-shim/           # Auto-TLS
│   ├── config-shim/        # Hot-reload config
│   ├── failover-shim/      # Automatic failover
│   ├── replication-shim/   # Database replication
│   ├── cache-shim/         # Query result caching
│   ├── cdc-shim/           # Change Data Capture
│   ├── sharding-shim/      # Automatic sharding
│   ├── archival-shim/      # Data archival
│   ├── auth-shim/          # Authentication layer
│   ├── encryption-shim/    # Transparent encryption
│   ├── compliance-shim/    # CIS/STIG compliance
│   ├── scheduler-shim/     # Cron-like scheduling
│   ├── queue-shim/         # Background jobs
│   ├── alerting-shim/      # Alert routing
│   ├── chaos-shim/         # Fault injection
│   ├── cost-shim/          # Resource tracking
│   └── evergreen-shim/     # Unified binary (feature-gated)
├── tests/                  # Integration tests
├── docs/                   # Documentation
└── .github/workflows/      # CI/CD
```

## Design Principles

### 1. Single Binary, Multiple Capabilities

Each shim is a separate crate for development, but the final binary is a single executable with all capabilities enabled via Cargo features.

```rust
// crates/evergreen-shim/src/main.rs
#[cfg(feature = "health")]
use health_shim::HealthShim;

#[cfg(feature = "vault")]
use vault_shim::VaultShim;

// ...
```

### 2. Layered Architecture

```
┌─────────────────────────────────────────────────┐
│  shim-core (shared types, traits, config)       │
├─────────────────────────────────────────────────┤
│  health-shim │ vault-shim │ backup-shim │ ...   │
├─────────────────────────────────────────────────┤
│  evergreen-shim (unified binary)                │
└─────────────────────────────────────────────────┘
```

### 3. PID 1 as Entry Point

The shim runs as PID 1 and manages the application as a child process:

```
PID 1: /app/shim
  └─ PID N: /app/postgres (child process)
```

The shim:
- Forwards signals (SIGTERM, SIGINT, SIGHUP) to the child
- Monitors child health
- Performs background tasks (backups, rotations)
- Exits with child's exit code

### 4. No Runtime Dependencies

The shim is a static binary with no runtime dependencies (musl-linked). It can run in `scratch` images.

## Core Traits

```rust
// crates/shim-core/src/lib.rs

/// A shim capability that can be enabled/disabled.
pub trait Capability: Send + Sync {
    /// Name of the capability (e.g., "health", "vault").
    fn name(&self) -> &str;
    
    /// Initialize the capability with config.
    fn init(&mut self, config: &Config) -> Result<()>;
    
    /// Start background tasks (if any).
    fn start(&mut self) -> Result<()>;
    
    /// Stop gracefully.
    fn stop(&mut self) -> Result<()>;
    
    /// Collect metrics.
    fn metrics(&self) -> Vec<Metric>;
}

/// Health check for the managed application.
pub trait HealthChecker: Send + Sync {
    /// Check if the application is alive.
    fn liveness(&self) -> HealthStatus;
    
    /// Check if the application is ready to serve.
    fn readiness(&self) -> HealthStatus;
}
```

## Signal Handling

```
┌─────────────────────────────────────────────────┐
│  Signal Flow                                     │
├─────────────────────────────────────────────────┤
│                                                  │
│  Kubernetes ──SIGTERM──→ Shim ──SIGTERM──→ App   │
│                                                  │
│  Shim waits for app to exit (timeout: 30s)       │
│  If app doesn't exit, SIGKILL after timeout      │
│                                                  │
└─────────────────────────────────────────────────┘
```

## Metrics Exposure

All shims expose metrics on a single HTTP endpoint:

```
GET /metrics

# Health metrics
health_liveness{service="postgres"} 1
health_readiness{service="postgres"} 1
health_check_duration_seconds{service="postgres"} 0.001

# Vault metrics
vault_rotation_success_total{secret="postgres"} 42
vault_rotation_failure_total{secret="postgres"} 0
vault_rotation_last_success_timestamp{secret="postgres"} 1717500000

# Backup metrics
backup_success_total{storage="s3"} 30
backup_failure_total{storage="s3"} 0
backup_duration_seconds{storage="s3"} 12.5
backup_size_bytes{storage="s3"} 1073741824

# Migration metrics
migration_current_version{service="postgres"} 10
migration_last_success_timestamp{service="postgres"} 1717500000
```

## Configuration Hierarchy

1. **Environment variables** (highest priority)
2. **TOML config file** (`/etc/shim/config.toml`)
3. **Default values** (lowest priority)

```rust
pub fn load_config() -> Config {
    let mut config = Config::default();
    
    // Load from file
    if let Ok(file_config) = Config::from_file("/etc/shim/config.toml") {
        config.merge(file_config);
    }
    
    // Override with env vars
    config.merge(Config::from_env());
    
    config
}
```

## Error Handling

All shims use `anyhow::Result` for error propagation:

```rust
use anyhow::{Context, Result};

fn backup_database() -> Result<()> {
    let conn = connect().context("Failed to connect to database")?;
    let backup = dump(&conn).context("Failed to dump database")?;
    upload(&backup).context("Failed to upload backup")?;
    Ok(())
}
```

## Testing Strategy

See [testing.md](testing.md) for the full testing strategy.
