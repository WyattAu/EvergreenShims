// from_env() coverage via child processes.
//
// `TelemetryConfig::from_env` reads process env vars; Rust 2024 makes
// `std::env::set_var` unsafe, so instead of mutating the test process's
// environment, the parent test re-executes this test binary as a child with
// the desired vars and a marker argument. The child writes the parsed config
// to a temp file; the parent asserts on it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::io::Write;
use std::process::Command;

const CHILD_MARKER: &str = "OTELKIT_CHILD_MARKER";
const CHILD_ENV_PATH: &str = "OTELKIT_CHILD_OUT";

fn write_child_output(content: &str) -> ! {
    let path = std::env::var(CHILD_ENV_PATH).expect("child must receive output path");
    let mut f = std::fs::File::create(&path).expect("create child output file");
    f.write_all(content.as_bytes()).expect("write child output");
    std::process::exit(0);
}

fn run_child(tag: &str, vars: &[(&str, &str)]) -> String {
    let out =
        std::env::temp_dir().join(format!("otelkit-from-env-{}-{tag}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let exe = std::env::current_exe().expect("current_exe");
    let status = Command::new(exe)
        .env(CHILD_MARKER, "1")
        .env(CHILD_ENV_PATH, &out)
        .envs(vars.iter().copied())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn child");
    assert!(status.success(), "child exited with {status}");
    std::fs::read_to_string(&out).expect("read child output")
}

/// Format: name|version|level|format|otlp|sentry|rate
fn child_dump() -> String {
    let cfg = otelkit::from_env().expect("from_env should not fail");
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        cfg.service_name,
        cfg.service_version,
        cfg.log_level,
        if cfg.log_format == otelkit::LogFormat::Json {
            "json"
        } else {
            "text"
        },
        cfg.otlp_endpoint.as_deref().unwrap_or(""),
        cfg.sentry_dsn.as_deref().unwrap_or(""),
        cfg.sample_rate,
    )
}

#[test]
fn from_env_reads_overrides_and_defaults() {
    if std::env::var(CHILD_MARKER).is_ok() {
        write_child_output(&child_dump());
    }

    // All recognized variables set.
    let all = run_child(
        "all",
        &[
            ("OTEL_SERVICE_NAME", "child-svc"),
            ("OTEL_SERVICE_VERSION", "7.7.7"),
            ("OTEL_LOG_LEVEL", "debug"),
            ("OTEL_LOG_FORMAT", "text"),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"),
            ("SENTRY_DSN", "https://key@sentry.io/42"),
            ("OTEL_SAMPLE_RATE", "0.25"),
        ],
    );
    assert_eq!(
        all,
        "child-svc|7.7.7|debug|text|http://collector:4317|https://key@sentry.io/42|0.25"
    );

    // Unparseable sample rate falls back to 1.0.
    let bad_rate = run_child("bad-rate", &[("OTEL_SAMPLE_RATE", "not-a-number")]);
    let rate: f32 = bad_rate.split('|').next_back().unwrap().parse().unwrap();
    assert_eq!(rate, 1.0);

    // Unknown log format maps to json; unset vars fall back to defaults.
    let partial = run_child("bad-format", &[("OTEL_LOG_FORMAT", "banana")]);
    let parts: Vec<&str> = partial.split('|').collect();
    assert_eq!(parts[0], "unknown");
    assert_eq!(parts[1], "0.0.0");
    assert_eq!(parts[2], "info");
    assert_eq!(parts[3], "json");
    assert_eq!(parts[4], "");
    assert_eq!(parts[5], "");
    assert_eq!(parts[6], "1");

    // Format parsing is case-insensitive.
    let upper = run_child("upper", &[("OTEL_LOG_FORMAT", "TEXT")]);
    let upper_parts: Vec<&str> = upper.split('|').collect();
    assert_eq!(upper_parts[3], "text", "got: {upper}");
}
