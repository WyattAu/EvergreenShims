# Production Deployment Guide

Zero-to-production walkthrough for EvergreenShims.

## Prerequisites

### Required Tools

| Tool | Version | Purpose |
|------|---------|---------|
| Docker | >= 24.0 | Container builds |
| kubectl | >= 1.28 | Kubernetes management |
| Helm | >= 3.14 | Chart deployment |
| Rust toolchain | >= 1.78 | musl static builds |
| Docker Buildx | >= 0.12 | Multi-arch images |

Install musl target:
```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
```

### Required Services

| Service | Default | Purpose |
|---------|---------|---------|
| PostgreSQL | 16+ | Primary database |
| Redis | 7+ | Cache / queue backend |
| S3 (or MinIO) | - | Backup / archival storage |

---

## Local Development

### 1. Clone the Repository

```bash
git clone https://github.com/WyattAu/EvergreenShims.git
cd EvergreenShims
```

### 2. Start Docker Compose Services

```bash
docker compose -f docker-compose.playground.yml up -d
```

This starts:
- PostgreSQL 16 on `localhost:5432`
- Redis 7 on `localhost:6379`
- MinIO (S3-compatible) on `localhost:9000` (console at `:9001`)

Verify services are healthy:
```bash
docker compose -f docker-compose.playground.yml ps
```

### 3. Build the Shim Binary

Build the health-only binary (default, smallest):
```bash
cargo build --release -p evergreen-shim --no-default-features --features health
```

Or build the full binary (all 27 shims):
```bash
cargo build --release -p evergreen-shim --no-default-features --features full
```

### 4. Build the Docker Image

```bash
docker buildx build --target shim-health -t evergreen-shims:local .
```

### 5. Verify Health Endpoints

```bash
docker compose -f docker-compose.playground.yml up health-shim -d
curl http://localhost:9101/health
curl http://localhost:9101/livez
curl http://localhost:9101/readyz
curl http://localhost:9101/metrics
```

Expected output from `/health`:
```json
{
  "status": "healthy",
  "uptime_secs": 12,
  "shims": ["health"],
  "child_pid": 1234,
  "child_status": "running"
}
```

---

## Database Shim Deployment

### 1. Build db-shim with musl Target

```bash
cargo build --release -p evergreen-shim \
  --no-default-features \
  --features "health,vault,backup,migration,audit" \
  --target x86_64-unknown-linux-musl

strip target/x86_64-unknown-linux-musl/release/shim
```

### 2. Create Dockerfile

The project already has `Dockerfile.shim-image`. For a standalone db-shim:

```dockerfile
FROM scratch
COPY --from=ghcr.io/wyattau/evergreen-shims:latest /shim /shim
USER 65532:65532
ENTRYPOINT ["/shim"]
```

### 3. Environment Variables

```bash
# Health
HEALTH_LISTEN=0.0.0.0:9101
HEALTH_CMD=pg_isready -h postgres -p 5432 -U shim_user -d mydb
HEALTH_INTERVAL_SECS=5

# Database
DB_HOST=postgres
DB_PORT=5432
DB_USER=shim_user
DB_PASSWORD=changeme
DB_NAME=mydb
PROCESS_COMMAND=postgres
PROCESS_ARGS=-D,/var/lib/postgresql/data

# Backup
BACKUP_SCHEDULE=0 2 * * *
BACKUP_STORAGE=s3
BACKUP_S3_BUCKET=my-backups
BACKUP_S3_ENDPOINT=http://minio:9000
BACKUP_S3_REGION=us-east-1
BACKUP_RETENTION_DAYS=30
BACKUP_DB_TYPE=postgres
BACKUP_DB_HOST=postgres
BACKUP_DB_PORT=5432
BACKUP_DB_USER=shim_user
BACKUP_DB_PASSWORD=changeme
BACKUP_DB_NAME=mydb
BACKUP_COMPRESSION=gzip
BACKUP_OUTPUT_DIR=/tmp/backups

# Migration
MIGRATION_DIR=/migrations
MIGRATION_AUTO_MIGRATE=true

# Audit
AUDIT_DATABASE=mydb
AUDIT_FORMAT=json
AUDIT_OUTPUT=stdout
AUDIT_LOG_DIR=/var/log/audit-shim

# Vault (optional)
VAULT_ADDR=https://vault.example.com
VAULT_ROLE=db-shim
VAULT_SECRET=secret/data/db
```

### 4. Deploy to Docker

```bash
docker buildx build --target shim-db -t evergreen-shims:db .

docker run -d \
  --name db-shim \
  -p 9101:9101 \
  --network data-tier \
  -e DB_HOST=postgres \
  -e DB_PASSWORD=changeme \
  -e BACKUP_S3_ENDPOINT=http://minio:9000 \
  -v /path/to/migrations:/migrations:ro \
  evergreen-shims:db
```

### 5. Verify

