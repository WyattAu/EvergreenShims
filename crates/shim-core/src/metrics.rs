//! Prometheus metrics collector and HTTP server for shim observability.
//!
//! `ShimMetrics` provides standard counters/gauges for all shims plus
//! a `/metrics` HTTP endpoint that scrapes in Prometheus text format.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use prometheus::{
    Encoder, Gauge, Histogram, HistogramOpts, IntCounter, IntCounterVec, Opts, Registry,
    TextEncoder,
};
use tracing::info;

use crate::error::Result;

/// Standard metrics collected by every shim.
pub struct ShimMetrics {
    pub registry: Registry,

    // ── Event bus metrics ─────────────────────────────────────────────
    /// Total events published on the bus.
    pub events_published: IntCounter,
    /// Total events dropped (lagged receivers).
    pub events_dropped: IntCounter,
    /// Active bus subscribers.
    pub bus_subscribers: Gauge,

    // ── Health metrics ────────────────────────────────────────────────
    /// Health status: 1 = healthy, 0 = unhealthy.
    pub health_status: Gauge,
    /// Total health check evaluations.
    pub health_checks_total: IntCounter,

    // ── Shim lifecycle metrics ────────────────────────────────────────
    /// Uptime in seconds.
    pub uptime_seconds: Gauge,
    /// Total errors encountered.
    pub errors_total: IntCounterVec,
    /// Operation durations (shim-specific).
    pub operation_duration_seconds: Histogram,

    // ── Cross-shim event metrics (per source) ─────────────────────────
    /// Events emitted per source shim.
    pub events_by_source: IntCounterVec,
    /// Events received per handler.
    pub events_handled: IntCounterVec,
}

impl ShimMetrics {
    /// Create a new metrics collector with a fresh registry.
    pub fn new() -> Self {
        let registry = Registry::new();

        let events_published = IntCounter::with_opts(Opts::new(
            "shim_events_published_total",
            "Total events published on the bus",
        ))
        .expect("metric opts for shim_events_published_total are valid");
        let events_dropped = IntCounter::with_opts(Opts::new(
            "shim_events_dropped_total",
            "Total events dropped due to lagged receivers",
        ))
        .expect("metric opts for shim_events_dropped_total are valid");
        let bus_subscribers = Gauge::with_opts(Opts::new(
            "shim_bus_subscribers",
            "Number of active bus subscribers",
        ))
        .expect("metric opts for shim_bus_subscribers are valid");
        let health_status = Gauge::with_opts(Opts::new(
            "shim_health_status",
            "Health status: 1=healthy, 0=unhealthy",
        ))
        .expect("metric opts for shim_health_status are valid");
        let health_checks_total = IntCounter::with_opts(Opts::new(
            "shim_health_checks_total",
            "Total health check evaluations",
        ))
        .expect("metric opts for shim_health_checks_total are valid");
        let uptime_seconds =
            Gauge::with_opts(Opts::new("shim_uptime_seconds", "Uptime in seconds"))
                .expect("metric opts for shim_uptime_seconds are valid");
        let errors_total = IntCounterVec::new(
            Opts::new("shim_errors_total", "Total errors by type"),
            &["error_type"],
        )
        .expect("metric opts for shim_errors_total are valid");
        let operation_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "shim_operation_duration_seconds",
                "Operation duration in seconds",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
        )
        .expect("metric opts for shim_operation_duration_seconds are valid");
        let events_by_source = IntCounterVec::new(
            Opts::new("shim_events_by_source_total", "Events by source shim"),
            &["source"],
        )
        .expect("metric opts for shim_events_by_source_total are valid");
        let events_handled = IntCounterVec::new(
            Opts::new("shim_events_handled_total", "Events handled by handler"),
            &["handler"],
        )
        .expect("metric opts for shim_events_handled_total are valid");

        // Register all metrics
        registry
            .register(Box::new(events_published.clone()))
            .expect("register events_published must not conflict");
        registry
            .register(Box::new(events_dropped.clone()))
            .expect("register events_dropped must not conflict");
        registry
            .register(Box::new(bus_subscribers.clone()))
            .expect("register bus_subscribers must not conflict");
        registry
            .register(Box::new(health_status.clone()))
            .expect("register health_status must not conflict");
        registry
            .register(Box::new(health_checks_total.clone()))
            .expect("register health_checks_total must not conflict");
        registry
            .register(Box::new(uptime_seconds.clone()))
            .expect("register uptime_seconds must not conflict");
        registry
            .register(Box::new(errors_total.clone()))
            .expect("register errors_total must not conflict");
        registry
            .register(Box::new(operation_duration_seconds.clone()))
            .expect("register operation_duration_seconds must not conflict");
        registry
            .register(Box::new(events_by_source.clone()))
            .expect("register events_by_source must not conflict");
        registry
            .register(Box::new(events_handled.clone()))
            .expect("register events_handled must not conflict");

        Self {
            registry,
            events_published,
            events_dropped,
            bus_subscribers,
            health_status,
            health_checks_total,
            uptime_seconds,
            errors_total,
            operation_duration_seconds,
            events_by_source,
            events_handled,
        }
    }

    /// Record an error.
    pub fn record_error(&self, error_type: &str) {
        self.errors_total.with_label_values(&[error_type]).inc();
    }

    /// Set health status from bool.
    pub fn set_healthy(&self, healthy: bool) {
        self.health_status.set(if healthy { 1.0 } else { 0.0 });
    }

    /// Export all metrics as Prometheus text format.
    pub fn export(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .expect("text encoder write to Vec must succeed");
        String::from_utf8(buffer).expect("prometheus text output is valid UTF-8")
    }

    /// Register a custom external collector.
    pub fn register_custom(&self, collector: Box<dyn prometheus::core::Collector>) {
        if let Err(e) = self.registry.register(collector) {
            tracing::warn!("Failed to register custom metric: {}", e);
        }
    }
}

