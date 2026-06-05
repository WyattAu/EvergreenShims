# Roadmap

Current state: v0.3.0 -- 22 shims implemented, 491 tests passing, CI/CD hardened, documentation complete.

Target audience: Platform teams and enterprise deployments running production database infrastructure.

---

## Honest Status Assessment

### Production-Viable (12 shims)

| Shim | Status | Limitation |
|------|--------|------------|
| health-shim | Real | -- |
| vault-shim | Real | `danger_accept_invalid_certs(true)` must be fixed |
| backup-shim | Real | S3 upload not implemented, local-only |
| migration-shim | Real | PostgreSQL uses `psql` CLI (brittle) |
| audit-shim | Real | -- |
| cache-shim | Real | Single-node by design |
| sharding-shim | Real | Pure routing logic, no DB integration |
| auth-shim | Real | In-process only, no external IdP |
| encryption-shim | Real | -- |
| config-shim | Real | -- |
| scheduler-shim | Real | -- |
| queue-shim | Real | Single-node by design |

### Partially Real (7 shims)

| Shim | What Works | What's Missing |
|------|-----------|----------------|
| failover-shim | Detection (TCP, Patroni, Sentinel) | Promotion is in-memory string update, not real |
| replication-shim | WAL monitoring via psql | Promote is in-memory only |
| tls-shim | Vault PKI, self-signed certs | Let's Encrypt ACME is a stub |
| archival-shim | Local file copy, lifecycle tiers | S3/Glacier not implemented |
| alerting-shim | Severity routing, dedup | Webhook delivery works, no real PagerDuty/Slack integration |
| chaos-shim | Latency/error injection | Only in-process, not against real infrastructure |
| cost-shim | Budget tracking | In-memory only |

### Scaffolding (3 shims)

| Shim | Reality |
|------|---------|
| proxy-shim | Circuit breaker algorithms exist, but no actual TCP proxy or connection pooling |
| compliance-shim | Framework exists, zero actual CIS/STIG rules |
| cdc-shim | In-memory events, no actual WAL/binlog reading, Kafka is a no-op |

### Integration Test Reality

Of 68 "integration" tests, approximately **25-30 test real behavior**. The rest are API surface tests that verify env var parsing and struct field values. The `docker_integration.rs` tests only check TCP connectivity and skip if Docker is unavailable.

---

## v0.4.0 -- Foundation Hardening (Weeks 1-4)

### Goal

Make the 12 production-viable shims genuinely enterprise-grade. Fix every security issue, implement S3 upload, write real integration tests.

### P0 -- Security and Core Gaps

| Task | Effort | Details |
|------|--------|---------|
| Fix vault-shim TLS | 1 day | Remove `danger_accept_invalid_certs(true)`. Make TLS verification default-on, with `VAULT_TLS_SKIP_VERIFY=true` as explicit opt-out. |
| Implement S3 upload in backup-shim | 1 week | `aws-sdk-s3` is already in Cargo.toml but never imported. Implement `put_object` with multipart upload for large backups, SSE-S3 encryption, configurable endpoint (MinIO compatibility). |
| Implement S3/Glacier in archival-shim | 3 days | Same S3 client, add lifecycle transition to Glacier/Deep Archive. |
| PostgreSQL migration via sqlx | 3 days | Replace `psql` CLI shelling with `sqlx::PostgresPool` for direct connection. The MySQL path already uses sqlx correctly. |
| Real integration tests against Docker services | 2 weeks | Tests must actually: run migrations against PostgreSQL, upload backups to MinIO, rotate secrets via Vault, execute failover against MariaDB. Current 68 tests are mostly API surface. |

### P1 -- Enterprise Hardening

| Task | Effort | Details |
|------|--------|---------|
| Structured logging standardization | 3 days | Every shim uses consistent `tracing` field names: `service`, `operation`, `duration_ms`, `status`, `error`. No freeform strings. |
| Graceful degradation | 1 week | When Vault is unreachable, backup-shim retries with exponential backoff instead of failing silently. When PostgreSQL is down, migration-shim queues pending migrations. |
| Prometheus metrics completeness | 3 days | Every shim exposes `_total` counters, `_duration_seconds` histograms, and `_info` gauges. Currently inconsistent. |
| Backup retry with exponential backoff | 2 days | Configurable retry count, base delay, max delay. Jitter to prevent thundering herd. |
| Migration dry-run mode | 3 days | `--dry-run` flag that parses SQL, validates syntax, reports what would execute, without touching the database. |

