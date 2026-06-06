# queue-shim

In-memory job queue with worker pool, retry, and dead-letter queue. Manages background job processing with configurable worker count and exponential backoff retries.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `QUEUE_BACKEND` | Backend: `memory` | `memory` |
| `QUEUE_MAX_WORKERS` | Max concurrent workers | `4` |
| `QUEUE_MAX_RETRIES` | Max job retries | `3` |
| `QUEUE_RETRY_BASE_SECS` | Base retry delay in seconds | `5` |
| `QUEUE_RETRY_MAX_SECS` | Max retry delay in seconds | `300` |
| `QUEUE_JOB_TIMEOUT_SECS` | Job timeout in seconds | `300` |

## Usage

```rust
use queue_shim::QueueShim;
use shim_core::Capability;

let mut shim = QueueShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- In-memory backend for single-node deployments.
- Configurable worker pool size.
- Exponential backoff retry with configurable base and max delays.
- Dead-letter queue for jobs that exhaust retries.

## Metrics Exposed

- `queue_jobs_pending` – Jobs waiting to be processed.
- `queue_jobs_running` – Currently executing jobs.
- `queue_jobs_completed_total` – Total completed jobs.
- `queue_jobs_failed_total` – Total failed jobs.
- `queue_jobs_dead_total` – Jobs in the dead-letter queue.

## Testing

```bash
cargo test -p queue-shim
```
