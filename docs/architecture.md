# Architecture

## System Overview

EvergreenShims implements a PID 1 shim pattern: a single Rust binary wraps a child process (database, proxy, etc.) while providing operational capabilities. The binary is statically linked via musl, runs in `scratch` containers, and has zero runtime dependencies.

## Workspace Structure

```
evergreen-shims/
  Cargo.toml                    Workspace root (25 members)
  crates/
    shim-core/                  Shared types, Capability trait, bus, events, metrics
    health-shim/                Health probes, Prometheus metrics, process mgmt
    vault-shim/                 Vault/KMS secrets rotation
    backup-shim/                Automated backups, WAL archiving
    migration-shim/             SQL file-based migrations
    audit-shim/                 Query logging, SIEM export
    proxy-shim/                 Connection pooling, circuit breaker, retries
    tls-shim/                   Auto-TLS (Let's Encrypt / internal CA)
    config-shim/                Hot-reload configuration
    failover-shim/              Automatic failover for HA databases
    replication-shim/           WAL tracking, lag monitoring
    cache-shim/                 Query result caching (LRU/FIFO)
    cdc-shim/                   Change Data Capture
    sharding-shim/              Hash/range shard routing
    archival-shim/              Lifecycle tiers, compression
    auth-shim/                  Token auth, API keys, RBAC
    encryption-shim/            AES-GCM, ChaCha20, key rotation
    compliance-shim/            CIS/STIG scoring
    scheduler-shim/             Cron task scheduling
    queue-shim/                 Job processing, DLQ
    alerting-shim/              Alert routing and dedup
    chaos-shim/                 Fault injection
    cost-shim/                  Resource tracking per tenant
    evergreen-shim/             Unified binary (feature-gated)
    integration-tests/          Cross-shim integration tests
  tests/                        Docker Compose infrastructure
  docs/                         Documentation
  .github/workflows/            CI/CD pipelines
```

## Design Principles

### Single Binary, Multiple Capabilities

Each shim is a separate crate for development isolation, compiled into a single binary via Cargo feature flags:

```rust
#[cfg(feature = "health")]
capabilities.push(Box::new(HealthShim::new()));
#[cfg(feature = "vault")]
capabilities.push(Box::new(VaultShim::new()));
```

### Layered Architecture

```
+--------------------------------------------------+
|  shim-core                                        |
|  (Capability trait, Config, ShimBus, Events,      |
|   Metrics, Shutdown, Process, HotReload, Signals) |
+--------------------------------------------------+
|  health | vault | backup | migration | ... (22)   |
+--------------------------------------------------+
|  evergreen-shim (unified binary, feature-gated)   |
+--------------------------------------------------+
```

### PID 1 Execution Model

```
PID 1: /app/shim
  +-- PID N: /app/postgres (child process)
```

The shim:

1. Spawns the child process on startup
2. Forwards signals (SIGTERM, SIGINT, SIGHUP) to the child
3. Monitors child health via configured probes
4. Executes background tasks (backups, rotations, migrations)
5. Exits with the child's exit code

### Capability Trait

All shims implement `Capability`:

```rust
pub trait Capability: Send + Sync {
    fn name(&self) -> &str;
    fn init(&mut self, config: &Config) -> Result<()>;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn metrics(&self) -> Vec<Metric>;
    fn set_bus(&mut self, bus: ShimBus);
}
```

### ShimBus Event System

Cross-shim communication via in-process broadcast channel (`tokio::sync::broadcast`):

```
health-shim --[Unhealthy]--> ShimBus --[FailoverTriggered]--> failover-shim
scheduler-shim --[SchedulerTaskFired]--> ShimBus --[BackupStarted]--> backup-shim
backup-shim --[BackupCompleted]--> ShimBus --[EncryptionKeyRotate]--> encryption-shim
```

Pre-wired handlers:

- `HealthFailoverHandler`: Triggers failover after N consecutive unhealthy events
- `SchedulerBackupHandler`: Triggers backup on scheduler task fire
- `BackupEncryptionHandler`: Triggers key rotation after backup completion
- `AlertFanInHandler`: Routes alertable events to alerting-shim

## Signal Handling

```
Kubernetes --SIGTERM--> Shim (PID 1) --SIGTERM--> Child Process
                             |
                             +-- timeout (configurable)
                             |
                             +-- SIGKILL if not exited
```

Per-database shutdown sequences:

| DB Type | Sequence |
|---------|----------|
| PostgreSQL | SIGTERM (smart shutdown) -> wait for queries -> checkpoint -> exit |
| Redis | SIGTERM -> RDB save -> wait for fork -> exit |
| Generic | SIGTERM -> timeout -> SIGKILL |

## Metrics

Prometheus exposition format on a single HTTP endpoint:

```
GET /metrics

health_liveness{service="postgres"} 1
health_readiness{service="postgres"} 1
vault_rotation_success_total{secret="postgres"} 42
backup_success_total{storage="s3"} 30
backup_duration_seconds{storage="s3"} 12.5
migration_current_version{service="postgres"} 10
failover_events_total 1
```

## Configuration Hierarchy

Priority (highest to lowest):

1. Environment variables
2. TOML config file (`/etc/shim/config.toml`)
3. Compiled defaults

## Error Handling

`anyhow::Result` for error propagation with context. `thiserror` for library error types:

```rust
fn backup_database() -> Result<()> {
    let conn = connect().context("database connection failed")?;
    let backup = dump(&conn).context("dump failed")?;
    upload(&backup).context("upload failed")?;
    Ok(())
}
```

## Static Linking

All builds target `x86_64-unknown-linux-musl` or `aarch64-unknown-linux-musl` for static linking. No runtime dependencies. Runs in `scratch` Docker images.

`reqwest` uses `aws-lc-rs` (ring-free) for musl compatibility. TLS via `rustls`.

## Thread Safety

- `ShimBus`: `tokio::sync::broadcast` channel, clone-safe
- Config: `parking_lot::RwLock` for hot-reload
- Metrics: `parking_lot::Mutex` on `HashMap`
- Process: `tokio::process::Child` with async wait
- Shutdown: `tokio::sync::watch` channel for coordinated shutdown
