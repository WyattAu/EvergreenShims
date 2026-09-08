// Sentry-gated init error path, isolated in its own test binary so the
// process-global tracing subscriber is untouched by other init tests.
#![cfg(feature = "sentry")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use otelkit::{TelemetryConfig, TelemetryError};

#[test]
fn init_without_otlp_endpoint_or_sentry_dsn_is_invalid_config() {
    // With the `sentry` feature compiled in, a config that selects neither
    // the OTLP path nor provides a DSN fails the DSN requirement.
    let err = otelkit::init(TelemetryConfig::default());
    assert!(matches!(err, Err(TelemetryError::InvalidConfig(_))));
}