```bash
# Health
curl http://localhost:9101/health

# Metrics
curl http://localhost:9101/metrics

# Logs
docker logs db-shim --tail 50
```

---

## Kubernetes Deployment

### 1. Install Helm Chart

```bash
helm install evergreen-shim ./helm/evergreen-shims \
  --namespace evergreen \
  --create-namespace
```

Or from the registry (when published):
```bash
helm repo add evergreen https://wyattau.github.io/EvergreenShims
helm install evergreen-shim evergreen/evergreen-shims
```

### 2. Configure values.yaml

Create `values-production.yaml`:

```yaml
replicaCount: 2

image:
  repository: ghcr.io/wyattau/evergreen-shims
  tag: "latest"
  pullPolicy: Always

# Pod security
podSecurityContext:
  runAsNonRoot: true
  runAsUser: 65532
  runAsGroup: 65532
  fsGroup: 65532

securityContext:
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities:
    drop:
      - ALL

# Health
health:
  listen: "0.0.0.0:9101"

# Proxy (if using connection pooling)
proxy:
  enabled: true
  target: "postgres-service:5432"
  maxConnections: 20
  circuitThreshold: 5

# Resources
resources:
  limits:
    cpu: 200m
    memory: 256Mi
  requests:
    cpu: 50m
    memory: 128Mi

# Autoscaling
autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilizationPercentage: 70

# Metrics
metrics:
  enabled: true
  port: 9101
  path: /metrics
  serviceMonitor:
    enabled: true
    interval: 30s

# Pod disruption budget
podDisruptionBudget:
  enabled: true
  minAvailable: 1

# Extra environment
extraEnv:
  - name: DB_HOST
    value: "postgres-service"
  - name: DB_NAME
    value: "mydb"
  - name: DB_PASSWORD
    valueFrom:
      secretKeyRef:
        name: db-credentials
        key: password
  - name: BACKUP_SCHEDULE
    value: "0 2 * * *"
  - name: BACKUP_STORAGE
    value: "s3"
  - name: BACKUP_S3_BUCKET
    value: "my-backups"
  - name: BACKUP_RETENTION_DAYS
    value: "30"
```

### 3. Deploy to Cluster

```bash
helm upgrade --install evergreen-shim ./helm/evergreen-shims \
  -f values-production.yaml \
  --namespace evergreen
```

### 4. Verify Operator Reconciliation

```bash
# Check deployment status
kubectl get deployments -n evergreen

# Check pods
kubectl get pods -n evergreen -l app.kubernetes.io/name=evergreen-shims

# Check logs
kubectl logs -n evergreen deployment/evergreen-shim --tail 50

# Port-forward and check health
kubectl port-forward -n evergreen svc/evergreen-shim 9101:9101 &
curl http://localhost:9101/health
```

### 5. Check Health Endpoints

```bash
# Liveness (should return 200)
kubectl exec -n evergreen <pod> -- wget -qO- http://localhost:9101/livez

# Readiness (should return 200 when DB is connected)
kubectl exec -n evergreen <pod> -- wget -qO- http://localhost:9101/readyz

# Metrics (Prometheus format)
kubectl exec -n evergreen <pod> -- wget -qO- http://localhost:9101/metrics
```

---

## Production Checklist

### TLS Configuration

1. Enable TLS via `tls-shim`:
   ```yaml
   extraEnv:
     - name: TLS_ENABLED
       value: "true"
     - name: TLS_CERT_SOURCE
       value: "letsencrypt"  # or "self-signed" or "vault"
     - name: TLS_DOMAIN
       value: "shim.example.com"
   ```

2. Verify TLS:
   ```bash
   curl -k https://localhost:9101/health
   openssl s_client -connect localhost:9101 -servername shim.example.com
   ```

### Backup Scheduling

1. Configure in values.yaml:
   ```yaml
   extraEnv:
     - name: BACKUP_SCHEDULE
       value: "0 2 * * *"  # Daily at 2 AM
     - name: BACKUP_RETENTION_DAYS
       value: "30"
     - name: BACKUP_STORAGE
       value: "s3"
   ```

2. Verify backup runs:
   ```bash
   curl http://localhost:9101/metrics | grep backup_success_total
   ```

### Monitoring Setup (Grafana Dashboard)

1. Prometheus scrape config (via ServiceMonitor):
   ```yaml
   metrics:
     serviceMonitor:
       enabled: true
       interval: 30s
       additionalLabels:
         release: prometheus
   ```

2. Grafana dashboard import:
   ```bash
   # Import pre-built dashboard
   curl -X POST http://grafana:3000/api/dashboards/import \
     -H "Content-Type: application/json" \
     -d @grafana/dashboards/evergreen-shims.json
   ```

3. Key metrics to monitor:
   - `health_liveness` / `health_readiness`
   - `backup_success_total` / `backup_failure_total`
   - `backup_size_bytes`
   - `vault_rotation_success_total`
   - `migration_current_version`
   - `queue_processed_total` / `queue_dead_total`

