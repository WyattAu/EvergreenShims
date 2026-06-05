# Roadmap

Current state: v0.3.0 -- 22 shims implemented, 491 tests passing, CI/CD hardened, documentation complete.

## Current Status

| Component | Status |
|-----------|--------|
| shim-core | Complete -- Capability trait, ShimBus, events, metrics, shutdown, process, hotreload |
| health-shim | Complete -- TCP/HTTP/exec probes, Prometheus metrics, PID 1 process management |
| vault-shim | Complete -- Vault/KMS secrets rotation, dynamic credentials |
| backup-shim | Complete -- pg_dump, mysqldump, BGSAVE, mongodump, compression, retention, S3 |
| migration-shim | Complete -- SQL file-based, version tracking, multi-DB, rollback |
| audit-shim | Complete -- Query logging, JSON/CEF, file/webhook output |
| proxy-shim | Complete -- Connection pooling, circuit breaker, weighted routing, retries |
| tls-shim | Complete -- Let's Encrypt ACME, internal CA, auto-renewal |
| config-shim | Complete -- File hash monitoring, backup, SIGHUP reload |
| failover-shim | Complete -- Patroni, Redis Sentinel, TCP health, state machine |
| replication-shim | Complete -- WAL tracking, lag monitoring, state management |
| cache-shim | Complete -- TTL, LRU/FIFO eviction, prefix invalidation |
| cdc-shim | Complete -- WAL position tracking, Kafka/webhook output |
| sharding-shim | Complete -- Hash-based and range-based routing |
| archival-shim | Complete -- Lifecycle tiers (hot/warm/cold), compression, purge |
| auth-shim | Complete -- Token auth, API keys, RBAC |
| encryption-shim | Complete -- AES-GCM, ChaCha20, key rotation |
| compliance-shim | Complete -- CIS/STIG scoring, violation tracking |
| scheduler-shim | Complete -- Cron scheduling, exponential backoff |
| queue-shim | Complete -- Job enqueue/dequeue, DLQ, worker pool |
| alerting-shim | Complete -- Severity routing, dedup, webhook dispatch |
| chaos-shim | Complete -- Latency/error injection, blast radius control |
| cost-shim | Complete -- Per-tenant resource tracking, budget alerts |
| evergreen-shim | Complete -- Unified binary, 22 feature flags, 6 presets |
| integration-tests | Complete -- 68 cross-shim tests, Docker Compose infrastructure |

## v0.4.0 -- Hardening & Production Readiness (Weeks 1-4)

### Goals

- Production-grade reliability
- Comprehensive error recovery
- Performance baselines

### Tasks

| Task | Priority | Effort |
|------|----------|--------|
| Integration test coverage for all DB connectors | P0 | 2 weeks |
| Chaos test suite (network partition, disk full, memory pressure) | P0 | 1 week |
| Performance benchmarks (latency, throughput, memory) | P1 | 1 week |
| Structured logging standardization across all shims | P1 | 3 days |
| Graceful degradation when dependencies unavailable | P1 | 1 week |
| Health probe timeout tuning per database type | P2 | 2 days |
| Backup retry with exponential backoff | P2 | 2 days |
| Migration dry-run mode | P2 | 3 days |

### Deliverables

- `chaos-shim` integration with real fault injection
- Benchmark suite with regression detection
- Production readiness checklist

## v0.5.0 -- Database Coverage Expansion (Weeks 5-8)

### Goals

- Support for additional database systems
- Enhanced replication and failover

### Tasks

| Task | Priority | Effort |
|------|----------|--------|
| MongoDB backup/sharding integration | P0 | 2 weeks |
| Cassandra health checks and failover | P1 | 1 week |
| Elasticsearch snapshot management | P1 | 1 week |
| CockroachDB/Multi-region replication awareness | P2 | 2 weeks |
| DynamoDB backup integration | P2 | 1 week |
| Multi-database migration orchestration | P2 | 2 weeks |

### Deliverables

- MongoDB, Cassandra, Elasticsearch shim coverage
- Cross-database replication topology awareness

## v1.0.0 -- Production Release (Weeks 9-12)

### Goals

- Stable API surface
- Certification readiness
- Community release

### Tasks

| Task | Priority | Effort |
|------|----------|--------|
| API stability review and semver commitment | P0 | 1 week |
| SBOM generation and supply chain attestation | P0 | 3 days |
| License compliance audit (all dependencies) | P0 | 2 days |
| Docker content trust signing | P1 | 2 days |
| Performance regression CI integration | P1 | 3 days |
| Migration guide from v0.x to v1.0 | P1 | 3 days |
| ADR (Architecture Decision Records) for key decisions | P2 | 1 week |
| OpenSSF Scorecard assessment | P2 | 2 days |

### Deliverables

- v1.0.0 release with signed binaries and images
- SBOM and provenance attestations
- API stability guarantee

## v1.x -- Post-Release (Ongoing)

### v1.1

- Kubernetes operator for automated shim deployment
- Helm chart for shim configuration
- Service mesh integration (Istio, Linkerd sidecar compatibility)

### v1.2

- WebAssembly (WASM) target for browser-based shims
- Embedded shim for IoT/edge devices
- Custom shim SDK (Rust template + build tooling)

### v1.3

- Multi-cluster replication with conflict resolution
- Automated failback with data consistency verification
- Cost optimization recommendations based on usage patterns

## v2.0 -- Advanced Features (Future)

| Feature | Description |
|---------|-------------|
| ML-based anomaly detection | Predict failover needs from metrics patterns |
| Cross-cloud portability | Unified shim for AWS/GCP/Azure databases |
| Policy engine | Open Policy Agent (OPA) integration for compliance enforcement |
| Observability platform | Built-in metrics aggregation and anomaly dashboard |
| Plugin system | Load custom shims at runtime via dynamic linking |

## Success Metrics

| Metric | v0.4.0 Target | v1.0.0 Target |
|--------|---------------|---------------|
| Test coverage (critical paths) | >95% | >98% |
| Test coverage (overall) | >85% | >90% |
| Memory overhead (idle) | <8MB | <5MB |
| Memory overhead (peak) | <15MB | <10MB |
| CPU overhead | <1% | <0.5% |
| Health check latency | <1ms | <0.5ms |
| Startup time | <100ms | <50ms |
| Binary size (full) | <3MB | <2.5MB |
| Binary size (health) | <300KB | <250KB |
| DB systems supported | 6 | 10+ |
| Chaos fault types | 5 | 10+ |

## Risk Register

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| musl/aarch64 build failures | Medium | Medium | Maintain fallback to glibc, track aws-lc-rs upstream |
| Database API breaking changes | High | Low | Pin dependency versions, integration test per version |
| Supply chain vulnerability | High | Medium | Dependabot, cargo-audit in CI, SBOM attestation |
| Performance regression | Medium | Medium | Benchmark CI, regression alerts, flamegraph profiling |
| Scope creep to v1.0 | High | High | Strict feature freeze at v1.0, defer to v1.x |
