# cost-shim

Resource tracking per tenant. Tracks resource usage (CPU, memory, storage, I/O) per tenant for billing and cost allocation.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `COST_TRACKING_ENABLED` | Enable tracking | `true` |
| `COST_TENANT_KEY` | Header/key for tenant identification | — |
| `COST_REPORT_SCHEDULE` | Report schedule | `daily` |
| `COST_BUDGET_DEFAULT` | Default budget per tenant | `100.0` |
| `COST_ALERT_THRESHOLD` | Alert at this % of budget | `80` |
| `COST_CURRENCY` | Currency code | `USD` |

## Usage

```rust
use cost_shim::CostShim;
use shim_core::Capability;

let mut shim = CostShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Resource types: CPU, Memory, Storage, NetworkIn, NetworkOut, Requests, DatabaseReads, DatabaseWrites.
- Per-tenant budget tracking with configurable alert threshold.
- Report generation on configurable schedule.

## Metrics Exposed

- `cost_usage_by_tenant` – Resource usage per tenant.
- `cost_budget_remaining` – Remaining budget per tenant.
- `cost_alerts_triggered` – Total budget alerts triggered.

## Testing

```bash
cargo test -p cost-shim
```
