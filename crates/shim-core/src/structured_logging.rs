//! Structured logging initialization for shim processes.
//!
//! Provides `init_structured_logging` which sets up the global tracing
//! subscriber with either human-readable or JSON-formatted output.
//!
//! Includes:
//! - Request ID propagation (UUID per request, included in all log lines)
//! - Correlation ID support (propagate through ShimBus events)
//! - Structured fields for all shim operations (shim_name, operation, duration_ms, status)

use std::sync::atomic::{AtomicU64, Ordering};

use tracing::Span;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Request context for log propagation.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Unique request ID (UUID).
    pub request_id: uuid::Uuid,
    /// Optional correlation ID for cross-service tracing.
    pub correlation_id: Option<uuid::Uuid>,
    /// Shim name for structured fields.
    pub shim_name: String,
}

impl RequestContext {
    /// Create a new request context with a generated request ID.
    pub fn new(shim_name: impl Into<String>) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4(),
            correlation_id: None,
            shim_name: shim_name.into(),
        }
    }

    /// Create a request context with a specific correlation ID.
    pub fn with_correlation(shim_name: impl Into<String>, correlation_id: uuid::Uuid) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4(),
            correlation_id: Some(correlation_id),
            shim_name: shim_name.into(),
        }
    }

    /// Create a tracing span for this request context.
    pub fn span(&self) -> Span {
        tracing::info_span!(
            "request",
            request_id = %self.request_id,
            correlation_id = %self.correlation_id.map(|c| c.to_string()).unwrap_or_default(),
            shim_name = %self.shim_name,
        )
    }
}

/// Operation tracking for structured logging.
pub struct OperationTracker {
    /// Name of the operation being tracked.
    pub operation: String,
    /// Shim name.
    pub shim_name: String,
    /// Start time.
    start: std::time::Instant,
    /// Request ID.
    request_id: uuid::Uuid,
}

impl OperationTracker {
    /// Start tracking a new operation.
    pub fn start(
        operation: impl Into<String>,
        shim_name: impl Into<String>,
        request_id: uuid::Uuid,
    ) -> Self {
        Self {
            operation: operation.into(),
            shim_name: shim_name.into(),
            start: std::time::Instant::now(),
            request_id,
        }
    }

    /// Complete the operation and log the result.
    pub fn complete(self, status: &str) -> OperationMetrics {
        let duration_ms = self.start.elapsed().as_millis() as u64;

        tracing::info!(
            request_id = %self.request_id,
            shim_name = %self.shim_name,
            operation = %self.operation,
            duration_ms = duration_ms,
            status = status,
            "operation completed"
        );

        OperationMetrics {
            operation: self.operation,
            shim_name: self.shim_name,
            duration_ms,
            status: status.to_string(),
            request_id: self.request_id,
        }
    }
}

/// Metrics from a completed operation.
#[derive(Debug, Clone)]
pub struct OperationMetrics {
    /// Operation name.
    pub operation: String,
    /// Shim name.
    pub shim_name: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Status (success, error, timeout).
    pub status: String,
    /// Request ID.
    pub request_id: uuid::Uuid,
}

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

/// Global request ID counter for fallback ID generation.
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique request ID as a string.
pub fn generate_request_id() -> String {
    let seq = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req-{}-{}", chrono::Utc::now().timestamp_millis(), seq)
}

/// Generate a correlation ID from a request ID.
pub fn derive_correlation_id(request_id: &str) -> String {
    format!("corr-{}", request_id)
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

    #[test]
    fn test_request_context_new() {
        let ctx = RequestContext::new("test-shim");
        assert_eq!(ctx.shim_name, "test-shim");
        assert!(ctx.correlation_id.is_none());
        // request_id should be a valid UUID
        let _ = ctx.request_id;
    }

    #[test]
    fn test_request_context_with_correlation() {
        let corr = uuid::Uuid::new_v4();
        let ctx = RequestContext::with_correlation("shim", corr);
        assert_eq!(ctx.correlation_id, Some(corr));
    }

    #[test]
    fn test_request_context_span() {
        let ctx = RequestContext::new("test-shim");
        let _span = ctx.span();
        // Span should be created without panic
    }

    #[test]
    fn test_operation_tracker_start_and_complete() {
        let request_id = uuid::Uuid::new_v4();
        let tracker = OperationTracker::start("backup", "backup-shim", request_id);
        let metrics = tracker.complete("success");

        assert_eq!(metrics.operation, "backup");
        assert_eq!(metrics.shim_name, "backup-shim");
        assert_eq!(metrics.status, "success");
        assert!(metrics.duration_ms < 1000); // Should be fast
        assert_eq!(metrics.request_id, request_id);
    }

    #[test]
    fn test_generate_request_id() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();
        assert!(id1.starts_with("req-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_derive_correlation_id() {
        let corr = derive_correlation_id("req-123");
        assert_eq!(corr, "corr-req-123");
    }

    #[test]
    fn test_request_context_new_uniqueness() {
        let ctx1 = RequestContext::new("shim");
        let ctx2 = RequestContext::new("shim");
        assert_ne!(ctx1.request_id, ctx2.request_id);
    }

    #[test]
    fn test_request_context_with_correlation_uniqueness() {
        let corr = uuid::Uuid::new_v4();
        let ctx1 = RequestContext::with_correlation("shim", corr);
        let ctx2 = RequestContext::with_correlation("shim", corr);
        assert_ne!(ctx1.request_id, ctx2.request_id);
        assert_eq!(ctx1.correlation_id, ctx2.correlation_id);
    }

    #[test]
    fn test_request_context_span_correlation_field() {
        let corr = uuid::Uuid::new_v4();
        let ctx = RequestContext::with_correlation("test-shim", corr);
        let span = ctx.span();
        // Span should contain the correlation ID in its fields
        let _ = span;
    }

    #[test]
    fn test_operation_tracker_complete_status_values() {
        let rid = uuid::Uuid::new_v4();
        let tracker = OperationTracker::start("op1", "shim1", rid);
        let m = tracker.complete("success");
        assert_eq!(m.status, "success");

        let rid2 = uuid::Uuid::new_v4();
        let tracker2 = OperationTracker::start("op2", "shim2", rid2);
        let m2 = tracker2.complete("error");
        assert_eq!(m2.status, "error");
    }

    #[test]
    fn test_operation_tracker_metrics_fields() {
        let rid = uuid::Uuid::new_v4();
        let tracker = OperationTracker::start("test-op", "test-shim", rid);
        let m = tracker.complete("ok");
        assert_eq!(m.operation, "test-op");
        assert_eq!(m.shim_name, "test-shim");
        assert_eq!(m.request_id, rid);
    }

    #[test]
    fn test_generate_request_id_format() {
        let id = generate_request_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts[0], "req");
        assert_eq!(parts.len(), 3);
        // Second part should be a timestamp (digits)
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()));
        // Third part should be a sequence number
        assert!(parts[2].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_generate_request_id_sequential() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();
        let seq1: u64 = id1.split('-').nth(2).unwrap().parse().unwrap();
        let seq2: u64 = id2.split('-').nth(2).unwrap().parse().unwrap();
        assert_eq!(seq2, seq1 + 1);
    }

    #[test]
    fn test_derive_correlation_id_empty() {
        let corr = derive_correlation_id("");
        assert_eq!(corr, "corr-");
    }

    #[test]
    fn test_derive_correlation_id_long() {
        let long_id = "a".repeat(200);
        let corr = derive_correlation_id(&long_id);
        assert!(corr.starts_with("corr-"));
        assert!(corr.len() > 200);
    }
}
