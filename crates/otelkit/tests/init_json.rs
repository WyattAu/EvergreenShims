// Json-format init success path + guard drop (separate process from
// init_text.rs because tracing allows one successful global init per
// process). unwrap/expect are the test signal here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use otelkit::{LogFormat, TelemetryConfig};

#[test]
fn json_format_init_lifecycle() {
    let config = TelemetryConfig::default()
        .service_name("otelkit-tests")
        .service_version("1.0.0")
        .log_level("info")
        .log_format(LogFormat::Json)
        .sentry_dsn("https://key@sentry.io/1");

    let guard = otelkit::init(config)
        .map_err(|e| format!("init failed: {e}"))
        .unwrap();

    // Emit an event through the installed subscriber so we know it works.
    tracing::info!("otelkit json init verified");

    // Dropping the guard must flush and shut down cleanly (RAII).
    drop(guard);
}
