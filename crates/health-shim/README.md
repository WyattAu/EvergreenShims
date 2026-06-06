# health-shim

Provides health probes, metrics, and process management for EvergreenShims.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `HEALTH_WEBHOOK_URL` | Webhook URL to push health status via POST | — |
| `HEALTH_WEBHOOK_INTERVAL_SECS` | Push interval in seconds | `30` |

## Usage

```rust
use health_shim::HealthShim;
use shim_core::Capability;

let mut shim = HealthShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- **Webhook URL**: Set `HEALTH_WEBHOOK_URL` to receive periodic health POSTs.
- **Push interval**: Control frequency via `HEALTH_WEBHOOK_INTERVAL_SECS`.
- Default listen address: `0.0.0.0:9101`.

## Metrics Exposed

- `health_shim_up` – 1 when the shim is running.
- `health_shim_checks_total` – Total health checks executed.
- `health_shim_webhook_errors_total` – Webhook delivery failures.

## Testing

```bash
cargo test -p health-shim
```
