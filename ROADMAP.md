# Roadmap

Current state: v1.0.0 -- 34 crates, 885 tests, all CI/CD pipelines active, GitHub Pages deployed.

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
| Multi-language shim SDK (Go, Python, Node) | Completed (skeleton) | High | Ecosystem |
| Edge deployment targets (ARM, RISC-V) | Completed | Medium | Deployment |
| Plugin system for custom shims | Completed | High | Extensibility |
| WebAssembly shim runtime | Completed | Medium | Portability |
| CLI management tool (shimctl) | Completed | Medium | Operations |
| Terraform provider (scaffolding) | Completed | Medium | IaC integration |

### v3.x Known Gaps (Post-Release Work)

| Gap | Severity | Description |
|-----|----------|-------------|
| SDK HTTP client implementation | High | Go/Python/Node SDKs are importable shells with no real HTTP logic |
| Terraform provider compilation | High | Go code is syntactically valid but never compiled or tested |
| Helm chart (was lost) | Fixed | Recreated in Phase A (helm/evergreen-shims/) |
| Grafana dashboard (was lost) | Fixed | Recreated in Phase A (grafana/evergreen-shims-dashboard.json) |
| k8s sample configmap | Fixed | Created in Phase A (k8s/sample-configmap.yaml) |
| Example plugin build in CI | Medium | Requires C compiler for cdylib crate-type |
| Coverage measurement | Medium | No cargo-tarpaulin or cargo-llvm-cov in CI |
| Auth test flakiness | Medium | temp_env parallel pollution |

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

## Risk Register

| Risk | Impact | Mitigation |
|------|--------|------------|
| aarch64/riscv64 cross-compile failures | Known | Limited to health-shim for non-x86 |
| Docker Hub availability | Known | Retry logic in CI, GHCR as primary |
| Cargo-deny false positives | Low | Allowlist tuning in deny.toml |
| Pre-push hook latency (~84s) | Low | Acceptable for safety guarantee |
| SDK/Provider not tested in target language | Known | Requires Go/Node/Python toolchains in CI |

## Quality Metrics

| Metric | Current | Target (v1.0.0) |
|--------|---------|-----------------|
| Unit tests | 885 | >900 |
| Clippy warnings | 0 | 0 |
| Unsafe code | 0 | 0 |
| Pre-commit checks | 8 | 8 |
| CI pipeline jobs | 14 | 14 |
| Binary size (health) | ~2.5MB | <3MB |
| Crates | 34 | 34 |
| Documentation coverage | 90% | >95% |
| GitHub Pages | Active | Active |
| Supply chain (SHA pins) | Active | Active |
| Dependency audit (cargo-deny) | Active | Active |
| Helm chart | Active | Active |
| Grafana dashboard | Active | Active |

## Post-1.0 Iteration Plan

### v1.1.0 -- SDK & Provider Hardening

| Task | Priority | Impact |
|------|----------|--------|
| Go SDK HTTP client with retry, auth, TLS | High | Ecosystem |
| Python SDK HTTP client with retry, auth, TLS | High | Ecosystem |
| Node SDK HTTP client with retry, auth, TLS | High | Ecosystem |
| Terraform provider: compile, test, lint | High | IaC |
| SDK CI: go test, pytest, npm test | High | Quality |
| SDK documentation with usage examples | Medium | DX |

### v1.2.0 -- Coverage & Observability

| Task | Priority | Impact |
|------|----------|--------|
| Coverage threshold enforcement in CI (>80%) | High | Quality |
| Grafana dashboard tested against live metrics | Medium | Ops |
| Helm chart lint (helm lint, ct lint) | Medium | Quality |
| Example plugin CI build (gcc toolchain) | Low | DX |
| Prometheus scrape endpoint validation | Medium | Observability |

### v2.0.0 -- Advanced Platform Features

| Task | Priority | Impact |
|------|----------|--------|
| WASM shim runtime hardening (fuzzing, OOM) | High | Portability |
| Plugin SDK v2 (capability negotiation) | High | Extensibility |
| Multi-cluster failover live testing | High | HA |
| Load testing proxy-shim circuit breaker | High | Performance |
| Chaos testing with real databases | High | Resilience |
