//! Structured logging initialization for shim processes.
//!
//! Provides `init_structured_logging` which sets up the global tracing
//! subscriber with either human-readable or JSON-formatted output.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Initialize structured logging for the application.
///
/// When `json` is true, logs are emitted as JSON objects suitable for log aggregators.
/// When `json` is false, human-readable colored output is used.
///
/// The `debug` flag sets the default log level to `debug` when true, `info` otherwise.
/// The `RUST_LOG` environment variable always takes precedence.
///
/// Returns `Ok(())` if the subscriber was set, or `Err` if a global subscriber
/// was already installed (safe to ignore in tests or when called multiple times).
pub fn init_structured_logging(
    debug: bool,
    json: bool,
) -> Result<(), tracing_subscriber::util::TryInitError> {
    let default_level = if debug { "debug" } else { "info" };

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    if json {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                fmt::layer()
                    .json()
                    .with_target(true)
                    .with_thread_ids(true)
                    .with_file(true)
                    .with_line_number(true),
            )
            .try_init()
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer())
            .try_init()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_human_readable_returns_ok_or_already_set() {
        // First call may succeed or fail if another test set the subscriber first.
        let _ = init_structured_logging(false, false);
    }

    #[test]
    fn init_json_returns_ok_or_already_set() {
        let _ = init_structured_logging(false, true);
    }

    #[test]
    fn init_debug_mode_returns_ok_or_already_set() {
        let _ = init_structured_logging(true, false);
    }

    #[test]
    fn init_debug_json_returns_ok_or_already_set() {
        let _ = init_structured_logging(true, true);
    }

    #[test]
    fn double_init_does_not_panic() {
        let _ = init_structured_logging(false, false);
        // Second call should return Err, not panic.
        let result = init_structured_logging(false, false);
        assert!(result.is_err());
    }
}
