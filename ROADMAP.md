# Roadmap

Current state: v3.2.0 -- 35 crates, 1200+ tests, deployed to test server (192.168.1.191), all CI/CD pipelines active, GitHub Pages deployed.

## Completed Milestones

### v0.3.0 Audit Complete

| Task | Status | Impact |
|------|--------|--------|
| Alerting-shim panic elimination | Completed | Production safety |
| Backup-shim mutex poison recovery | Completed | Fault tolerance |
| Encryption-shim Result-based construction | Completed | API ergonomics |
| Dead code suppression cleanup (16 crates) | Completed | Code hygiene |
| Trivy action pinning | Completed | Supply chain security |
| Pre-commit hook hardening | Completed | Developer ergonomics |
| Landing page redesign (brutalist/amoebic) | Completed | Brand identity |
| Documentation overhaul | Completed | Developer experience |

### v0.4.0 Hardening

| Task | Status | Impact |
|------|--------|--------|
| Migration-shim SQL injection remediation | Completed | Security |
| Metrics/prometheus unwrap elimination | Completed | Stability |
| Hotreload timestamp metric fix | Completed | Observability correctness |
| Redis bridge async I/O | Completed | Runtime stability |
| Shutdown strategy from_str cleanup | Completed | API correctness |
| Health-check exec dead code branch | Completed | Code clarity |
| Config validate() side-effect removal | Completed | Pure validation |
| subscribe_filtered() API fix | Completed | API correctness |
| Missing module-level doc comments | Completed | Documentation |

### v0.5.0 Test Coverage Expansion

| Task | Status | Impact |
|------|--------|--------|
| evergreen-shim binary integration tests | Completed | Test coverage (+19 tests) |
| Signal handler unit tests | Completed | Test coverage |
| OpenTelemetry functional tests | Completed | Test coverage |
| Property-based testing | Completed | Config/PEM roundtrip tests |
| Concurrency stress tests | Completed | ShimBus/Config concurrency |
| Benchmark regression threshold tuning | Completed | CI accuracy |

### v0.6.0 Production Readiness

| Task | Status | Impact |
|------|--------|--------|
| `#[non_exhaustive]` on public Error enum | Completed | API stability |
| Circuit breaker integration test | Completed | Proxy reliability |
| Graceful handler shutdown | Completed | Resource cleanup |
| Tenant rate-limit counter reset | Completed | Rate limiting correctness |
| WASM build verification in CI | Completed | Cross-platform |
| Config schema versioning | Completed | Migration support |
| Unused variable warning fix (health.rs) | Completed | Build hygiene |
| Benchmark baseline recalibration | Completed | Regression accuracy |
| Supply chain hardening (SHA-pinned actions) | Completed | CI/CD security |
| cargo-deny integration | Completed | Dependency auditing |
| commit-msg/pre-push hooks | Completed | Developer ergonomics |
| Landing page accessibility (WCAG) | Completed | Accessibility |
| Documentation metrics update | Completed | Accuracy |

## v1.0.0 Production Release

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Structured error types per shim | Completed | Medium | API ergonomics |
| Per-crate README files | Completed | Medium | Discoverability |
| API reference docs (cargo doc) | Completed | High | Developer experience |
| Integration test coverage expansion | Completed | High | Reliability |
| Chaos test automation in CI | Completed | High | Resilience validation |
| Performance regression baseline v2 | Completed | Medium | Benchmark accuracy |
| Docker image hardening (pinned base, OCI labels) | Completed | High | Security |
| aarch64/riscv64 full/infra builds | Completed | Low | Platform coverage |
| SBOM generation in CI | Completed | Medium | Supply chain transparency |

## v1.0.1 Quality Hardening (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Eliminate 47 unwrap() calls in production library code | Completed | High | Runtime safety |
| Fix critical tenant.rs HashMap unwrap (panic on missing tenant) | Completed | High | Fault tolerance |
| Add graceful error handling in metrics/healthz HTTP handlers | Completed | High | Availability |
| Remove crate-level #![allow(dead_code)] from integration-tests | Completed | Medium | Code hygiene |
| Add missing module-level doc comment on shimctl | Completed | Low | Documentation |
| Fix all 19 documentation issues (stale metrics, broken links, etc.) | Completed | Medium | Accuracy |
| Fix CI: add protobuf-compiler to 8 pipeline jobs | Completed | High | CI/CD reliability |
| Fix CI: replace broken rustsec/audit-check SHA | Completed | High | Security pipeline |
| Fix CI: cargo-sbom CLI flag migration | Completed | Medium | SBOM pipeline |
| Fix CI: add go mod tidy for terraform build | Completed | Medium | IaC pipeline |
| Update Dockerfile base image to rust:1-alpine | Completed | High | Docker builds |
| Fix migration-shim env var pollution in connection string tests | Completed | Medium | Test reliability |
| Apply cargo fmt across workspace | Completed | Low | Code style |

