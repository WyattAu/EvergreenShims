# failover-shim

Automatic failover for HA databases. Monitors a primary database, detects failure, promotes a replica, and sends notifications.

## Connectors

- **Generic TCP**: Basic TCP connectivity checks (default).
- **Patroni Failover**: PostgreSQL Patroni cluster monitoring via `psql`.
- **Redis Sentinel Failover**: Redis Sentinel master tracking.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `FAILOVER_PRIMARY` | Primary database address (`host:port`) | — |
| `FAILOVER_REPLICA` | Replica database address (`host:port`) | — |
| `FAILOVER_CHECK_INTERVAL` | Health check interval (seconds) | `5` |
| `FAILOVER_TIMEOUT` | Health check timeout (seconds) | `3` |
| `FAILOVER_FAILURE_THRESHOLD` | Consecutive failures before failover | `3` |
| `FAILOVER_WEBHOOK` | Webhook URL for notifications | — |
| `FAILOVER_DB_TYPE` | Database type: `postgres`, `mariadb`, `mysql` | — |
| `FAILOVER_CONNECTOR` | Connector type: `tcp`, `patroni`, `redis-sentinel` | `tcp` |
| `FAILOVER_DB_HOST` | Database host for psql | `127.0.0.1` |
| `FAILOVER_DB_PORT` | Database port for psql | `5432` |
| `FAILOVER_DB_USER` | Database user for psql | `postgres` |
| `FAILOVER_DB_PASSWORD` | Database password for psql | — |
| `FAILOVER_DB_NAME` | Database name for psql | `postgres` |
| `FAILOVER_LAG_THRESHOLD_SECS` | Replication lag threshold (seconds) | `30` |
| `REDIS_SENTINEL_URL` | Sentinel URL | `redis://localhost:26379` |
| `REDIS_SENTINEL_MASTER` | Master name | `mymaster` |

## Usage

```rust
use failover_shim::FailoverShim;
use shim_core::Capability;

let mut shim = FailoverShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Supports automatic failback when the original primary recovers.
- Configurable failure threshold and health check intervals.
- Webhook notifications for failover events (Slack, PagerDuty).

## Metrics Exposed

- `failover_checks_total` – Total health checks performed.
- `failover_state` – Current state (0=healthy, 1=degraded, 2=failed).
- `failover_events_total` – Total failover events triggered.

## Testing

```bash
cargo test -p failover-shim
```
