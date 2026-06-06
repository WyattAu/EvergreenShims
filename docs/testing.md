# Testing Strategy

## Test Pyramid

Three tiers, executed in order of cost and scope:

```
        +-----------+
        |  Chaos    |   Fault injection, resilience validation
        +-----------+
        | Integration|   Cross-shim wiring, Docker Compose services
        +-----------+
        |   Unit    |   Per-crate, deterministic, fast
        +-----------+
```

## Unit Tests

Per-crate `#[cfg(test)]` modules. 577 tests across 30 crates.

```bash
cargo test --workspace
```

### Coverage Targets

| Metric | Requirement |
|--------|-------------|
| Branch coverage (critical paths) | >95% |
| Branch coverage (overall) | >80% |
| Line coverage | >80% |

### Test Distribution

| Crate | Tests | Focus |
|-------|-------|-------|
| shim-core | 78 | Bus, config, events, health, metrics, process, shutdown, signal, wiring |
| integration-tests | 73 | Cross-shim event chains, real DB operations |
| proxy-shim | 25 | Circuit breaker, rate limiting, load balancing, routing |
| tls-shim | 24 | ACME, self-signed, Vault PKI, PEM handling, cert validation |
| migration-shim | 22 | Checksum, parsing, apply, rollback, multi-DB orchestration |
| auth-shim | 22 | HMAC tokens, API keys, passwords, RBAC, lockout |
| audit-shim | 21 | Disk persistence, log rotation, webhook, CEF format |
| failover-shim | 19 | TCP/Patroni/RedisSentinel connectors, state machine |
| replication-shim | 19 | Replica management, WAL tracking, state transitions |
| chaos-shim | 19 | Fault injection, experiments, blast radius |
| cache-shim | 16 | LRU/LFU/FIFO eviction, TTL, prefix invalidation |
| queue-shim | 16 | Worker pool, retry, dead-letter queue, timeout |
| cockroachdb-shim | 15 | Connection strings, env overrides, metrics |
| alerting-shim | 14 | Deduplication, backoff, severity routing |
| cost-shim | 14 | Budgets, billing, cost projection, alerts |
| cassandra-shim | 14 | Host/port/cluster config, JMX, serialization |
| cdc-shim | 13 | WAL advancement, batch publishing, ring buffer |
| scheduler-shim | 13 | Cron parsing, retry/jitter, task execution |
| health-shim | 12 | HealthChecker, mock health checks, metrics, lifecycle |
| mongodb-shim | 16 | Health, backup, env overrides, serialization |
| dynamodb-shim | 12 | Region/table, endpoint, metrics, serialization |
| elasticsearch-shim | 11 | URL/index, snapshot repo, metrics, lifecycle |
| compliance-shim | 12 | CIS/STIG rules, violation tracking, severity filtering |
| encryption-shim | 11 | AES-GCM, ChaCha20, key rotation, AAD |
| vault-shim | 10 | Defaults, env overrides, TLS skip verify, metrics |
| config-shim | 11 | SHA-256 hashing, validation, backup, signal parsing |
| sharding-shim | 12 | Hash ring, range, directory routing |

## Integration Tests

Crate: `crates/integration-tests/`

Tests exercise cross-shim event wiring, DB connector configuration, and lifecycle management.

### Test Matrix

| Component | PostgreSQL | MariaDB | Redis | Vault | MinIO |
|-----------|------------|---------|-------|-------|-------|
| health-shim | Y | Y | Y | Y | Y |
| backup-shim | Y | Y | Y | - | Y |
| migration-shim | Y | Y | - | - | - |
| failover-shim | Y | Y | - | - | - |
| replication-shim | Y | Y | - | - | - |
| vault-shim | - | - | - | Y | - |

### Infrastructure

```bash
docker compose -f tests/docker-compose.yml up -d
# Services: PostgreSQL 17, MariaDB 11, Redis 7, Vault 1.15, MinIO
```

### Key Integration Tests

- `test_cross_shim_health_to_failover`: health -> bus -> failover event chain
- `test_cross_shim_scheduler_to_backup`: scheduler -> bus -> backup trigger
- `test_cross_shim_backup_to_encryption`: backup -> bus -> key rotation
- `test_cross_shim_alert_fan_in`: multi-source alert aggregation
- `test_wire_all_handlers`: validates all pre-wired handlers activate

## Chaos Tests

Fault injection via `chaos-shim`:

| Fault Type | Injection Method | Detection |
|------------|------------------|-----------|
| Latency injection | `apply_latency(ms)` | Timeout detection |
| Error injection | `set_error_rate(0.1)` | Error rate monitoring |
| Partition simulation | Target PID mismatch | State machine transition |

## Performance Targets

| Metric | Target |
|--------|--------|
| Memory overhead (idle) | <10MB |
| CPU overhead | <1% |
| Health check latency | <1ms |
| Startup time | <100ms |

## Performance Baselines (from benchmarks)

| Operation | Throughput |
|-----------|------------|
| SHA-256 checksum (1MB) | ~220ns |
| Cache set (1K keys) | ~23ms |
| Cache get (1K keys) | ~14ms |
| AES-GCM encrypt (4KB) | ~52us |
| AES-GCM decrypt (4KB) | ~53us |
| Migration checksum | ~224ns |

## CI/CD Pipeline

GitHub Actions (`.github/workflows/ci.yml`):

1. `cargo check --workspace`
2. `cargo test --workspace`
3. `cargo fmt --all -- --check`
4. `cargo clippy --workspace --all-targets -- -D warnings`
5. Security audit (rustsec/audit-check)
6. Benchmark build verification
7. Binary size threshold enforcement (500KB for health-shim)

## Pre-Commit Hook

Local enforcement of formatting, linting, unit tests, and secret scanning:

```bash
./scripts/install-hooks.sh
# Configures core.hooksPath to .githooks/
```

Hook runs: `cargo fmt --check` -> `cargo clippy -D warnings` -> `cargo test --workspace --lib` -> secret scan -> unwrap check.