### Deliverables

- S3 upload working with AWS S3 and MinIO
- Vault integration with proper TLS
- PostgreSQL migrations via sqlx
- 50+ real integration tests against Docker services
- Structured logging across all shims
- Performance baseline benchmarks

---

## v0.5.0 -- Complete the Scaffolding (Weeks 5-10)

### Goal

Transform the 3 scaffolding shims into real implementations. This is the largest engineering effort -- each requires genuine systems programming.

### proxy-shim -- Real TCP Proxy (3 weeks)

This is the hardest shim to build correctly. A production TCP proxy requires:

| Component | Implementation |
|-----------|---------------|
| TCP listener | `tokio::net::TcpListener` binding to configurable address |
| Connection pooling | Per-backend pool with configurable min/max/idle timeout |
| Load balancing | Weighted round-robin, least-connections, random |
| Circuit breaker | Half-open state machine with configurable thresholds |
| Health checking | Background TCP/connect checks against backends |
| TLS termination | `rustls` server for frontend, `rustls` client for backends |
| Request/response buffering | Configurable buffer sizes, streaming vs buffered |
| Rate limiting | Token bucket per-client and global |
| Retry with backoff | Configurable retry count, delay, jitter |
| Connection draining | Graceful shutdown with in-flight request completion |

The existing circuit breaker and rate limiter algorithms are real -- they just need a real TCP layer underneath.

### compliance-shim -- Real CIS/STIG Rules (2 weeks)

| Rule Category | Implementation |
|---------------|---------------|
| PostgreSQL CIS | Query `pg_settings`, `pg_hba.conf` parsing, role/member checks |
| MariaDB/MySQL CIS | Query `SHOW VARIABLES`, `SHOW GRANTS`, password policy |
| Redis STIG | Query `CONFIG GET`, ACL checks, TLS configuration |
| File system checks | Permission verification on data dirs, config files |
| Network checks | Verify bind addresses, port exposure |
| Scoring engine | Weighted scoring per rule, pass/fail/warning/severity |

Rules should be data-driven (TOML/YAML) so the community can contribute without code changes.

### cdc-shim -- Real Change Data Capture (3 weeks)

| Database | CDC Method |
|----------|-----------|
| PostgreSQL | Logical replication (`pg_output` plugin), WAL polling via `pg_replication_slots` |
| MariaDB/MySQL | Binlog reading via `mysql_async` or `mysql-binlog` crate |
| Redis | Keyspace notifications (`notify-keyspace-events`) |
| Output | Kafka via `rdkafka`, webhooks via `reqwest`, NATS via `async-nats` |

This is the most complex shim because CDC requires persistent connections and real-time streaming. The existing in-memory event model becomes the internal pipeline, but the input/output must be real.

### Additional v0.5.0 Tasks

| Task | Effort | Details |
|------|--------|---------|
| Patroni failover promotion | 3 days | Execute `patronictl switchover` or direct `pg_ctl promote` |
| Redis Sentinel failover | 2 days | `SENTINEL failover <master-name>` |
| Let's Encrypt ACME completion | 1 week | HTTP-01 challenge, certificate renewal, rate limit handling |
| chaos-shim real fault injection | 1 week | Network partition via `iptables` (requires root), process kill via `SIGKILL`, disk fill via `fallocate` |

### Deliverables

- Real TCP proxy with connection pooling, TLS, circuit breaker
- CIS/STIG rules for PostgreSQL, MariaDB, Redis (data-driven)
- Real CDC for PostgreSQL (logical replication), MariaDB (binlog), Redis (keyspace)
- Real failover promotion for Patroni and Redis Sentinel
- Complete Let's Encrypt ACME flow
- Real chaos tests against Docker services

---

## v0.6.0 -- Database Expansion (Weeks 11-16)

