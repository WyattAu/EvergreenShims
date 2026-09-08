# otelkit

Tracing and telemetry initialization for Rust — OpenTelemetry, OTLP export, Sentry integration, and structured logging.

## Purpose

`otelkit` replaces boilerplate tracing/telemetry setup with a single `init()` call. It wires together:

- **OpenTelemetry** — trace context propagation and span export.
- **OTLP export** — push traces to any OTLP-compatible collector (Jaeger, Grafana Tempo, etc.).
- **Sentry** — error tracking with release association and sample-rate control.
- **Structured logging** — `tracing-subscriber` with env-filter, JSON or text output, and an RAII guard that flushes on shutdown.

## Usage

```rust
use otelkit::{TelemetryConfig, LogFormat, init};

#[tokio::main]
async fn main() {
    let config = TelemetryConfig::from_env().expect("valid config");

    let _guard = init(config).expect("telemetry init failed");

    tracing::info!(message = "service started", version = "1.0.0");
    tracing::error!(message = "something went wrong", code = 42);
}
```

Or build config manually:

```rust
let config = TelemetryConfig::default()
    .service_name("my-service")
    .service_version("1.0.0")
    .log_level("debug")
    .log_format(LogFormat::Json)
    .otlp_endpoint("http://localhost:4317")
    .sentry_dsn("https://key@sentry.io/project")
    .sample_rate(0.5);
```

## Comparison with manual tracing-subscriber

| Feature | Manual setup | `otelkit` |
|---|---|---|
| Env filter | DIY | Built-in |
| JSON logs | `fmt::layer().json()` | `LogFormat::Json` |
| OTLP export | Wire `opentelemetry` manually | Feature flag `otlp` |
| Sentry | Manual `sentry::init` | Feature flag `sentry` |
| RAII shutdown | DIY drop guard | `TelemetryGuard` |
| Config from env | DIY | `from_env()` |

## Features

- `default` — `["std", "json"]`
- `std` — `tracing-subscriber/std`
- `json` — `tracing-subscriber/json`
- `otlp` — OpenTelemetry OTLP trace export
- `sentry` — Sentry error tracking integration

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `OTEL_SERVICE_NAME` | Service name | `"unknown"` |
| `OTEL_SERVICE_VERSION` | Service version | `"0.0.0"` |
| `OTEL_LOG_LEVEL` | Log level filter | `"info"` |
| `OTEL_LOG_FORMAT` | `"text"` or `"json"` | `"json"` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP endpoint | — |
| `SENTRY_DSN` | Sentry DSN | — |
| `OTEL_SAMPLE_RATE` | Sample rate 0.0–1.0 | `1.0` |

## MSRV

Rust **1.85** (edition 2024).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.

## Security

Threat model: [THREAT-MODEL.md](THREAT-MODEL.md).
