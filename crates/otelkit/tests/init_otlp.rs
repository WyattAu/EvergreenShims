// OTLP init success path + tracer-provider shutdown on guard drop.
// The OTLP HTTP exporter is constructed without connecting, so this is
// hermetic — no collector endpoint is required. Runs as its own process
// (one successful tracing global init per process).
#![cfg(feature = "otlp")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use otelkit::TelemetryConfig;

#[test]
fn otlp_init_and_guard_shutdown() {
    let config = TelemetryConfig::default()
        .service_name("otelkit-otlp-tests")
        .log_level("info")
        .otlp_endpoint("http://127.0.0.1:4317");

    let guard = otelkit::init(config)
        .map_err(|e| format!("init failed: {e}"))
        .unwrap();

    // Spans flow through the OTLP layer; nothing is exported because no
    // collector is listening, but the pipeline must be wired up.
    tracing::info!("otelkit otlp init verified");

    // Drop triggers SdkTracerProvider::shutdown(); with no pending spans
    // this returns without error output.
    drop(guard);
}
