# Roadmap

Current state: v0.3.0 -- 30 crates, 577 tests, all CI/CD pipelines hardened.

## v1.0.0 Completion

### Completed

| Task | Version | Tests |
|------|---------|-------|
| vault-shim TLS fix | v0.4.0 | +10 |
| S3 upload in backup-shim | v0.4.0 | 23 existing |
| S3 in archival-shim | v0.4.0 | 14 existing |
| PostgreSQL via sqlx | v0.4.0 | 22 existing |
| Backup retry with backoff | v0.4.0 | 23 existing |
| Migration dry-run mode | v0.4.0 | 22 existing |
| Patroni/Redis Sentinel promotion | v0.5.0 | 19 existing |
| Let's Encrypt ACME flow | v0.5.0 | 24 existing |
| Real chaos fault injection | v0.5.0 | 19 existing |
| Real TCP proxy (proxy-shim) | v0.5.0 | 25 existing |
| CIS/STIG rules (compliance-shim) | v0.5.0 | 12 existing |
| Real CDC output (Kafka/NATS) | v0.5.0 | 13 existing |
| MongoDB shim | v0.6.0 | +10 (16 total) |
| CockroachDB shim | v0.6.0 | +8 (15 total) |
| DynamoDB shim | v0.6.0 | +8 (12 total) |
| Elasticsearch shim | v0.6.0 | +7 (11 total) |
| Cassandra shim | v0.6.0 | +10 (14 total) |
| API stability policy | v1.0.0 | 0 |
| Benchmark suite | v1.0.0 | 0 |
| health-shim unit tests | v0.7.0 | +12 (12 total) |
| CI/CD pipeline hardening | v0.7.0 | 0 |
| Pre-commit hook hardening | v0.7.0 | 0 |
| Documentation overhaul | v0.7.0 | 0 |

### Total Tests: 577 (up from 491)

### Remaining v1.0.0 Items

| Task | Status | Priority |
|------|--------|----------|
| Multi-DB migration orchestration | Pending | Medium |
| Performance regression CI gates | Pending | Medium |
| Complete per-shim READMEs | Low | Low |

## v1.1.0 Post-Launch

| Task | Description | Priority |
|------|-------------|----------|
| OpenTelemetry integration | Distributed tracing via OTLP | High |
| Webhook-based health export | Push health status to external systems | Medium |
| Migration lock files | Prevent concurrent migrations across replicas | Medium |
| Backup verification | Restore-and-test after backup completion | High |
| Config schema validation | TOML schema enforcement at init | Medium |
| Structured logging | JSON-formatted tracing output | Medium |
| Graceful degradation | Continue operation when non-critical shims fail | High |

## v1.2.0 Scaling

| Task | Description | Priority |
|------|-------------|----------|
| Redis event bridge (multi-container) | Cross-pod event propagation via Redis Streams | High |
| Kubernetes operator | Custom CRD for shim configuration | Medium |
| Sidecar injection | Automatic shim injection via mutating webhook | Medium |
| Resource quotas | Per-shim CPU/memory limits | Medium |
| Hot configuration reload | TOML file watch with zero-downtime reconfiguration | Low |

## v2.0.0 Advanced Features

| Task | Description | Priority |
|------|-------------|----------|
| WASM shim targets | Browser/Edge computing via wasm32-wasi | Low |
| gRPC management API | Programmatic shim control and status | Medium |
| Multi-tenancy isolation | Per-tenant shims with resource isolation | High |
| Chaos engineering platform | Full fault injection orchestration | Medium |
| Cost optimization recommendations | ML-based resource right-sizing | Low |

## Architecture Summary

### 30 Crates

| Category | Crates |
|----------|--------|
| Core | shim-core |
| Data | backup, migration, cache, cdc, sharding, archival, replication, failover |
| Security | vault, tls, auth, encryption, compliance |
| Operations | health, config, scheduler, queue, alerting, chaos, cost |
| Proxy | proxy |
| Databases | mongodb, cockroachdb, dynamodb, elasticsearch, cassandra |
| Unified | evergreen-shim |
| Testing | integration-tests, benchmarks |

### Performance Baselines (from benchmarks)

| Operation | Throughput |
|-----------|------------|
| SHA-256 checksum (1MB) | ~220ns |
| Cache set (1K keys) | ~23ms |
| Cache get (1K keys) | ~14ms |
| AES-GCM encrypt (4KB) | ~52us |
| AES-GCM decrypt (4KB) | ~53us |
| Migration checksum | ~224ns |

### Quality Metrics

| Metric | Current | Target |
|--------|---------|--------|
| Unit tests | 577 | >600 |
| Clippy warnings | 0 | 0 |
| Unsafe code | 0 | 0 |
| Pre-commit checks | 5 | 5 |
| CI pipeline jobs | 7 | 7 |
| Binary size (health) | ~300KB | <500KB |

## Risk Register

| Risk | Status | Mitigation |
|------|--------|------------|
| musl/aarch64 build | Mitigated | CI builds both architectures |
| Supply chain | Mitigated | rustsec audit + cosign signing + SBOM |
| Performance regression | Mitigated | Criterion benchmarks + CI gates |
| Scope creep | Mitigated | Feature freeze at v1.0.0 |
| Docker Hub availability | Known | Retry logic in CI, alternate registries |
| aarch64 aws-lc-rs cross-compile | Known | Limited to health-shim for aarch64-musl |

## Release Strategy

| Version | Scope | Timeline |
|---------|-------|----------|
| v0.7.0 | Audit, refactor, CI hardening | Current |
| v1.0.0 | Production-ready, API stable | Pending remaining items |
| v1.1.0 | Post-launch improvements | +2 weeks |
| v1.2.0 | Scaling features | +1 month |
| v2.0.0 | Advanced features | +3 months |
