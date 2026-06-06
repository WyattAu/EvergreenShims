# replication-shim

Database replication management for PostgreSQL and MySQL. Spawns a health-check loop that monitors primary connectivity, replica lag, and overall replication state.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `REPLICATION_PRIMARY` | Primary database address | — |
| `REPLICATION_REPLICAS` | Comma-separated replica addresses | — |
| `REPLICATION_MODE` | Mode: `synchronous`, `asynchronous` | `asynchronous` |
| `REPLICATION_SLOT` | Replication slot name (PostgreSQL) | — |
| `REPLICATION_CHECK_SECS` | Health check interval (seconds) | `10` |
| `REPLICATION_DB_TYPE` | Database type: `postgres`, `mysql` | — |
| `REPLICATION_DB_HOST` | Primary DB host | `127.0.0.1` |
| `REPLICATION_DB_PORT` | Primary DB port | `5432` |
| `REPLICATION_DB_USER` | Primary DB user | `postgres` |
| `REPLICATION_DB_PASSWORD` | Primary DB password | — |
| `REPLICATION_DB_NAME` | Primary DB name | `postgres` |
| `REPLICATION_LAG_THRESHOLD_BYTES` | Lag threshold in bytes | `1048576` (1MB) |

## Usage

```rust
use replication_shim::ReplicationShim;
use shim_core::Capability;

let mut shim = ReplicationShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Supports synchronous and asynchronous replication modes.
- Publishes `ReplicationState` events to the ShimBus.
- Configurable lag threshold in bytes.

## Metrics Exposed

- `replication_state` – Current state (0=healthy, 1=degraded, 2=broken).
- `replication_lag_bytes` – Current replication lag in bytes.
- `replication_checks_total` – Total health checks performed.

## Testing

```bash
cargo test -p replication-shim
```