### Goal

Extend shim coverage to databases commonly found in enterprise environments.

### MongoDB (3 weeks)

| Capability | Implementation |
|-----------|---------------|
| Health checks | `serverStatus` command, replica set health |
| Backup | `mongodump` via command, oplog archiving |
| Migration | JSON schema validation, index management |
| Failover | Replica set awareness, primary detection |
| CDC | Change streams (`$changeStream`) |
| Sharding | Shard key analysis, balancer status |

### CockroachDB (2 weeks)

| Capability | Implementation |
|-----------|---------------|
| Health checks | `crdb_internal.node_infos`, range distribution |
| Migration | PostgreSQL-compatible SQL (reuse migration-shim) |
| Failover | Range-based leaseholder awareness |
| Sharding | Automatic (CockroachDB handles this), topology monitoring |
| CDC | Changefeeds (CockroachDB built-in) |

### DynamoDB (2 weeks)

| Capability | Implementation |
|-----------|---------------|
| Health checks | `DescribeTable`, provisioned vs on-demand capacity |
| Backup | Point-in-time recovery, export to S3 |
| Migration | DynamoDB JSON schema, GSI management |
| CDC | DynamoDB Streams |
| Cost | Per-table read/write capacity tracking |

### Additional v0.6.0 Tasks

| Task | Effort | Details |
|------|--------|---------|
| Elasticsearch snapshots | 1 week | `_snapshot` API, repository management |
| Cassandra health | 1 week | `nodetool status`, gossiper info |
| Multi-DB migration orchestration | 2 weeks | Coordinate migrations across database clusters with locking |

### Deliverables

- MongoDB, CockroachDB, DynamoDB shim coverage
- Elasticsearch snapshot management
- Cassandra health monitoring
- Cross-database migration orchestration

---

## v1.0.0 -- Enterprise Release (Weeks 17-20)

### Goal

Lock the API, harden the supply chain, produce enterprise-grade release artifacts.

### API Stability

| Task | Effort | Details |
|------|--------|---------|
| Lock `Capability` trait | 1 week | No breaking changes after v1.0. Any field/method removal requires major version bump |
| Lock `ShimBus` event types | 2 days | Freeze `EventType` enum variants. New events = new variants (non-breaking) |
| Lock configuration schema | 3 days | Document every env var, every TOML field. Deprecation policy: 2 minor versions |
| Stability badge | 1 day | Add stability indicators to public API documentation |

### Supply Chain Security

| Task | Effort | Details |
|------|--------|---------|
| SBOM generation | 2 days | `cargo-sbom` for SPDX format, include in every release |
| Binary signing | 2 days | `cosign` for container images, GPG for binary downloads |
| Provenance attestation | 2 days | SLSA Level 2 provenance via GitHub Actions |
| License compliance | 1 day | `cargo-deny` for license audit, block copyleft in dependencies |
| Dependabot + cargo-audit | 1 day | Already in CI, verify alerting works |

### Performance

| Task | Effort | Details |
|------|--------|---------|
| Criterion benchmark suite | 1 week | Per-shim benchmarks: health check latency, backup throughput, cache hit rate, proxy connections/sec |
| Regression CI gate | 2 days | Fail CI if benchmark regresses >5% from baseline |
| Memory profiling | 2 days | `dhat` or `jemalloc` profiling, document peak/idle memory per feature set |
| Binary size optimization | 3 days | LTO, `opt-level=z`, `strip` -- target <2.5MB full, <250KB health |

### Documentation

| Task | Effort | Details |
|------|--------|---------|
| Per-shim README | 1 week | Architecture, config reference, metrics reference, example Dockerfile |
| Operations guide | 3 days | Sizing, tuning, troubleshooting per database type |
| Migration guide from v0.x | 2 days | Breaking changes, upgrade path |
| Architecture Decision Records | 3 days | Document key design decisions with rationale |

### Deliverables

- v1.0.0 release with signed binaries and container images
- SBOM and SLSA provenance attestation
- API stability guarantee
- Complete benchmark suite with regression gates
- Enterprise-grade documentation

---

## v1.1.0 -- Community & Observability (Weeks 21-24)

