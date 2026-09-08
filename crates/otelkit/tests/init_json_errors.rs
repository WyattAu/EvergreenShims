// JSON-format init error path. Runs in its own test binary so the global
// tracing subscriber state is isolated from other init tests.
#![cfg(feature = "sentry")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use otelkit::{TelemetryConfig, TelemetryError};

#[test]
fn init_json_twice_fails_to_rebind_global_subscriber() {
    // The first init installs the JSON subscriber (and a Sentry client);
    // a second init must surface `try_init`'s refusal as `InvalidConfig`.
    assert!(
        otelkit::init(TelemetryConfig::default().sentry_dsn("https://key@sentry.io/1")).is_ok()
    );
    let err = otelkit::init(TelemetryConfig::default().sentry_dsn("https://key@sentry.io/1"));
    assert!(matches!(err, Err(TelemetryError::InvalidConfig(_))));
}
