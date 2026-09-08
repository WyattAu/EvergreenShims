// OTLP init error paths. Runs in its own test binary so the global
// tracing subscriber state is isolated from other init tests.
#![cfg(feature = "otlp")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use otelkit::{TelemetryConfig, TelemetryError};

#[test]
fn init_otlp_malformed_endpoint_maps_to_connection_error() {
    // Exporter construction fails before any I/O; the error is mapped to
    // `OtlpConnection` with the exporter message.
    let err = otelkit::init(TelemetryConfig::default().otlp_endpoint("not a url"));
    assert!(matches!(err, Err(TelemetryError::OtlpConnection(_))));
}

#[test]
fn init_otlp_twice_fails_to_rebind_global_subscriber() {
    // The first successful init binds the process-global subscriber; a
    // second init must surface `try_init`'s refusal as `InvalidConfig`.
    assert!(
        otelkit::init(TelemetryConfig::default().otlp_endpoint("http://127.0.0.1:4317")).is_ok()
    );
    let err = otelkit::init(TelemetryConfig::default().otlp_endpoint("http://127.0.0.1:4317"));
    assert!(matches!(err, Err(TelemetryError::InvalidConfig(_))));
}
