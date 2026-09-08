# Threat Model — otelkit

Status: **v1.0** · One-page STRIDE over the public API surface
(`TelemetryConfig`/`from_env`, `LogFormat`, `init`/`TelemetryGuard`).

Assets: (A1) telemetry pipeline integrity — logs/traces go where configured;
(A2) credential material passed as config (`sentry_dsn`, OTLP endpoint
auth); (A3) log confidentiality (no PII scrubbing happens here).

| # | Threat | Category | Surface | Mitigation | Verifying test |
|---|--------|----------|---------|------------|----------------|
| T1 | Hostile env values crash `init` | DoS | `TelemetryConfig::from_env` | Parse failures return `TelemetryError`; unknown log format falls back to JSON (`from_str_opt`) | `src/config.rs::from_str_opt` fallback mapping; `LogFormat` parsing |
| T2 | Telemetry silently disabled (init failure ignored) | Repudiation | `init` | `Result<TelemetryGuard, TelemetryError>`; guard drop restores previous subscriber | `src/lib.rs::init` guard semantics; error variants in `src/error.rs` |
| T3 | DSN/endpoint logged via `Debug` | Info disclosure | `TelemetryConfig` | `Debug` is derived and *does* render `sentry_dsn`/`otlp_endpoint` | none (see OPEN-1) |
| T4 | Sample-rate misconfiguration silences everything | Tampering | `sample_rate` | f32 in 0.0–1.0 documented; clamping behavior unverified | none (see OPEN-2) |

**OPEN RISKS**

- **OPEN-1 — `TelemetryConfig` derives `Debug` including `sentry_dsn`.**
  A DSN is a write credential; `{:?}` of the config leaks it. No redacted
  `Debug` impl, no test.
- **OPEN-2 — `sample_rate` is not validated/clamped.** Negative or > 1.0
  values pass through to the tracing layer; behavior depends on downstream
  sampler semantics. No test.
- **OPEN-3 — no endpoint URL validation on `otlp_endpoint`.** A malformed
  endpoint fails at exporter runtime, not at config time; a *hostile*
  endpoint (env-controlled) redirects telemetry — env is assumed trusted,
  but no parse-time guard exists.
- **OPEN-4 — no log redaction layer.** The crate transports whatever spans
  carry; PII scrubbing is the emitter's duty (cf. kestrel ADR 0008 rules,
  not enforced here).

**Out of scope:** exporter transport security (OTLP/TLS config belongs to
the exporter); backend retention policies; span content policy.

**Residual risk:** `init` is process-global — calling it twice (tests,
libraries) swaps subscribers; the guard-based restore mitigates but does
not prevent ordering surprises.
