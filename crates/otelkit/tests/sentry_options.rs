// init() must plumb `sample_rate` and `service_version` into the Sentry
// ClientOptions (`traces_sample_rate` / `release`). Both are observable in
// process via the client bound to the hub — no network traffic involved.
//
// Separate integration-test file: init() installs the one global tracing
// subscriber this process is allowed to have.
//
// unwrap/expect are the test signal here.
#![cfg(feature = "sentry")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use otelkit::TelemetryConfig;

#[test]
fn sentry_options_carry_sample_rate_and_release() {
    let guard = otelkit::init(
        TelemetryConfig::default()
            .sentry_dsn("https://key@sentry.io/1")
            .service_version("9.9.9")
            .sample_rate(0.25),
    )
    .expect("init with a DSN should succeed");

    let client = sentry::Hub::current()
        .client()
        .expect("sentry::init binds the client to the current hub");
    let options = client.options();
    assert!((options.traces_sample_rate - 0.25).abs() < f32::EPSILON);
    assert_eq!(options.release.as_deref(), Some("9.9.9"));

    drop(guard);
}
