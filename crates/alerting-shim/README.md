# alerting-shim

Webhook delivery with severity routing, deduplication, and backoff. Sends alerts to configured webhooks (Slack, PagerDuty, custom).

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `ALERTING_WEBHOOKS` | JSON array of webhook configs | — |
| `ALERTING_DEDUP_WINDOW` | Dedup window in seconds | `300` |
| `ALERTING_BACKOFF_BASE` | Base backoff in seconds | `30` |
| `ALERTING_BACKOFF_MAX` | Max backoff in seconds | `3600` |

## Usage

```rust
use alerting_shim::AlertingShim;
use shim_core::Capability;

let mut shim = AlertingShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Severity levels: Info, Warning, Critical.
- Deduplication window to suppress repeated alerts.
- Exponential backoff on failing webhook endpoints.
- Configurable webhook retry count and delay.

## Metrics Exposed

- `alerting_alerts_sent_total` – Total alerts sent.
- `alerting_alerts_deduped_total` – Total alerts suppressed by dedup.
- `alerting_webhook_errors_total` – Webhook delivery failures.
- `alerting_pending_alerts` – Alerts in the queue.

## Testing

```bash
cargo test -p alerting-shim
```
