# Testing Strategy

## Overview

Every shim is tested at three levels:
1. **Unit tests** — Individual functions and modules
2. **Integration tests** — Interaction with real databases
3. **Chaos tests** — Fault injection and resilience

## Unit Tests

Each crate has its own unit tests in `src/` files. Run with:

```bash
cargo test --workspace
```

### Coverage Requirements

| Metric | Requirement |
|--------|-------------|
| Line coverage | >80% |
| Branch coverage | >70% |
| Critical path coverage | >95% |

## Integration Tests

Integration tests use Docker Compose to spin up real databases and test shims against them.

### Test Matrix

| Shim | PostgreSQL | MariaDB | MySQL | MongoDB | Redis |
|------|------------|---------|-------|---------|-------|
| health-shim | ✓ | ✓ | ✓ | ✓ | ✓ |
| vault-shim | ✓ | ✓ | ✓ | ✓ | ✓ |
| backup-shim | ✓ | ✓ | ✓ | - | ✓ |
| migration-shim | ✓ | ✓ | ✓ | - | - |
| audit-shim | ✓ | ✓ | ✓ | - | - |
| proxy-shim | ✓ | ✓ | ✓ | - | ✓ |
| failover-shim | ✓ | ✓ | ✓ | - | - |
| tls-shim | ✓ | ✓ | ✓ | ✓ | ✓ |
| config-shim | ✓ | ✓ | ✓ | ✓ | ✓ |
| cache-shim | ✓ | ✓ | ✓ | - | ✓ |
| cdc-shim | ✓ | ✓ | - | - | - |
| replication-shim | ✓ | ✓ | - | - | - |

### Running Integration Tests

```bash
# All integration tests
cargo test --workspace --features integration

# Specific database
cargo test --workspace --features integration-postgres

# Specific shim
cargo test --workspace -p failover-shim --features integration
```

### MariaDB Failover Test

This is the flagship integration test, validating failover-shim with real MariaDB:

```rust
// tests/integration/failover_mariadb.rs

#[tokio::test]
async fn test_mariadb_failover() {
    // 1. Start primary MariaDB
    let primary = start_mariadb("primary").await;
    
    // 2. Start replica MariaDB
    let replica = start_mariadb("replica").await;
    
    // 3. Start failover-shim
    let shim = start_failover_shim(FailoverConfig {
        primary: primary.addr(),
        replica: replica.addr(),
        check_interval: Duration::from_secs(1),
        timeout: Duration::from_secs(5),
    }).await;
    
    // 4. Verify primary is healthy
    assert!(shim.is_primary_healthy().await);
    
    // 5. Kill primary
    primary.kill().await;
    
    // 6. Wait for failover
    tokio::time::sleep(Duration::from_secs(10)).await;
    
    // 7. Verify replica is now primary
    assert!(shim.is_primary_healthy().await);
    assert_eq!(shim.current_primary().await, replica.addr());
    
    // 8. Verify metrics
    let metrics = shim.metrics().await;
    assert!(metrics.contains("failover_events_total 1"));
}
```

### Test Infrastructure

```bash
# Start test databases
docker compose -f tests/docker-compose.yml up -d

# Run tests
cargo test --workspace --features integration

# Stop test databases
docker compose -f tests/docker-compose.yml down -v
```

## Chaos Tests

Chaos tests inject faults and verify shim resilience.

### Fault Injection

| Fault | Method | Expected Behavior |
|-------|--------|-------------------|
| Network partition | `iptables -A INPUT -s <ip> -j DROP` | Shim detects failure, triggers failover |
| Process crash | `kill -9 <pid>` | Shim detects crash, restarts child |
| Disk full | `dd if=/dev/zero of=/tmp/full bs=1M` | Shim logs error, continues running |
| Memory pressure | `stress-ng --vm 1 --vm-bytes 1G` | Shim stays within limits |
| CPU starvation | `stress-ng --cpu 4` | Shim continues health checks |

### Running Chaos Tests

```bash
# Requires root for iptables
sudo cargo test --workspace --features chaos

# Specific fault
sudo cargo test --workspace --features chaos --test network_partition
```

## Performance Tests

Performance tests measure shim overhead.

### Metrics

| Metric | Target |
|--------|--------|
| Memory overhead | <10MB |
| CPU overhead | <1% |
| Latency overhead | <1ms per health check |
| Startup time | <100ms |

### Running Performance Tests

```bash
cargo bench --workspace
```

## Test Data Management

### Test Vectors

Test vectors are stored in `tests/vectors/` as TOML files:

```toml
# tests/vectors/failover_mariadb.toml

[metadata]
name = "MariaDB Failover Test"
description = "Test automatic failover with MariaDB primary-replica setup"

[setup]
primary_image = "mariadb:11"
replica_image = "mariadb:11"
primary_env = { MYSQL_ROOT_PASSWORD = "test", MYSQL_DATABASE = "testdb" }
replica_env = { MYSQL_ROOT_PASSWORD = "test", MYSQL_DATABASE = "testdb" }

[steps]
step_1 = { action = "start_primary", expected = "healthy" }
step_2 = { action = "start_replica", expected = "healthy" }
step_3 = { action = "start_shim", expected = "running" }
step_4 = { action = "kill_primary", expected = "killed" }
step_5 = { action = "wait_failover", expected = "replica_promoted", timeout = "30s" }

[assertions]
primary_killed = true
replica_promoted = true
failover_duration = "< 30s"
metrics_updated = true
```

### Test Data Cleanup

All test data is cleaned up automatically after each test run:

```bash
# Manual cleanup
docker compose -f tests/docker-compose.yml down -v
rm -rf tests/target/
```

## CI/CD Integration

### GitHub Actions

```yaml
# .github/workflows/test.yml

name: Test

on: [push, pull_request]

jobs:
  unit-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --workspace

  integration-tests:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:17
        env:
          POSTGRES_PASSWORD: test
        ports: ['5432:5432']
      mariadb:
        image: mariadb:11
        env:
          MYSQL_ROOT_PASSWORD: test
        ports: ['3306:3306']
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --workspace --features integration

  chaos-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: sudo cargo test --workspace --features chaos
```

## Test Reporting

Test results are exported in JUnit XML format for CI/CD integration:

```bash
cargo test --workspace --format junit -o test-results.xml
```