impl Default for ShimMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Start a Prometheus metrics HTTP server using axum.
///
/// Serves `/metrics` (Prometheus text format) and `/healthz` (JSON status).
pub async fn start_metrics_server(addr: SocketAddr, metrics: Arc<ShimMetrics>) -> Result<()> {
    use axum::{routing::get, Router};

    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/healthz", get(healthz_handler))
        .with_state(metrics);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("metrics server listening on {}", addr);

    axum::serve(listener, app)
        .await
        .map_err(|e| crate::error::Error::Connection(format!("metrics server: {}", e)))?;

    Ok(())
}

async fn metrics_handler(State(metrics): State<Arc<ShimMetrics>>) -> axum::response::Response {
    let body = metrics.export();
    match axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "text/plain; version=0.0.4")
        .body(axum::body::Body::from(body))
    {
        Ok(resp) => resp,
        Err(_) => axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::from("internal server error"))
            .expect("500 response builder must succeed"),
    }
}

async fn healthz_handler(State(metrics): State<Arc<ShimMetrics>>) -> axum::response::Response {
    let healthy = metrics.health_status.get() > 0.5;
    let status = serde_json::json!({
        "status": if healthy { "healthy" } else { "unhealthy" },
        "uptime_seconds": metrics.uptime_seconds.get(),
    });
    let body = match serde_json::to_string(&status) {
        Ok(b) => b,
        Err(_) => {
            return axum::response::Response::builder()
                .status(500)
                .body(axum::body::Body::from("internal server error"))
                .expect("500 response builder must succeed")
        }
    };
    match axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
    {
        Ok(resp) => resp,
        Err(_) => axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::from("internal server error"))
            .expect("500 response builder must succeed"),
    }
}

/// A single metric value (backward-compat type for shim consumers).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Metric {
    /// Metric name.
    pub name: String,
    /// Metric value.
    pub value: f64,
    /// Metric labels.
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

impl Metric {
    /// Create a new metric.
    pub fn new(name: &str, value: f64) -> Self {
        Self {
            name: name.to_string(),
            value,
            labels: std::collections::HashMap::new(),
        }
    }

    /// Create a metric with labels.
    pub fn with_labels(
        name: &str,
        value: f64,
        labels: std::collections::HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.to_string(),
            value,
            labels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_new() {
        let m = ShimMetrics::new();
        assert_eq!(m.events_published.get(), 0);
        assert_eq!(m.events_dropped.get(), 0);
        assert_eq!(m.health_status.get(), 0.0);
    }

    #[test]
    fn test_metrics_export() {
        let m = ShimMetrics::new();
        m.events_published.inc();
        m.events_published.inc();
        m.health_status.set(1.0);

        let output = m.export();
        assert!(output.contains("shim_events_published_total 2"));
        assert!(output.contains("shim_health_status 1"));
    }

    #[test]
    fn test_record_error() {
        let m = ShimMetrics::new();
        m.record_error("connection");
        m.record_error("connection");
        m.record_error("timeout");

        let output = m.export();
        assert!(output.contains("shim_errors_total"));
        assert!(output.contains("connection"));
    }

    #[test]
    fn test_set_healthy() {
        let m = ShimMetrics::new();
        m.set_healthy(true);
        assert_eq!(m.health_status.get(), 1.0);
        m.set_healthy(false);
        assert_eq!(m.health_status.get(), 0.0);
    }

    #[test]
    fn test_uptime() {
        let m = ShimMetrics::new();
        m.uptime_seconds.set(42.5);
        let output = m.export();
        assert!(output.contains("shim_uptime_seconds 42.5"));
    }

    #[test]
    fn test_events_by_source() {
        let m = ShimMetrics::new();
        m.events_by_source.with_label_values(&["backup-shim"]).inc();
        m.events_by_source.with_label_values(&["backup-shim"]).inc();
        m.events_by_source.with_label_values(&["tls-shim"]).inc();

        let output = m.export();
        assert!(output.contains("backup-shim"));
        assert!(output.contains("tls-shim"));
    }

    #[test]
    fn test_operation_duration() {
        let m = ShimMetrics::new();
        m.operation_duration_seconds.observe(0.05);
        let output = m.export();
        assert!(output.contains("shim_operation_duration_seconds"));
    }

    #[test]
    fn test_prometheus_format_valid() {
        let m = ShimMetrics::new();
        m.events_published.inc();
        m.health_status.set(1.0);

        let output = m.export();
        // Prometheus text format lines should start with # or metric name
        for line in output.lines() {
            if !line.is_empty() && !line.starts_with('#') {
                // Should be a metric line: name{labels} value [timestamp]
                assert!(
                    line.chars()
                        .next()
                        .expect("non-empty prometheus line")
                        .is_alphabetic()
                        || line.starts_with('_'),
                    "Invalid prometheus line: {}",
                    line
                );
            }
        }
    }

    #[tokio::test]
    async fn test_metrics_server_serves_metrics() {
        use std::net::SocketAddr;

        let metrics = Arc::new(ShimMetrics::new());
        metrics.events_published.inc();
        metrics.events_published.inc();
        metrics.health_status.set(1.0);

        let app = axum::Router::new()
            .route("/metrics", axum::routing::get(metrics_handler))
            .with_state(metrics.clone());

        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let actual_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Use raw TCP to avoid reqwest TLS issues
        let mut stream = tokio::net::TcpStream::connect(actual_addr).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream
            .write_all(b"GET /metrics HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(response.contains("shim_events_published_total 2"));
        assert!(response.contains("shim_health_status 1"));
    }
}
