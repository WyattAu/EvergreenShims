# Roadmap

Current state: v0.6.0 -- 32 crates, 834+ tests, all CI/CD pipelines active.

## v0.3.0 Audit Complete

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

## v0.4.0 Hardening

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

## v0.5.0 Test Coverage Expansion

| Task | Status | Impact |
|------|--------|--------|
| evergreen-shim binary integration tests | Completed | Test coverage (+19 tests) |
| Signal handler unit tests | Completed | Test coverage |
| OpenTelemetry functional tests | Completed | Test coverage |
| Property-based testing | Completed | Config/PEM roundtrip tests |
| Concurrency stress tests | Completed | ShimBus/Config concurrency |
| Benchmark regression threshold tuning | Completed | CI accuracy |

## v0.6.0 Production Readiness

| Task | Status | Impact |
|------|--------|--------|
| `#[non_exhaustive]` on public Error enum | Completed | API stability |
| Circuit breaker integration test | Completed | Proxy reliability |
| Graceful handler shutdown | Completed | Resource cleanup |
| Tenant rate-limit counter reset | Completed | Rate limiting correctness |
| WASM build verification in CI | Completed | Cross-platform |
| Config schema versioning | Completed | Migration support |
| Structured error types per shim | Deferred | Low priority |

## v1.0.0 Production Release

| Task | Status | Impact |
|------|--------|--------|
| Performance regression baseline recalibration | Pending | Benchmark accuracy |
| SBOM automation in CI | Pending | Supply chain transparency |
| Documentation site polish | Pending | Developer experience |

## v2.0.0 Scaling

| Task | Status | Impact |
|------|--------|--------|
| Distributed tracing across shim instances | Pending | Observability |
| Multi-cluster failover | Pending | HA |
| Observability dashboard | Pending | Operations |
| Operator maturity (Helm/CRD) | Pending | Kubernetes |

## v3.0.0 Platform

| Task | Status | Impact |
|------|--------|--------|
| Multi-language shim SDK | Pending | Ecosystem |
| Edge deployment targets | Pending | Deployment |

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
| Docker Hub availability | Known | Retry logic in CI, alternate registries |

## Quality Metrics

| Metric | Current | Target (v1.0.0) |
|--------|---------|-----------------|
| Unit tests | 834+ | >900 |
| Clippy warnings | 0 | 0 |
| Unsafe code | 0 | 0 |
| Pre-commit checks | 6 | 6 |
| CI pipeline jobs | 9 | 9 |
| Binary size (health) | ~2.5MB | <3MB |
| Crates | 32 | 32 |
| Per-shim READMEs | 27 | 27 |
| Documentation coverage | 90% | >95% |
