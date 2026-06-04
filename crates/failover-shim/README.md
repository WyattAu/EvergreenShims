# failover-shim

Automatic failover for high-availability databases.

## Features

- **Health monitoring** — TCP-based primary health checks
- **Automatic failover** — Promotes replica after consecutive failures
- **Webhook notifications** — Slack, PagerDuty, custom webhooks
- **State machine** — Healthy → Suspect → FailingOver → FailedOver

## How It Works

```
┌─────────┐     ┌─────────┐     ┌─────────┐
│ Primary │────▶│ Replica │     │ Webhook │
└────┬────┘     └────┬────┘     └────┬────┘
     │                │                │
     │  Health Check  │                │
     │◀───────────────│                │
     │                │                │
     │  Failover!     │                │
     │───────────────▶│                │
     │                │  Notify        │
     │                │───────────────▶│
```

## Configuration

### Environment Variables

```bash
FAILOVER_PRIMARY="127.0.0.1:3306"          # Primary address
FAILOVER_REPLICA="127.0.0.1:3307"          # Replica address
FAILOVER_CHECK_INTERVAL=5                   # Check every 5 seconds
FAILOVER_TIMEOUT=3                          # Timeout per check
FAILOVER_FAILURE_THRESHOLD=3                # Failover after 3 failures
FAILOVER_WEBHOOK="https://hooks.slack.com/..."  # Notification URL
FAILOVER_DB_TYPE="mariadb"                  # Database type
```

## State Machine

| State | Description |
|-------|-------------|
| `Healthy` | Primary is reachable |
| `Suspect` | Some checks failed |
| `FailingOver` | Failover in progress |
| `FailedOver` | Replica is now primary |
| `Recovered` | Original primary is back |

## Usage

```rust
use failover_shim::FailoverShim;
use shim_core::Capability;

let mut shim = FailoverShim::new();
shim.init(&config).await?;
shim.start().await?;  // Starts monitoring loop
```

## Metrics

| Metric | Description |
|--------|-------------|
| `failover_state` | Current state (0=healthy, 3=failed over) |
| `failover_events_total` | Total failover events |
| `failover_consecutive_failures` | Current failure count |
