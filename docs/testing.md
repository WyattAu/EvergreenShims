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

Per-crate `#[cfg(test)]` modules. 970 tests across 34 crates.

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
| shim-core | 175 | Bus, config, events, health, metrics, process, shutdown, signal, wiring |
| integration-tests | 87 | Cross-shim event chains, real DB operations |
| proxy-shim | 25 | Circuit breaker, rate limiting, load balancing, routing |
| tls-shim | 33 | ACME, self-signed, Vault PKI, PEM handling, cert validation |
| backup-shim | 31 | Checksum, S3 upload, retention, verification |
| migration-shim | 30 | Checksum, parsing, apply, rollback, multi-DB orchestration |
| chaos-shim | 32 | Fault injection, experiments, blast radius, orchestrator |
| auth-shim | 22 | HMAC tokens, API keys, passwords, RBAC, lockout |
| audit-shim | 21 | Disk persistence, log rotation, webhook, CEF format |
| evergreen-shim | 21 | Binary lifecycle, feature flags, critical capabilities |
| failover-shim | 19 | TCP/Patroni/RedisSentinel connectors, state machine |
| replication-shim | 19 | Replica management, WAL tracking, state transitions |
| cost-shim | 26 | Budgets, billing, cost projection, optimizer |
| cache-shim | 16 | LRU/LFU/FIFO eviction, TTL, prefix invalidation |
| queue-shim | 16 | Worker pool, retry, dead-letter queue, timeout |
| mongodb-shim | 16 | Health, backup, env overrides, serialization |
| cockroachdb-shim | 15 | Connection strings, env overrides, metrics |
| alerting-shim | 14 | Deduplication, backoff, severity routing |
| cassandra-shim | 14 | Host/port/cluster config, JMX, serialization |
| cdc-shim | 13 | WAL advancement, batch publishing, ring buffer |
| scheduler-shim | 13 | Cron parsing, retry/jitter, task execution |
| health-shim | 20 | HealthChecker, mock health checks, metrics, lifecycle |
| dynamodb-shim | 12 | Region/table, endpoint, metrics, serialization |
| elasticsearch-shim | 11 | URL/index, snapshot repo, metrics, lifecycle |
| compliance-shim | 12 | CIS/STIG rules, violation tracking, severity filtering |
| encryption-shim | 11 | AES-GCM, ChaCha20, key rotation, AAD |
| vault-shim | 10 | Defaults, env overrides, TLS skip verify, metrics |
| config-shim | 11 | SHA-256 hashing, validation, backup, signal parsing |
| sharding-shim | 12 | Hash ring, range, directory routing |
| management-api | 5 | Status, metrics, capability listing |
| benchmarks | 16 | Criterion output parsing, regression detection |

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

| Operation | Measured | Tolerance |
|-----------|----------|-----------|
| SHA-256 checksum (1MB) | ~1.5ms | 20% |
| Cache set (1K keys) | ~5ms | 20% |
| Cache get (1K keys) | ~3ms | 20% |
| AES-GCM encrypt (4KB) | ~32us | 25% |
| AES-GCM decrypt (4KB) | ~32us | 25% |
| Migration checksum | ~195ns | 25% |

## CI/CD Pipeline

GitHub Actions (`.github/workflows/ci.yml`):

1. `cargo check --workspace` -- Compile verification
2. `cargo test --workspace` -- Full test suite
3. `cargo fmt --all -- --check` -- Formatting enforcement
4. `cargo clippy --workspace --all-targets -- -D warnings` -- Lint enforcement
5. `rustsec/audit-check` -- Known vulnerability scanning
6. `cargo-deny` -- License, advisory, and ban checking
7. Benchmark regression detection -- Performance regression gating
8. Binary size threshold enforcement -- 3MB for health-shim (musl, stripped)

## Pre-Commit Hooks

Local enforcement of formatting, linting, unit tests, and secret scanning:

```bash
./scripts/install-hooks.sh
# Configures core.hooksPath to .githooks/
```

### Hooks

| Hook | Purpose | Failure Mode |
|------|---------|--------------|
| `pre-commit` | fmt, clippy, unit tests, secret scan, unwrap detection, dead_code check | Hard fail (except unwrap warning) |
| `commit-msg` | Conventional Commits format enforcement | Hard fail |
| `pre-push` | Compile check, unit tests | Hard fail |
