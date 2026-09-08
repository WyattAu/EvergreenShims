// TelemetryGuard::drop + init() text-format paths.
//
// tracing installs one global subscriber per process, so init() can only
// succeed once per test binary. Each scenario lives in its own integration
// test file (separate process) and sequences its phases inside a single
// #[test] to keep ordering deterministic.
//
// unwrap/expect are the test signal here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use otelkit::{LogFormat, TelemetryConfig, TelemetryError};

/// `init` variant for the failure assertions (TelemetryGuard has no Debug,
/// so `.unwrap_err()` is not usable on `Result<TelemetryGuard, _>`).
fn init_err(config: TelemetryConfig) -> TelemetryError {
    match otelkit::init(config) {
        Err(e) => e,
        Ok(_) => panic!("expected init to fail"),
    }
}

#[test]
fn text_format_init_lifecycle() {
    // 1. Invalid log level is rejected before any global state is touched.
    //    (EnvFilter treats bare words as target names, so an invalid LEVEL
    //    after '=' is what actually fails to parse.)
    let err = init_err(TelemetryConfig::default().log_level("foo=notalevel"));
    assert!(
        matches!(err, TelemetryError::InvalidConfig(_)),
        "got: {err}"
    );

    // 2. Text format without a Sentry DSN.
    #[cfg(feature = "sentry")]
    {
        // With the sentry feature compiled in, a missing DSN is a config
        // error — but note the subscriber is already installed at that
        // point, so the rest of this process can never init successfully.
        let err = init_err(TelemetryConfig::default().log_format(LogFormat::Text));
        assert!(
            matches!(&err, TelemetryError::InvalidConfig(msg) if msg.contains("DSN")),
            "got: {err}"
        );

        // 3. Any further init fails because the global subscriber is taken.
        let err = init_err(
            TelemetryConfig::default()
                .log_format(LogFormat::Text)
                .sentry_dsn("https://key@sentry.io/1"),
        );
        assert!(
            matches!(err, TelemetryError::InvalidConfig(_)),
            "got: {err}"
        );
    }

    #[cfg(not(feature = "sentry"))]
    {
        // Without the sentry feature the same config initializes cleanly.
        let guard = match otelkit::init(
            TelemetryConfig::default()
                .log_format(LogFormat::Text)
                .log_level("info"),
        ) {
            Ok(g) => g,
            Err(e) => panic!("text init should succeed: {e}"),
        };
        drop(guard);

        // A second init in the same process must fail: the global
        // subscriber is already installed.
        let err = init_err(TelemetryConfig::default().log_format(LogFormat::Text));
        assert!(
            matches!(err, TelemetryError::InvalidConfig(_)),
            "got: {err}"
        );
    }
}
