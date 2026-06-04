# health-shim

Health probes, metrics, and process management for container images.

## Features

- **Liveness probes** — TCP, HTTP, or exec-based health checks
- **Readiness probes** — Separate liveness/readiness endpoints
- **Metrics endpoint** — Prometheus-compatible `/metrics`
- **Process management** — Signal forwarding to child processes

## Endpoints

| Endpoint | Description |
|----------|-------------|
| `GET /livez` | Liveness check (200=healthy, 503=unhealthy) |
| `GET /readyz` | Readiness check (200=ready, 503=not ready) |
| `GET /metrics` | Health metrics in JSON format |

## Configuration

### Environment Variables

```bash
HEALTH_CMD="exec:pg_isready -U postgres"    # Liveness command
HEALTH_LISTEN="0.0.0.0:9101"                 # Listen address
HEALTH_INTERVAL_SECS=10                       # Check interval
HEALTH_TIMEOUT_SECS=5                         # Check timeout
```

### Health Check Types

```bash
# TCP check
HEALTH_CMD="tcp:127.0.0.1:5432"

# HTTP check
HEALTH_CMD="http:http://127.0.0.1:8080/healthz"

# Exec check
HEALTH_CMD="exec:pg_isready -U postgres"

# Always healthy (for scratch images)
HEALTH_CMD="exec:true"
```

## Usage

```rust
use health_shim::HealthShim;
use shim_core::Capability;

let mut shim = HealthShim::new();
shim.init(&config).await?;
shim.start().await?;
```

## Dockerfile

```dockerfile
FROM scratch
COPY --from=builder /app/shim /shim
COPY --from=builder /app/postgres /app/postgres
ENTRYPOINT ["/shim"]
```
