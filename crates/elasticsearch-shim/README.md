# elasticsearch-shim

Health checks and snapshot management for Elasticsearch.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `ES_URL` | Elasticsearch URL | `http://localhost:9200` |
| `ES_INDEX` | Index to monitor | — |
| `ES_SNAPSHOT_REPO` | Snapshot repository name | — |
| `ES_SNAPSHOT_REPO_URL` | Repository location URL | — |

## Usage

```rust
use elasticsearch_shim::ElasticsearchShim;
use shim_core::Capability;

let mut shim = ElasticsearchShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Cluster health checks with shard status.
- Snapshot repository management.
- Index-level monitoring.

## Metrics Exposed

- `es_health_checks_total` – Total health checks.
- `es_cluster_status` – Cluster status (0=green, 1=yellow, 2=red).
- `es_active_shards` – Number of active shards.
- `es_unassigned_shards` – Number of unassigned shards.

## Testing

```bash
cargo test -p elasticsearch-shim
```
