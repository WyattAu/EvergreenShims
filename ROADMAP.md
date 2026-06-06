# Roadmap

Current state: v2.0.0 -- 32 crates, 742 tests, all CI/CD pipelines hardened.

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
| Performance regression CI gates | v1.0.0 | +16 (benchmark lib) |
| Graceful degradation | v1.0.0 | +11 (integration) |
| Multi-DB migration orchestration | v1.0.0 | existing |

### Total Tests: 742 (up from 491)

## v1.1.0 Post-Launch

| Task | Status | Tests |
|------|--------|-------|
| OpenTelemetry integration | Completed | +6 (structured_logging + otel) |
| Webhook-based health export | Completed | +8 (health-shim) |
| Migration lock files | Completed | +8 (migration-shim) |
| Backup verification | Completed | +6 (backup-shim) |
| Config schema validation | Completed | +20 (shim-core + 6 shims) |
| Structured logging | Completed | +5 (shim-core) |
| Graceful degradation | Completed | +11 (integration-tests) |

## v1.2.0 Scaling

| Task | Status | Tests |
|------|--------|-------|
| Redis event bridge completion | Completed | +5 (shim-core) |
| Resource quotas | Completed | +17 (shim-core) |
| Hot configuration reload integration | Completed | +8 (shim-core) |
| Kubernetes operator manifests | Completed | 0 (YAML) |
| Sidecar injection | Completed | 0 (YAML) |

## v2.0.0 Advanced Features

| Task | Status | Tests |
|------|--------|-------|
| WASM shim targets | Completed | +7 (shim-core) |
| gRPC management API | Completed | +5 (management-api) |
| Multi-tenancy isolation | Completed | +24 (shim-core) |
| Chaos engineering platform | Completed | +13 (chaos-shim) |
| Cost optimization recommendations | Completed | +12 (cost-shim) |
| Per-shim READMEs (27) | Completed | 0 (docs) |

## Architecture Summary

### 32 Crates

| Category | Crates |
|----------|--------|
| Core | shim-core |
| Data | backup, migration, cache, cdc, sharding, archival, replication, failover |
| Security | vault, tls, auth, encryption, compliance |
| Operations | health, config, scheduler, queue, alerting, chaos, cost |
| Proxy | proxy |
| Databases | mongodb, cockroachdb, dynamodb, elasticsearch, cassandra |
| Unified | evergreen-shim |
| Management | management-api |
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
| Unit tests | 742 | >700 |
| Clippy warnings | 0 | 0 |
| Unsafe code | 0 | 0 |
| Pre-commit checks | 5 | 5 |
| CI pipeline jobs | 8 | 8 |
| Binary size (health) | ~300KB | <500KB |
| Crates | 32 | 32 |
| Per-shim READMEs | 27 | 27 |

## Feature Inventory

| Feature | Status | Tests Added |
|---------|--------|-------------|
| OpenTelemetry tracing | Implemented | +6 |
| Structured JSON logging | Implemented | +5 |
| Config schema validation | Implemented | +20 |
| Migration lock files | Implemented | +8 |
| Backup verification | Implemented | +6 |
| Webhook health export | Implemented | +8 |
| Graceful degradation | Implemented | +11 |
| Resource quotas | Implemented | +17 |
| Multi-tenancy isolation | Implemented | +24 |
| Hot config reload + validation | Implemented | +8 |
| Redis event bridge | Implemented | +5 |
| WASM shim targets | Implemented | +7 |
| gRPC management API | Implemented | +5 |
| Chaos orchestration | Implemented | +13 |
| Cost optimization | Implemented | +12 |
| Performance regression CI | Implemented | +16 |
| K8s operator + sidecar | Implemented | 0 (YAML) |

## Risk Register

| Risk | Status | Mitigation |
|------|--------|------------|
| musl/aarch64 build | Mitigated | CI builds both architectures |
| Supply chain | Mitigated | rustsec audit + cosign signing + SBOM |
| Performance regression | Mitigated | Criterion benchmarks + CI gates |
| Scope creep | Mitigated | All roadmap items completed |
| Docker Hub availability | Known | Retry logic in CI, alternate registries |
| aarch64 aws-lc-rs cross-compile | Known | Limited to health-shim for aarch64-musl |

## Release Strategy

| Version | Scope | Status |
|---------|-------|--------|
| v0.7.0 | Audit, refactor, CI hardening | Completed |
| v1.0.0 | Production-ready, API stable | Completed |
| v1.1.0 | Post-launch improvements | Completed |
| v1.2.0 | Scaling features | Completed |
| v2.0.0 | Advanced features | Completed |