| Feature | Description |
|---------|-------------|
| Alerting integrations | Real PagerDuty Events API v2, real Slack Block Kit, real email via SMTP |
| Dashboard shim | Optional web UI showing all shim states, metrics, recent events |
| Config validation CLI | `shim validate` command that checks TOML/env config without starting |
| Example repository | One Dockerfile per database type with EvergreenShims pre-configured |
| Community governance | CODEOWNERS, issue templates, PR templates, release cadence policy |

---

## v1.2.0 -- Advanced Operations (Weeks 25-28)

| Feature | Description |
|---------|-------------|
| Multi-cluster replication | Cross-datacenter WAL shipping with conflict detection |
| Automated failback | After primary recovers, verify data consistency, then switchover back |
| Cost optimization | Analyze usage patterns, recommend right-sizing, detect idle resources |
| Compliance dashboards | Web UI showing compliance scores, violation history, trend over time |
| Chaos engineering platform | Scheduled chaos experiments with automatic rollback on error budget |

---

## Success Metrics

| Metric | v0.4.0 | v0.5.0 | v0.6.0 | v1.0.0 |
|--------|--------|--------|--------|--------|
| Production-viable shims | 12 | 19 | 22 | 22 |
| Real integration tests | 50+ | 100+ | 150+ | 200+ |
| DB systems with real support | 5 | 5 | 8 | 8 |
| Test coverage (critical paths) | >95% | >97% | >97% | >98% |
| Test coverage (overall) | >85% | >88% | >90% | >92% |
| Memory overhead (idle) | <8MB | <8MB | <8MB | <5MB |
| Memory overhead (peak) | <15MB | <15MB | <15MB | <10MB |
| CPU overhead | <1% | <1% | <1% | <0.5% |
| Health check latency | <1ms | <1ms | <1ms | <0.5ms |
| Binary size (full) | <3MB | <4MB | <5MB | <5MB |
| Binary size (health) | <300KB | <300KB | <300KB | <250KB |
| CIS/STIG rules | 0 | 30+ | 50+ | 80+ |

---

## Risk Register

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| proxy-shim complexity exceeds estimate | High | Medium | Start with TCP passthrough, add TLS/pooling incrementally. Ship without TLS in v0.5.0 if needed. |
| CDC-shim database protocol changes | High | Low | Pin protocol versions, integration test per DB version, monitor upstream changelogs |
| S3 API compatibility (MinIO vs AWS) | Medium | Medium | Test against both MinIO and real S3 in CI |
| CIS rule maintenance burden | Medium | Medium | Data-driven rules (TOML), community contributions, automated rule testing |
| Performance regression during feature additions | High | Medium | Benchmark CI gate at every release, flamegraph profiling |
| Supply chain vulnerability in deep dependencies | High | Medium | `cargo-audit` in CI, Dependabot alerts, SBOM for traceability |
| PostgreSQL `psql` CLI removal | Medium | Low | Migrate to sqlx in v0.4.0 before it becomes critical |
| ACME rate limiting in testing | Low | Medium | Use Let's Encrypt staging environment for CI, production for releases |

---

## Design Decisions

### Why Rust Shim Only (No K8s Operator)

The shim-as-PID-1 pattern works in any container runtime (Docker, Podman, containerd, CRI-O). An operator adds:
- A second language (Go/Python) to maintain
- Kubernetes-specific coupling that excludes non-K8s environments
- Operational complexity for a project targeting simplicity

The shim binary is self-contained. Configuration is 12-factor (env vars + TOML). No control plane needed.

### Why Open Source Only

- Community contributions to CIS rules, CDC implementations, and database support scale better than a small team
- Enterprise adoption starts with FOSS evaluation
- The shim pattern is a commodity -- value comes from implementation quality, not proprietary features
- Apache-2.0 enables commercial integration without licensing friction

### Why Complete Scaffolding Before Expanding

Shipping v1.0 with 3 shimmy shims undermines credibility with platform teams and enterprise users. Better to have 19 real shims than 22 where 3 are fake. The proxy-shim and cdc-shim are high-value for enterprise -- they justify the effort.
