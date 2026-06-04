# EvergreenShims

Rust-native shims for building self-managing container images. Single binary, multiple capabilities, zero overhead.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=flat-square)](LICENSE)
[![Release](https://img.shields.io/github/v/release/WyattAu/EvergreenShims?style=flat-square)](https://github.com/WyattAu/EvergreenShims/releases)

## Vision

Evergreen images don't just run — they manage themselves. A single Rust binary provides health checks, secrets rotation, automated backups, schema migrations, and more. No cron jobs, no scripts, no ops tickets.

```
┌─────────────────────────────────────────────────────────┐
│  FROM scratch                                           │
│                                                         │
│  /app/db-shim (Rust, ~1MB)         ← Entry point (PID 1)│
│  /app/postgres (application)       ← Child process      │
│                                                         │
│  ✓ Health probes (/livez, /readyz, /metrics)           │
│  ✓ Auto-rotate credentials (Vault/KMS)                 │
│  ✓ Automated backups (S3-compatible)                   │
│  ✓ Schema migrations (idempotent)                      │
│  ✓ Audit logging (SIEM export)                         │
│  ✓ Auto-TLS (Let's Encrypt)                            │
│  ✓ Hot-reload config                                   │
│  ✓ Automatic failover                                  │
└─────────────────────────────────────────────────────────┘
```

## Quick Start

```bash
# Download the database shim
curl -L https://github.com/WyattAu/EvergreenShims/releases/latest/download/db-shim-x86_64-unknown-linux-musl -o /app/shim
chmod +x /app/shim

# Use in Dockerfile
FROM scratch
COPY --from=builder /app/db-shim /app/shim
COPY --from=builder /app/postgres /app/postgres
ENTRYPOINT ["/app/shim"]
```

## Pre-Built Binaries

| Binary | Features | Size | Use Case |
|--------|----------|------|----------|
| `health-shim` | health | ~300KB | Any container |
| `db-shim` | health + vault + backup + migration + audit | ~1MB | Databases |
| `proxy-shim` | health + audit + tls | ~700KB | Proxies |
| `ha-shim` | health + failover + replication | ~800KB | HA databases |
| `full-shim` | everything | ~3MB | Power users |

## Shim Catalog

### Core

| Shim | Size | Description |
|------|------|-------------|
| [health-shim](crates/health-shim/) | ~300KB | Health probes, metrics, process management |
| [vault-shim](crates/vault-shim/) | ~200KB | Auto-rotate secrets from Vault/KMS |
| [backup-shim](crates/backup-shim/) | ~300KB | Automated backups, WAL archiving, S3 upload |
| [migration-shim](crates/migration-shim/) | ~200KB | Schema migrations, rollback support |
| [audit-shim](crates/audit-shim/) | ~200KB | Query logging, SIEM export |
| [proxy-shim](crates/proxy-shim/) | ~500KB | Connection pooling, retries, circuit breaker |
| [chaos-shim](crates/chaos-shim/) | ~100KB | Fault injection for resilience testing |
| [cost-shim](crates/cost-shim/) | ~150KB | Resource tracking per tenant |

### Data Management

| Shim | Size | Description |
|------|------|-------------|
| [cache-shim](crates/cache-shim/) | ~200KB | Query result caching (Redis/Memcached) |
| [replication-shim](crates/replication-shim/) | ~300KB | Database replication management |
| [failover-shim](crates/failover-shim/) | ~250KB | Automatic failover for HA databases |
| [sharding-shim](crates/sharding-shim/) | ~400KB | Automatic sharding for distributed DBs |
| [cdc-shim](crates/cdc-shim/) | ~300KB | Change Data Capture for event-driven |
| [archival-shim](crates/archival-shim/) | ~200KB | Data archival to cold storage |

### Security

| Shim | Size | Description |
|------|------|-------------|
| [tls-shim](crates/tls-shim/) | ~200KB | Auto-TLS with Let's Encrypt or internal CA |
| [auth-shim](crates/auth-shim/) | ~300KB | Authentication/authorization layer |
| [encryption-shim](crates/encryption-shim/) | ~250KB | Transparent data encryption |
| [compliance-shim](crates/compliance-shim/) | ~200KB | CIS/STIG compliance checking |

### Operations

| Shim | Size | Description |
|------|------|-------------|
| [config-shim](crates/config-shim/) | ~150KB | Hot-reload configuration |
| [scheduler-shim](crates/scheduler-shim/) | ~150KB | Cron-like task scheduling |
| [queue-shim](crates/queue-shim/) | ~250KB | Background job processing |
| [alerting-shim](crates/alerting-shim/) | ~150KB | Alert routing (PagerDuty, Slack) |

## Configuration

All shims support environment variables (12-factor) and TOML configuration files.

### Environment Variables

```bash
# Health probes
HEALTH_CMD="exec:pg_isready -U postgres"
HEALTH_LISTEN="0.0.0.0:9101"

# Vault secrets
VAULT_ADDR="https://vault.internal:8200"
VAULT_ROLE="postgres-readonly"
VAULT_SECRET="secret/data/postgres/creds"

# Backups
BACKUP_SCHEDULE="0 2 * * *"
BACKUP_STORAGE="s3://backups-bucket"
BACKUP_RETENTION_DAYS=30

# TLS
TLS_PROVIDER="letsencrypt"
TLS_DOMAIN="postgres.example.com"
```

### TOML Configuration

```toml
# /etc/shim/config.toml
[health]
cmd = "exec:pg_isready -U postgres"
listen = "0.0.0.0:9101"

[vault]
addr = "https://vault.internal:8200"
role = "postgres-readonly"
secret = "secret/data/postgres/creds"

[backup]
schedule = "0 2 * * *"
storage = "s3://backups-bucket"
retention_days = 30

[tls]
provider = "letsencrypt"
domain = "postgres.example.com"
```

## Building from Source

```bash
# Build all shims
cargo build --release

# Build specific shim
cargo build --release -p health-shim

# Build with features
cargo build --release -p evergreen-shim --features "health,vault,backup"
```

## Architecture

See [docs/architecture.md](docs/architecture.md) for the full architecture documentation.

## Testing

See [docs/testing.md](docs/testing.md) for the testing strategy.

## Roadmap

See [docs/roadmap.md](docs/roadmap.md) for the implementation roadmap.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
