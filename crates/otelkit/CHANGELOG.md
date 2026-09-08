# Changelog

All notable changes to this project are documented here. Format: [Keep a
Changelog](https://keepachangelog.com/) — versions follow [semver](https://semver.org).

## [1.0.0] - 2026-09-05

First stable release. The public API is now covered by the project's
semver guarantees: breaking changes require a major version bump.

### Changed

- MSRV is now 1.88 (was 1.85) so the `time` dependency can be bumped to
  ≥ 0.3.47, fixing RUSTSEC-2026-0009 (DoS via stack exhaustion, transitive
  through `sentry-types`).

## [0.1.0] - 2026-09-01

### Added

- OpenTelemetry trace context propagation and span export.
- OTLP export to any OTLP-compatible collector (Jaeger, Grafana Tempo,
  etc.) behind the `otlp` feature.
- Sentry error tracking with release association and sample-rate control
  behind the `sentry` feature.
- Structured logging via `tracing-subscriber`: env-filter, JSON or text
  output (`json` feature, on by default), and an RAII guard that flushes
  on shutdown.
