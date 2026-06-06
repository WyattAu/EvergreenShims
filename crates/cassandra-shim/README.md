# cassandra-shim

Health checks and cluster monitoring for Cassandra. Uses nodetool for health checks and cluster topology queries.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `CASSANDRA_HOST` | Cassandra host | `localhost` |
| `CASSANDRA_PORT` | CQL port | `9042` |
| `CASSANDRA_JMX_PORT` | JMX port for nodetool | `7199` |
| `CASSANDRA_CLUSTER` | Cluster name | `local` |

## Usage

```rust
use cassandra_shim::CassandraShim;
use shim_core::Capability;

let mut shim = CassandraShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Health checks via JMX nodetool.
- Cluster topology with datacenter and rack info.
- Node status and load monitoring.

## Metrics Exposed

- `cassandra_health_checks_total` – Total health checks.
- `cassandra_node_count` – Number of nodes in cluster.
- `cassandra_live_nodes` – Number of live nodes.
- `cassandra_datacenter_count` – Number of datacenters.

## Testing

```bash
cargo test -p cassandra-shim
```
