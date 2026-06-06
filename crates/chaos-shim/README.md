# chaos-shim

Fault injection for resilience testing. Injects faults (latency, errors, partitions, process kill, disk fill, CPU stress) to test application resilience.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `CHAOS_ENABLED` | Enable chaos | `false` |
| `CHAOS_LATENCY_MS` | Add latency to requests | `0` |
| `CHAOS_ERROR_RATE` | Error rate 0.0–1.0 | `0.0` |
| `CHAOS_PARTITION` | Simulate network partition | `false` |
| `CHAOS_KILL_PROBABILITY` | Probability of killing process | `0.0` |
| `CHAOS_DURATION_SECS` | How long chaos lasts | `60` |
| `CHAOS_TARGET` | Target to apply chaos to | `all` |
| `CHAOS_BLAST_RADIUS` | Blast radius as percentage 0.0–1.0 | `1.0` |
| `CHAOS_REAL_FAULTS` | Enable real fault injection via system commands | `false` |
| `CHAOS_IPTABLES_CHAIN` | iptables chain for network partition | `INPUT` |
| `CHAOS_DISK_FILL_PATH` | Path for disk fill fault | `/tmp/chaos-fill` |
| `CHAOS_DISK_FILL_SIZE` | Size in MB for disk fill | `100` |

## Usage

```rust
use chaos_shim::ChaosShim;
use shim_core::Capability;

let mut shim = ChaosShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Controlled blast radius to limit scope of injected faults.
- Duration-limited chaos windows.
- Real fault injection (iptables, disk fill) or simulated.
- Probability-based fault injection.

## Metrics Exposed

- `chaos_faults_injected_total` – Total faults injected.
- `chaos_active` – 1 if chaos is currently active.
- `chaos_faults_by_type` – Faults injected by type.

## Testing

```bash
cargo test -p chaos-shim
```