### Alerting Configuration

Configure alerting-shim via env vars:
```yaml
extraEnv:
  - name: ALERTING_ENABLED
    value: "true"
  - name: ALERTING_WEBHOOKS
    value: '[{"url":"https://hooks.slack.com/...","severity":"critical"}]'
  - name: ALERTING_DEDUP_WINDOW
    value: "300"
```

Or use `alertmanager` with Prometheus rules:
```yaml
groups:
  - name: evergreen-shims
    rules:
      - alert: ShimUnhealthy
        expr: health_liveness{service="postgres"} == 0
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "EvergreenShim unhealthy for {{ $labels.service }}"
      - alert: BackupFailing
        expr: increase(backup_failure_total[1h]) > 3
        labels:
          severity: warning
```

### Log Aggregation

1. Structured JSON logs (default):
   ```bash
   # Fluentd/FluentBit filter
   <filter kubernetes.var.log.containers.**>
     @type parser
     key_name log
     reserve_data true
     <parse>
       @type json
     </parse>
   </filter>
   ```

2. Key log fields: `target`, `service`, `level`, `message`

### Resource Limits

Set per-shim type:

| Shim Type | CPU Request | CPU Limit | Memory Request | Memory Limit |
|-----------|------------|-----------|----------------|--------------|
| health | 10m | 50m | 16Mi | 64Mi |
| db (backup+migration) | 50m | 200m | 64Mi | 256Mi |
| proxy | 50m | 500m | 128Mi | 512Mi |
| full (all shims) | 100m | 500m | 128Mi | 512Mi |

### Security Hardening

1. **Non-root user:** Built into Dockerfile (`USER 65532:65532`)
2. **Read-only filesystem:** `readOnlyRootFilesystem: true`
3. **Drop capabilities:** `drop: [ALL]`
4. **Network policies:** Restrict pod-to-pod traffic
5. **Secrets:** Use Kubernetes secrets + Vault integration
6. **Image scanning:** Scan with Trivy/Snyk before deploy

```bash
trivy image ghcr.io/wyattau/evergreen-shims:latest
```

---

## Troubleshooting

### Common Issues and Solutions

| Issue | Cause | Solution |
|-------|-------|----------|
| Pod stuck in `CrashLoopBackOff` | Health check failing | Check `HEALTH_CMD` reaches the database |
| `Connection refused` on backup | DB not reachable | Verify `DB_HOST`/`DB_PORT` and network policies |
| S3 upload fails | IAM or endpoint wrong | Check `BACKUP_S3_ENDPOINT`, AWS credentials |
| Migration fails | SQL syntax error | Check migration files in `/migrations` |
| High memory usage | Too many in-memory logs | Lower `AUDIT_MAX_ENTRIES` or increase limits |
| Backup verification fails | Checksum mismatch | Check disk space; rerun backup |

### Reading Health Endpoint Output

```bash
curl http://localhost:9101/health | jq .
```

```json
{
  "status": "healthy",        // "healthy" | "degraded" | "unhealthy"
  "uptime_secs": 3600,        // seconds since start
  "shims": ["health", "vault", "backup", "migration"],
  "child_pid": 1234,          // PID of managed process (0 if no child)
  "child_status": "running",  // "running" | "exited" | "signaled"
  "child_exit_code": null,    // exit code if exited
  "checks": {                 // per-shim health checks
    "health": "ok",
    "vault": "ok",
    "backup": "ok",
    "migration": "ok"
  }
}
```

Status meanings:
- **`healthy`**: All checks pass, child process running
- **`degraded`**: Some checks fail but not critical
- **`unhealthy`**: Critical check failed or child process down

### Interpreting Metrics

```bash
# Quick summary
curl -s http://localhost:9101/metrics | grep -E "^(health_|backup_|vault_)" | head -20
```

Key metrics:
- `health_liveness{service="postgres"} 1` — liveness probe passing
- `backup_success_total 30` — total successful backups
- `backup_failure_total 0` — should be 0; alert if increasing
- `backup_size_bytes 52428800` — last backup size (50MB)
- `backup_retained_total 7` — backups in retention window
- `vault_rotation_success_total 12` — successful key rotations
- `migration_current_version 10` — current schema version

### Debug Mode Configuration

Enable verbose logging:
```yaml
extraEnv:
  - name: RUST_LOG
    value: "debug"
  - name: HEALTH_LOG_LEVEL
    value: "debug"
```

Or per-shim:
```yaml
extraEnv:
  - name: VAULT_LOG_LEVEL
    value: "trace"
  - name: BACKUP_LOG_LEVEL
    value: "trace"
  - name: AUDIT_LOG_QUERIES
    value: "true"
```

View logs:
```bash
# Docker
docker logs db-shim -f --since 1h

# Kubernetes
kubectl logs -n evergreen <pod> -f --since=1h
```
