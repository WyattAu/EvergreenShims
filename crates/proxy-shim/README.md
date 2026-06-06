# proxy-shim

Connection pooling, retries, and circuit breaker. Sits between the application and database, providing connection reuse, automatic retries with exponential backoff, and circuit breaker patterns.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `PROXY_LISTEN` | Listen address | `0.0.0.0:5432` |
| `PROXY_TARGET` | Target database address | — (required) |
| `PROXY_MAX_CONNECTIONS` | Max pool connections | `20` |
| `PROXY_MIN_IDLE` | Min idle connections | `5` |
| `PROXY_MAX_LIFETIME_SECS` | Max connection lifetime | `1800` |
| `PROXY_IDLE_TIMEOUT_SECS` | Idle connection timeout | `600` |
| `PROXY_CONNECT_TIMEOUT` | Connect timeout (seconds) | `5` |
| `PROXY_RETRY_ATTEMPTS` | Max retry attempts | `3` |
| `PROXY_RETRY_BASE_MS` | Base retry delay (ms) | `100` |
| `PROXY_CIRCUIT_THRESHOLD` | Failures before opening circuit | `5` |
| `PROXY_CIRCUIT_RESET_SECS` | Seconds before half-open | `30` |

## Usage

```rust
use proxy_shim::ProxyShim;
use shim_core::Capability;

let mut shim = ProxyShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Three circuit breaker states: Closed, Open, Half-Open.
- Exponential backoff with configurable base delay and retry count.
- Configurable connection pool sizing and idle timeouts.

## Metrics Exposed

- `proxy_connections_active` – Active connections in pool.
- `proxy_connections_idle` – Idle connections in pool.
- `proxy_retries_total` – Total retry attempts.
- `proxy_circuit_state` – Current circuit breaker state (0=closed, 1=open, 2=half-open).
- `proxy_errors_total` – Total proxy errors.

## Testing

```bash
cargo test -p proxy-shim
```