## v2.0.0 Scaling

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Distributed tracing across shim instances | Completed | High | Observability |
| Multi-cluster failover | Completed | High | HA |
| Observability dashboard (Grafana) | Completed | Medium | Operations |
| Operator maturity (Helm/CRD) | Completed | High | Kubernetes |
| Config hot-reload via Kubernetes ConfigMap | Completed | Medium | K8s integration |
| Graceful degradation with circuit breakers | Completed | High | Fault tolerance |
| Multi-tenant resource isolation hardening | Completed | High | Security |
| Webhook-based alerting with retry | Completed | Medium | Reliability |

## v3.0.0 Platform

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Multi-language shim SDK (Go, Python, Node) | Completed | High | Ecosystem |
| Edge deployment targets (ARM, RISC-V) | Completed | Medium | Deployment |
| Plugin system for custom shims | Completed | High | Extensibility |
| WebAssembly shim runtime | Completed | Medium | Portability |
| CLI management tool (shimctl) | Completed | Medium | Operations |
| Terraform provider | Completed | Medium | IaC integration |

### v3.x Known Gaps (Post-Release Work)

| Gap | Severity | Status | Description |
|-----|----------|--------|-------------|
| SDK HTTP client implementation | High | Fixed | Go/Python/Node SDKs now have working HTTP clients with tests |
| Terraform provider compilation | High | Fixed | Simplified to use management API, removed k8s dependency |
| Helm chart | Fixed | Fixed | Recreated (helm/evergreen-shims/) |
| Grafana dashboard | Fixed | Fixed | Recreated with correct metric names from codebase |
| k8s sample configmap | Fixed | Fixed | Created (k8s/sample-configmap.yaml) |
| Coverage measurement | Fixed | Fixed | 73% baseline established, CI job active |
| Auth test flakiness | Fixed | Fixed | serial_test added to prevent parallel pollution |
| CI protobuf-compiler missing | Fixed | Fixed | Added protoc install to 8 CI jobs |
| Docker base image too old | Fixed | Fixed | Updated from rust:1.85 to rust:1-alpine |
| Migration-shim env var pollution | Fixed | Fixed | Clear db_url in connection string tests |

## Architecture Decisions

| ADR | Decision | Status |
|-----|----------|--------|
| ADR-001 | PID 1 execution model | Accepted |
| ADR-002 | musl static linking for scratch containers | Accepted |
| ADR-003 | Capability trait as core abstraction | Accepted |
| ADR-004 | ShimBus in-process broadcast | Accepted |
| ADR-005 | Environment variables as primary config | Accepted |
| ADR-006 | Feature-gated binary composition | Accepted |
| ADR-007 | ring-free aws-lc-rs for musl compatibility | Accepted |
| ADR-008 | expect() with descriptive messages over unwrap() | Accepted |
| ADR-009 | #[cfg(test)] modules for integration test helpers | Accepted |

## Risk Register

| Risk | Impact | Mitigation |
|------|--------|------------|
| Docker image rust:1-alpine is non-deterministic | Low | Cargo.lock pins dependency versions |
| Pre-push hook latency (~294s full, ~30s unit) | Low | Acceptable for safety guarantee |
| SDK/Provider not tested in target language | Known | Requires Go/Node/Python toolchains in CI |
| Node.js 20 actions deprecation (June 2026) | Medium | Update to Node.js 24 compatible actions |

## Quality Metrics

| Metric | Current | Target |
|--------|---------|--------|
| Unit tests | 1148 | >900 |
| Code coverage (excl. integration) | 78% | >80% |
| Clippy warnings | 0 | 0 |
| Unsafe code | 0 (library) | 0 |
| Pre-commit checks | 6 | 6 |
| CI pipeline jobs | 15 | 15 |
| Binary size (health) | ~2.5MB | <3MB |
| Crates | 34 | 34 |
| Documentation coverage | 95% | >95% |
| GitHub Pages | Active | Active |
| Supply chain (SHA pins) | Active | Active |
| Dependency audit (cargo-deny) | Active | Active |
| Helm chart | Active | Active |
| Grafana dashboard | Active (validated) | Active |
| Go SDK tests | 9 passing | Active |
| Python SDK tests | 16 passing | Active |
| Node SDK tests | 15 passing | Active |

