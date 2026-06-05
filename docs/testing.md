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

Per-crate `#[cfg(test)]` modules. 491 tests across 25 crates.

```bash
cargo test --workspace
```

### Coverage Targets

| Metric | Requirement |
|--------|-------------|
| Branch coverage (critical paths) | >95% |
| Branch coverage (overall) | >80% |
| Line coverage | >80% |

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

## CI/CD Pipeline

GitHub Actions (`.github/workflows/ci.yml`):

1. `cargo check --workspace`
2. `cargo test --workspace`
3. `cargo fmt --all -- --check`
4. `cargo clippy --workspace --all-targets -- -D warnings`
5. Security audit (rustsec/audit-check)

## Pre-Commit Hook

Local enforcement of formatting, linting, and unit tests:

```bash
./scripts/install-hooks.sh
# Configures core.hooksPath to .githooks/
```

Hook runs: `cargo fmt --check` -> `cargo clippy -D warnings` -> `cargo test --workspace --lib`.
