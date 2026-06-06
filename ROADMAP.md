# Roadmap

Current state: v0.6.0 -- 32 crates, 792 tests, all CI/CD pipelines active, GitHub Pages deployed.

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
| Structured error types per shim | Pending | Medium | API ergonomics |
| Per-crate README files | Pending | Medium | Discoverability |
| API reference docs (cargo doc) | Pending | High | Developer experience |
| Integration test coverage expansion | Pending | High | Reliability |
| Chaos test automation in CI | Pending | High | Resilience validation |
| Performance regression baseline v2 | Pending | Medium | Benchmark accuracy |
| Docker image vulnerability remediation | Pending | High | Security |
| aarch64 full/infra builds | Pending | Low | Platform coverage |
| SBOM generation in CI | Pending | Medium | Supply chain transparency |

## v2.0.0 Scaling

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Distributed tracing across shim instances | Pending | High | Observability |
| Multi-cluster failover | Pending | High | HA |
| Observability dashboard (Grafana) | Pending | Medium | Operations |
| Operator maturity (Helm/CRD) | Pending | High | Kubernetes |
| Config hot-reload via Kubernetes ConfigMap | Pending | Medium | K8s integration |
| Graceful degradation with circuit breakers | Pending | High | Fault tolerance |
| Multi-tenant resource isolation hardening | Pending | High | Security |
| Webhook-based alerting with retry | Pending | Medium | Reliability |

## v3.0.0 Platform

| Task | Status | Priority | Impact |
|------|--------|----------|--------|
| Multi-language shim SDK (Go, Python, Node) | Pending | High | Ecosystem |
| Edge deployment targets (ARM, RISC-V) | Pending | Medium | Deployment |
| Plugin system for custom shims | Pending | High | Extensibility |
| WebAssembly shim runtime | Pending | Medium | Portability |
| CLI management tool | Pending | Medium | Operations |
| Terraform/Pulumi provider | Pending | Medium | IaC integration |

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
| aarch64 aws-lc-rs cross-compile | Known | Limited to health-shim for aarch64-musl |
| Docker Hub availability | Known | Retry logic in CI, GHCR as primary |
| Cargo-deny false positives | Low | Allowlist tuning in deny.toml |
| Pre-push hook latency (~84s) | Low | Acceptable for safety guarantee |

## Quality Metrics

| Metric | Current | Target (v1.0.0) |
|--------|---------|-----------------|
| Unit tests | 792 | >900 |
| Clippy warnings | 0 | 0 |
| Unsafe code | 0 | 0 |
| Pre-commit checks | 8 | 8 |
| CI pipeline jobs | 10 | 10 |
| Binary size (health) | ~2.5MB | <3MB |
| Crates | 32 | 32 |
| Documentation coverage | 90% | >95% |
| GitHub Pages | Active | Active |
| Supply chain (SHA pins) | Active | Active |
| Dependency audit (cargo-deny) | Active | Active |