## Post-1.0 Iteration Plan

### v1.1.0 -- SDK & Provider Hardening (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Go SDK HTTP client with retry, auth, TLS | Completed | High | Ecosystem |
| Python SDK HTTP client with retry, auth, TLS | Completed | High | Ecosystem |
| Node SDK HTTP client with retry, auth, TLS | Completed | High | Ecosystem |
| Terraform provider: simplify to management API | Completed | High | IaC |
| SDK CI: go test, pytest, npm test | Completed | High | Quality |
| SDK documentation with usage examples | Completed | Medium | DX |
| Coverage baseline: 73% (14,801/20,155 lines) | Completed | High | Quality |
| Grafana dashboard: metrics validated against codebase | Completed | Medium | Ops |
| Pre-push hook: unit tests only (integration to CI) | Completed | High | DX |

### v1.2.0 -- Coverage & Observability (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Coverage threshold enforcement in CI (>70%) | Completed | High | Quality |
| Helm chart lint (helm lint, ct lint) | Completed | Medium | Quality |
| Failover-shim coverage improvement (51% -> ~55%) | Completed | High | Quality |
| Compliance-shim coverage improvement (36% -> ~45%) | Completed | Medium | Quality |
| Grafana dashboard metrics validated against codebase | Completed | Medium | Ops |

### v1.3.0 -- Pragmatic Coverage (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| shimctl: 11 response deserialization tests | Completed | High | 21% -> 25% |
| vault-shim: credentials serialization, write, config override | Completed | High | 50% -> 55% |
| cdc-shim: event/WAL/stats serialization, env config, case insensitivity | Completed | High | 51% -> 60% |
| evergreen-shim: CLI arg parsing, config merge | Completed | High | 52% -> 60% |
| failover-shim: TCP checks, serialization, env config | Completed | High | 51% -> 55% |
| compliance-shim: CIS rule generation, severity, violations | Completed | Medium | 36% -> 45% |

## Forward-Looking Roadmap

### v1.4.0 -- Coverage Push to 80% (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Failover-shim: 55% -> 70% (Patroni/Sentinel mock tests) | Completed | High | Coverage (+31 tests) |
| Compliance-shim: 45% -> 60% (STIG rule expansion) | Completed | Medium | Coverage (+37 tests) |
| shimctl: 25% -> 40% (command dispatch, error paths) | Completed | High | Coverage (+26 tests) |
| Backup-shim: S3 upload path tests with mock server | Completed | High | Coverage (+34 tests) |
| Integration test coverage: chaos-shim ignored tests in CI | Completed | Medium | Coverage (+27 chaos, +9 OTel, +5 integration) |

### v1.5.0 -- Security Hardening (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Fuzz testing with cargo-fuzz on parser-heavy shims | Completed | High | Security (6 fuzz targets) |
| SBOM attestation in release workflow (SLSA Level 3) | Completed | High | Supply chain |
| CVE remediation pipeline (Dependabot) | Completed | Medium | Security |
| TLS 1.3 enforcement audit across all network paths | Completed | High | Security |
| Encryption key rotation interval validation | Completed | Medium | Security |

### v1.6.0 -- Developer Experience (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Architecture Decision Records (ADRs) formalized | Completed | Medium | DX (6 ADRs) |
| Interactive playground (Docker Compose with all shims) | Completed | High | DX |
| CLI auto-completion (bash/zsh/fish) | Completed | Medium | DX |
| Configuration schema validation at startup | Completed | High | DX (+12 validation tests) |
| Hot-reload documentation with live examples | Completed | Low | DX |

### v2.0.0 -- Advanced Platform Features (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| WASM shim runtime hardening (fuzzing, OOM) | Completed | High | Portability (+16 tests) |
| Plugin SDK v2 (capability negotiation) | Completed | High | Extensibility (+20 tests) |
| Multi-cluster failover live testing | Completed | High | HA (+23 tests) |
| Load testing proxy-shim circuit breaker | Completed | High | Performance (+22 tests) |
| Chaos testing with real databases | Completed | High | Resilience (+27 tests) |
| OpenTelemetry SDK integration for shims | Completed | Medium | Observability (+9 tests) |

