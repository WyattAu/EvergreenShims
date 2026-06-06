# sharding-shim

Automatic sharding for distributed databases. Routes queries to the correct shard based on a shard key.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `SHARDING_STRATEGY` | Strategy: `hash`, `range`, `directory` | `hash` |
| `SHARDING_KEY` | Shard key column | — (required) |
| `SHARDING_COUNT` | Number of shards | `4` |
| `SHARDING_ADDRESSES` | Comma-separated shard addresses | — |
| `SHARDING_VNODES` | Virtual nodes per shard for hash ring | `150` |

## Usage

```rust
use sharding_shim::ShardingShim;
use shim_core::Capability;

let mut shim = ShardingShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Hash, range, and directory-based sharding strategies.
- Consistent hashing with configurable virtual nodes.
- Supports heterogeneous shard addresses.

## Metrics Exposed

- `sharding_queries_routed_total` – Total queries routed.
- `sharding_queries_per_shard` – Queries routed per shard.
- `sharding_rebalance_total` – Total rebalance events.

## Testing

```bash
cargo test -p sharding-shim
```
