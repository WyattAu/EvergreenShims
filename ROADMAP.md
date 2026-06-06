# Roadmap

Current state: v0.3.0 -- 32 crates, 757 tests, all CI/CD pipelines active.

## v0.3.0 Audit Complete (Current)

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

| Task | Priority | Description |
|------|----------|-------------|
| Migration-shim SQL injection remediation | Critical | Replace string formatting with parameterized queries for PostgreSQL path |
| Metrics/prometheus unwrap elimination | High | Convert remaining static-string unwraps to `unwrap_or_else` with descriptive messages |
| Hotreload timestamp metric fix | High | Replace IntCounter hack with proper Gauge for timestamp reporting |
| Redis bridge async I/O | High | Replace blocking Redis calls with async client in `redis_bridge.rs` |
| Shutdown strategy from_str cleanup | Medium | Implement `FromStr` trait properly instead of shadow method |
| Health-check exec: dead code branch | Medium | Consolidate duplicate `exec:` prefix handling in `execute_health_cmd()` |
| Config validate() side-effect removal | Medium | Remove `create_dir_all()` from validation method |
| subscribe_filtered() API fix | Medium | Either apply filters at subscription time or rename method |
| Missing module-level doc comments | Low | Add `//!` doc comments to otel.rs, structured_logging.rs |

## v0.5.0 Test Coverage Expansion

| Task | Priority | Description |
|------|----------|-------------|
| evergreen-shim binary integration tests | High | Add lifecycle tests for capability init/start/stop in unified binary |
| Signal handler unit tests | High | Test signal registration and broadcast in sandboxed environment |
| OpenTelemetry functional tests | Medium | Test actual span export, not just type assertions |
| Property-based testing | Medium | Add QuickCheck/Hypothesis tests for config parsing, PEM encode/decode |
| Concurrency stress tests | Medium | Multi-threaded access patterns for ShimBus, Config hot-reload |
| Benchmark regression threshold tuning | Low | Recalibrate baselines after code changes |

## v0.6.0 Production Readiness

| Task | Priority | Description |
|------|----------|-------------|
| `#[non_exhaustive]` on public Error enum | High | Prevent downstream exhaustive matching breakage |
| Circuit breaker integration test | High | Test actual TCP proxy with connection pool exhaustion |
| Graceful handler shutdown | High | Add stop() methods to wiring.rs handlers |
| Tenant rate-limit counter reset | Medium | Implement periodic counter reset mechanism |
| WASM build verification in CI | Medium | Add WASM target to CI matrix |
| Config schema versioning | Medium | Add version field to TOML config with migration support |
| Structured error types per shim | Low | Migrate from `anyhow` to typed errors where appropriate |

## v1.0.0 Production Release

| Task | Priority | Description |
|------|----------|-------------|
| API stability audit | Critical | Freeze public API surface per STABILITY.md |
| Performance regression baseline recalibration | High | Re-run benchmarks, update baseline.json with new code |
| Security audit | High | Third-party penetration test |
| SBOM automation in CI | Medium | Auto-generate SPDX SBOM on release |
| Container image signing verification | Medium | Add cosign verify step to deployment docs |
| Load testing | Medium | Sustained throughput testing under production conditions |
| Documentation site polish | Low | API reference generation, migration guides |

## v2.0.0 Scaling

| Task | Priority | Description |
|------|----------|-------------|
| Distributed tracing across shim instances | High | OpenTelemetry context propagation via Redis bridge |
| Multi-cluster failover | High | Cross-datacenter replication monitoring |
| WebAssembly shim runtime | Medium | Run shims as WASM modules for sandboxed execution |
| Plugin system | Medium | Dynamic capability loading at runtime |
| Observability dashboard | Medium | Pre-built Grafana dashboards for shim metrics |
| Operator maturity | Medium | Helm charts, CRD schema validation, admission webhooks |

## v3.0.0 Platform

| Task | Priority | Description |
|------|----------|-------------|
| Shim marketplace | High | Community-contributed shim registry |
| Multi-language shim SDK | High | Go, Python, Node.js SDKs for custom shims |
| Managed shim service | Medium | Hosted shim orchestration |
| AI-assisted operations | Medium | ML-based anomaly detection for shim metrics |
| Edge deployment | Low | ARM32, RISC-V targets for edge computing |

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
| Migration-shim SQL injection | Critical | Parameterized queries in v0.4.0 |
| Blocking I/O in Redis bridge | High | Async client migration in v0.4.0 |
| Config validation side effects | Medium | Pure validation in v0.4.0 |
| aarch64 aws-lc-rs cross-compile | Known | Limited to health-shim for aarch64-musl |
| Docker Hub availability | Known | Retry logic in CI, alternate registries |

## Quality Metrics

| Metric | Current | Target (v1.0.0) |
|--------|---------|-----------------|
| Unit tests | 757 | >800 |
| Clippy warnings | 0 | 0 |
| Unsafe code | 0 | 0 |
| Pre-commit checks | 6 | 6 |
| CI pipeline jobs | 8 | 8 |
| Binary size (health) | ~300KB | <500KB |
| Crates | 32 | 32 |
| Per-shim READMEs | 27 | 27 |
| Documentation coverage | 85% | >95% |
