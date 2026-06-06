# EvergreenShims

Rust-native shims for self-managing container images. Single binary, multiple capabilities, zero runtime overhead.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square)](LICENSE)
[![Release](https://img.shields.io/github/v/release/WyattAu/EvergreenShims?style=flat-square)](https://github.com/WyattAu/EvergreenShims/releases)

## Design

Evergreen images manage themselves. A single Rust binary executes as PID 1 in a `scratch` container, wrapping the application process while providing operational capabilities: health probing, secrets rotation, automated backups, schema migrations, failover, and more. No cron, no scripts, no external orchestration.

```
FROM scratch

/app/db-shim        (Rust, ~2.5MB)   PID 1 -- entry point
/app/postgres       (application)     PID N -- child process

Capabilities:
  Health probes:       /livez, /readyz, /metrics
  Secrets rotation:    Vault/KMS integration
  Backups:             S3-compatible, WAL archiving
  Migrations:          Idempotent SQL file-based
  Audit logging:       JSON/CEF, SIEM export
  Auto-TLS:            Let's Encrypt / internal CA
  Config hot-reload:   SHA-256 hash change detection
  Failover:            Automatic primary promotion
```

## Pre-Built Binaries

| Binary | Feature Set | Size | Target |
|--------|-------------|------|--------|
| `health-shim` | health | ~300KB | Any container |
| `db-shim` | health + vault + backup + migration + audit | ~1MB | Database containers |
| `proxy-shim` | health + audit + tls | ~700KB | Reverse proxies |
| `ha-shim` | health + failover + replication | ~800KB | HA database clusters |
| `full-shim` | all 27 shims | ~3MB | Full operational stack |

## Quick Start

```dockerfile
FROM scratch
COPY --from=builder /app/db-shim /app/shim
COPY --from=builder /app/postgres /app/postgres
ENTRYPOINT ["/app/shim"]
```

Binary download:

```bash
curl -L https://github.com/WyattAu/EvergreenShims/releases/latest/download/shim.gz | gunzip > /app/shim
chmod +x /app/shim
```

## Shim Catalog

### Core

| Shim | Description |
|------|-------------|
| [health-shim](crates/health-shim/) | Health probes (TCP/HTTP/exec), Prometheus metrics, child process management |
| [vault-shim](crates/vault-shim/) | Automatic credential rotation from Vault/KMS |
| [backup-shim](crates/backup-shim/) | pg_dump, mysqldump, BGSAVE, mongodump -- compression, retention, S3 upload |
| [migration-shim](crates/migration-shim/) | SQL file-based (.up.sql/.down.sql), version tracking, multi-DB |
| [audit-shim](crates/audit-shim/) | Query logging, JSON/CEF formats, file/webhook output |
| [proxy-shim](crates/proxy-shim/) | Connection pooling, circuit breaker, weighted routing, retries |
| [chaos-shim](crates/chaos-shim/) | Fault injection for resilience testing |
| [cost-shim](crates/cost-shim/) | Per-tenant resource usage tracking, budget alerts |

### Data Management

| Shim | Description |
|------|-------------|
| [cache-shim](crates/cache-shim/) | Query result caching (TTL, LRU/FIFO eviction) |
| [replication-shim](crates/replication-shim/) | WAL tracking, lag monitoring, state management |
| [failover-shim](crates/failover-shim/) | Patroni/Redis Sentinel/TCP health checks, automatic promotion |
| [sharding-shim](crates/sharding-shim/) | Hash-based and range-based shard routing |
| [cdc-shim](crates/cdc-shim/) | Change Data Capture, WAL position tracking, Kafka/webhook output |
| [archival-shim](crates/archival-shim/) | Lifecycle tiers (hot/warm/cold), compression, purge scheduling |

### Security

| Shim | Description |
|------|-------------|
| [tls-shim](crates/tls-shim/) | Auto-TLS with Let's Encrypt ACME or internal CA |
| [auth-shim](crates/auth-shim/) | Token-based auth, API key management, RBAC |
| [encryption-shim](crates/encryption-shim/) | AES-GCM / ChaCha20-Poly1305, key rotation |
| [compliance-shim](crates/compliance-shim/) | CIS/STIG compliance scoring and violation tracking |

### Operations

| Shim | Description |
|------|-------------|
| [config-shim](crates/config-shim/) | File hash monitoring, backup, SIGHUP reload |
| [scheduler-shim](crates/scheduler-shim/) | Cron-based task scheduling, retry with exponential backoff |
| [queue-shim](crates/queue-shim/) | Job enqueue/dequeue, DLQ, worker pool |
| [alerting-shim](crates/alerting-shim/) | Severity routing, deduplication, webhook dispatch |

### Database-Specific

| Shim | Description |
|------|-------------|
| [mongodb-shim](crates/mongodb-shim/) | MongoDB health checks (mongosh), backup (mongodump) |
| [cockroachdb-shim](crates/cockroachdb-shim/) | CockroachDB topology awareness, cluster health |
| [dynamodb-shim](crates/dynamodb-shim/) | DynamoDB table monitoring, backup exports |
| [elasticsearch-shim](crates/elasticsearch-shim/) | Elasticsearch cluster health, snapshot management |
| [cassandra-shim](crates/cassandra-shim/) | Cassandra cluster monitoring via nodetool |

## Configuration

12-factor: environment variables override TOML config file (`/etc/shim/config.toml`).

```bash
# Health
HEALTH_CMD="exec:pg_isready -U postgres"
HEALTH_LISTEN="0.0.0.0:9101"

# Vault
VAULT_ADDR="https://vault.internal:8200"
VAULT_ROLE="postgres-readonly"

# Backups
BACKUP_SCHEDULE="0 2 * * *"
BACKUP_STORAGE="s3://backups-bucket"
BACKUP_RETENTION_DAYS=30
```

## Building

```bash
# Static musl binary (for scratch images)
cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --no-default-features --features db-shim

# Cross-compile aarch64
rustup target add aarch64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl \
    -p evergreen-shim --no-default-features --features health
```

See [docs/building.md](docs/building.md) for full build instructions.

## Testing

792 tests across 32 crates. Three tiers:

1. **Unit tests**: Per-crate, run with `cargo test --workspace`
2. **Integration tests**: Docker Compose with PostgreSQL, MariaDB, Redis, Vault, MinIO
3. **Chaos tests**: Fault injection (network, process, disk, CPU)

See [docs/testing.md](docs/testing.md) for the testing strategy.

## Architecture

See [docs/architecture.md](docs/architecture.md) for system design, layered architecture, and the Capability trait specification.

## License

Apache License, Version 2.0. See [LICENSE](LICENSE).