### v3.0.0 -- Production Maturity (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| **Phase A: E2E Integration Tests** | Completed | High | Reliability (+18 E2E tests) |
| health-shim TCP probe and lifecycle | Completed | High | Health verification |
| migration-shim PostgreSQL migration | Completed | High | Database lifecycle |
| backup-shim SHA-256 checksum | Completed | High | Data integrity |
| vault-shim secret read (real Vault) | Completed | High | Secrets lifecycle |
| cache-shim Redis set/get/delete | Completed | High | Cache lifecycle |
| encryption-shim AES-GCM roundtrip | Completed | High | Crypto verification |
| alerting-shim webhook delivery | Completed | Medium | Alerting verification |
| config-shim file change detection | Completed | Medium | Config lifecycle |
| queue-shim worker/retry/DLQ | Completed | Medium | Job queue lifecycle |
| scheduler-shim cron task lifecycle | Completed | Medium | Scheduling verification |
| cdc-shim event capture + WAL tracking | Completed | High | CDC verification |
| chaos-shim experiment lifecycle | Completed | Medium | Chaos verification |
| **Phase B: Management API Hardening** | Completed | High | Security (+7 tests) |
| Request validation (ports, metrics, strings) | Completed | High | Input safety |
| Rate limiting middleware (per-IP RPM) | Completed | High | DoS protection |
| Audit logging for sensitive operations | Completed | Medium | Compliance |
| Input sanitization (control chars, null bytes) | Completed | High | Security |
| **Phase C: Cloud Metadata Plugin** | Completed | Medium | DX (example plugin) |
| AWS EC2 metadata fetcher plugin | Completed | Medium | Extensibility |
| C ABI vtable, lifecycle, metrics | Completed | Medium | Plugin SDK validation |
| Plugin documentation (build/deploy/config) | Completed | Medium | Developer onboarding |
| **Phase D: Kubernetes Operator** | Completed | High | Platform (+20 tests) |
| Reconciliation loop for ShimConfig CRD | Completed | High | K8s automation |
| ConfigMap generation from CRD spec | Completed | High | Configuration |
| Deployment sidecar injection | Completed | High | Deployment |
| Status conditions and events | Completed | Medium | Observability |
| **Phase E: Production Hardening** | Completed | High | Production readiness |
| Structured logging with request IDs | Completed | Medium | Observability |
| Graceful shutdown with drain timeout | Completed | High | Reliability |
| Resource monitoring (CPU, memory, FDs) | Completed | Medium | Operations |
| Health check hardening (readiness/liveness/startup) | Completed | High | Kubernetes |

### v3.1.0 -- CI Stability & Validation (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| **Priority 1: CI Stability** | Completed | High | Reliability |
| Dockerfile pinned to rust:1.91-alpine3.21 | Completed | High | Reproducible builds |
| E2E CI job with Docker Compose services | Completed | High | Integration verification |
| E2E tests skip gracefully when services unavailable | Completed | Medium | CI stability |
| **Priority 2: Kubernetes E2E** | Completed | High | Platform validation |
| Kind cluster CI job (CRD apply, operator deploy) | Completed | High | K8s automation |
| Operator RBAC and deployment specs | Completed | Medium | Security |
| **Priority 3: Load Testing** | Completed | High | Performance validation |
| Management API load test (100 clients, 10K requests) | Completed | High | Throughput/latency |
| Threshold gates (throughput >1000, p99 <100ms) | Completed | Medium | Regression detection |
| **Priority 4: Crate Consolidation** | Completed | Medium | Maintenance reduction |
| Consolidation analysis (audit/compliance, archival/backup, queue/scheduler) | Completed | Medium | Architecture clarity |
| **Priority 5: Deployment Guide** | Completed | Medium | Developer onboarding |
| Zero-to-production walkthrough | Completed | Medium | Adoption |

### v3.2.0 -- Production Deployment Verification (Completed)

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Release binary build (health-shim, 2.4MB stripped) | Completed | High | Deployment artifact |
| Deploy to test server (192.168.1.191) | Completed | High | Production validation |
| Docker Compose services (Postgres, Redis, MariaDB, Vault) | Completed | High | Service infrastructure |
| Health-shim running with livez/readyz/metrics | Completed | High | Runtime verification |
| All Docker services healthy | Completed | High | Infrastructure validation |
| Live E2E tests: 9/9 passed | Completed | High | Production validation |
| CI fix: duplicate e2e job key resolution | Completed | High | Pipeline stability |
| Coverage push: +37 tests (management-api, shim-core) | Completed | High | Code quality |
