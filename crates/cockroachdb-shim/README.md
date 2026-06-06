# cockroachdb-shim

Health checks, topology awareness, and CDC for CockroachDB. Uses the PostgreSQL wire protocol for health checks and SQL for topology queries.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `CRDB_HOST` | CockroachDB host | `localhost` |
| `CRDB_PORT` | CockroachDB port | `26257` |
| `CRDB_USER` | Database user | `root` |
| `CRDB_PASSWORD` | Database password | — |
| `CRDB_DATABASE` | Database name | `defaultdb` |
| `CRDB_URL` | Full connection URL (overrides host/port/user/password) | — |

## Usage

```rust
use cockroachdb_shim::CrdbShim;
use shim_core::Capability;

let mut shim = CrdbShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Cluster topology awareness with node info.
- Health checks using `pg_isready`.
- Supports connection via URL or individual parameters.

## Metrics Exposed

- `crdb_health_checks_total` – Total health checks.
- `crdb_node_count` – Number of nodes in cluster.
- `crdb_live_nodes` – Number of live nodes.

## Testing

```bash
cargo test -p cockroachdb-shim
```
