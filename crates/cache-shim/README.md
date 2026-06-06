# cache-shim

Query result caching with in-process LRU/LFU/FIFO eviction. Intercepts database queries and caches results for faster repeated access. Runs in-process (single-node deployments).

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `CACHE_TTL` | Time-to-live in seconds | `300` |
| `CACHE_MAX_ENTRIES` | Max cache entries | `10000` |
| `CACHE_MAX_SIZE` | Max cache size in bytes | `1073741824` (1GB) |
| `CACHE_STRATEGY` | Eviction strategy: `lru`, `lfu`, `fifo` | `lru` |
| `CACHE_PREFIX` | Key prefix | `shim:` |
| `CACHE_SWEEP_INTERVAL` | Background sweep interval (seconds) | `60` |

## Usage

```rust
use cache_shim::CacheShim;
use shim_core::Capability;

let mut shim = CacheShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Three eviction strategies: LRU (least recently used), LFU (least frequently used), FIFO (first in first out).
- Background sweep removes expired entries at `CACHE_SWEEP_INTERVAL`.
- Key prefix namespacing for multi-tenant deployments.

## Metrics Exposed

- `cache_entries` – Current number of cache entries.
- `cache_hits_total` – Total cache hits.
- `cache_misses_total` – Total cache misses.
- `cache_evictions_total` – Total evictions.
- `cache_size_bytes` – Current cache size in bytes.

## Testing

```bash
cargo test -p cache-shim
```
