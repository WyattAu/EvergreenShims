//! Prometheus metrics export server with per-shim metrics.
//!
//! `MetricsExporter` wraps `ShimMetrics` and adds a `shim_up` gauge,
//! per-shim custom metrics, and serves them on HTTP port 9101 with
//! `/metrics` (Prometheus text format) and `/healthz` (JSON) endpoints.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use prometheus::{
    Encoder, Gauge, IntCounter, IntCounterVec, Opts, Registry, TextEncoder,
};
use tracing::info;

use crate::error::Result;
use crate::metrics::ShimMetrics;

/// Default metrics export port.
pub const DEFAULT_METRICS_PORT: u16 = 9101;

/// Per-shim type metrics added beyond the standard `ShimMetrics`.
pub struct MetricsExporter {
    /// The underlying standard metrics collector.
    pub base: ShimMetrics,
    /// Prometheus registry for exporter-specific metrics.
    registry: Registry,
    /// shim_up gauge: 1 if the shim is running, 0 otherwise.
    pub shim_up: Gauge,
    /// shim_events_total counter (per event_type label).
    pub events_total: IntCounterVec,
    /// shim_alerts_forwarded_total counter (for AlertManager integration).
    pub alerts_forwarded: IntCounter,
    /// shim_alerts_dropped_total counter.
    pub alerts_dropped: IntCounter,
    /// shim_health_probes_total counter (per probe type: liveness/readiness).
    pub health_probes: IntCounterVec,
    /// shim_config_reloads_total counter.
    pub config_reloads: IntCounter,
    /// Per-shim custom metrics (shim_name → metric_name → value).
    custom_metrics: Arc<std::sync::RwLock<HashMap<String, HashMap<String, f64>>>>,
    /// Start time for uptime calculation.
    start_time: Instant,
    /// Whether the exporter is running.
    running: Arc<AtomicBool>,
    /// Total scrape requests served.
    scrape_count: Arc<AtomicU64>,
}

impl MetricsExporter {
    /// Create a new MetricsExporter wrapping the given ShimMetrics.
    pub fn new(base: ShimMetrics) -> Self {
        let registry = Registry::new();

        let shim_up = Gauge::with_opts(Opts::new(
            "shim_up",
            "1 if the shim is running, 0 otherwise",
        ))
        .expect("shim_up opts are valid");

        let events_total = IntCounterVec::new(
            Opts::new("shim_events_total", "Total events processed by type"),
            &["event_type"],
        )
        .expect("shim_events_total opts are valid");

        let alerts_forwarded = IntCounter::with_opts(Opts::new(
            "shim_alerts_forwarded_total",
            "Total alerts forwarded to AlertManager",
        ))
        .expect("shim_alerts_forwarded_total opts are valid");

        let alerts_dropped = IntCounter::with_opts(Opts::new(
            "shim_alerts_dropped_total",
            "Total alerts dropped (filtered or deduplicated)",
        ))
        .expect("shim_alerts_dropped_total opts are valid");

        let health_probes = IntCounterVec::new(
            Opts::new(
                "shim_health_probes_total",
                "Total health check probes executed",
            ),
            &["probe_type"],
        )
        .expect("shim_health_probes_total opts are valid");

        let config_reloads = IntCounter::with_opts(Opts::new(
            "shim_config_reloads_total",
            "Total config reload attempts",
        ))
        .expect("shim_config_reloads_total opts are valid");

        // Register exporter-specific metrics
        registry
            .register(Box::new(shim_up.clone()))
            .expect("register shim_up");
        registry
            .register(Box::new(events_total.clone()))
            .expect("register shim_events_total");
        registry
            .register(Box::new(alerts_forwarded.clone()))
            .expect("register shim_alerts_forwarded_total");
        registry
            .register(Box::new(alerts_dropped.clone()))
            .expect("register shim_alerts_dropped_total");
        registry
            .register(Box::new(health_probes.clone()))
            .expect("register shim_health_probes_total");
        registry
            .register(Box::new(config_reloads.clone()))
            .expect("register shim_config_reloads_total");

        Self {
            base,
            registry,
            shim_up,
            events_total,
            alerts_forwarded,
            alerts_dropped,
            health_probes,
            config_reloads,
            custom_metrics: Arc::new(std::sync::RwLock::new(HashMap::new())),
            start_time: Instant::now(),
            running: Arc::new(AtomicBool::new(false)),
            scrape_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Mark the shim as up (running).
    pub fn mark_up(&self) {
        self.shim_up.set(1.0);
        self.running.store(true, Ordering::Relaxed);
    }

    /// Mark the shim as down (not running).
    pub fn mark_down(&self) {
        self.shim_up.set(0.0);
        self.running.store(false, Ordering::Relaxed);
    }

    /// Record an event by type.
    pub fn record_event(&self, event_type: &str) {
        self.events_total.with_label_values(&[event_type]).inc();
    }

    /// Record an alert forwarded to AlertManager.
    pub fn record_alert_forwarded(&self) {
        self.alerts_forwarded.inc();
    }

    /// Record an alert dropped.
    pub fn record_alert_dropped(&self) {
        self.alerts_dropped.inc();
    }

    /// Record a health probe execution.
    pub fn record_health_probe(&self, probe_type: &str) {
        self.health_probes.with_label_values(&[probe_type]).inc();
    }

    /// Record a config reload attempt.
    pub fn record_config_reload(&self) {
        self.config_reloads.inc();
    }

    /// Set a per-shim custom metric value.
    pub fn set_custom_metric(&self, shim_name: &str, metric_name: &str, value: f64) {
        let mut metrics = self.custom_metrics.write().unwrap();
        metrics
            .entry(shim_name.to_string())
            .or_default()
            .insert(metric_name.to_string(), value);
    }

    /// Get uptime in seconds.
    pub fn uptime_seconds(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Check if the exporter considers the shim running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Get total number of scrape requests served.
    pub fn scrape_count(&self) -> u64 {
        self.scrape_count.load(Ordering::Relaxed)
    }

    /// Export all metrics (base + exporter-specific) as Prometheus text.
    pub fn export_all(&self) -> String {
        let encoder = TextEncoder::new();

        // Gather from both registries
        let mut all_families = self.base.registry.gather();
        all_families.extend(self.registry.gather());

        let mut buffer = Vec::new();
        encoder
            .encode(&all_families, &mut buffer)
            .expect("text encoder write to Vec must succeed");
        String::from_utf8(buffer).expect("prometheus text output is valid UTF-8")
    }

    /// Start the metrics HTTP server on the given address.
    pub async fn start_server(self: Arc<Self>, addr: SocketAddr) -> Result<()> {
        self.mark_up();

        let state = MetricsServerState {
            exporter: self.clone(),
        };

        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .route("/healthz", get(healthz_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!("metrics exporter listening on {}", addr);

        axum::serve(listener, app)
            .await
            .map_err(|e| crate::error::Error::Connection(format!("metrics server: {}", e)))?;

        Ok(())
    }

    /// Start the metrics HTTP server on the default port (9101).
    pub async fn start_default_server(self: Arc<Self>) -> Result<()> {
        let addr: SocketAddr = format!("0.0.0.0:{}", DEFAULT_METRICS_PORT)
            .parse()
            .map_err(|e| crate::error::Error::Config(format!("invalid metrics addr: {}", e)))?;
        self.start_server(addr).await
    }
}

/// Shared state for the axum HTTP handlers.
#[derive(Clone)]
struct MetricsServerState {
    exporter: Arc<MetricsExporter>,
}

/// Handler for GET /metrics — returns Prometheus text format.
async fn metrics_handler(State(state): State<MetricsServerState>) -> Response {
    state.exporter.scrape_count.fetch_add(1, Ordering::Relaxed);

    let body = state.exporter.export_all();
    (
        StatusCode::OK,
        [("Content-Type", "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

/// Handler for GET /healthz — returns JSON health status.
async fn healthz_handler(State(state): State<MetricsServerState>) -> Response {
    let healthy = state.exporter.is_running();
    let status = serde_json::json!({
        "status": if healthy { "healthy" } else { "unhealthy" },
        "uptime_seconds": state.exporter.uptime_seconds(),
        "shim_up": state.exporter.shim_up.get() > 0.5,
        "scrape_count": state.exporter.scrape_count(),
    });

    let body = match serde_json::to_string(&status) {
        Ok(b) => b,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    };

    (StatusCode::OK, [("Content-Type", "application/json")], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::ShimMetrics;

    fn make_exporter() -> MetricsExporter {
        MetricsExporter::new(ShimMetrics::new())
    }

    #[test]
    fn test_exporter_new() {
        let exp = make_exporter();
        assert!(!exp.is_running());
        assert_eq!(exp.scrape_count(), 0);
    }

    #[test]
    fn test_mark_up_down() {
        let exp = make_exporter();
        assert!(!exp.is_running());
        exp.mark_up();
        assert!(exp.is_running());
        exp.mark_down();
        assert!(!exp.is_running());
    }

    #[test]
    fn test_export_all_contains_base_metrics() {
        let exp = make_exporter();
        exp.base.events_published.inc();
        exp.base.events_published.inc();
        exp.base.health_status.set(1.0);

        let output = exp.export_all();
        assert!(output.contains("shim_events_published_total 2"));
        assert!(output.contains("shim_health_status 1"));
    }

    #[test]
    fn test_export_all_contains_exporter_metrics() {
        let exp = make_exporter();
        exp.mark_up();
        exp.record_event("backup_completed");
        exp.record_event("backup_completed");
        exp.record_event("tls_cert_renewed");
        exp.record_alert_forwarded();
        exp.record_health_probe("liveness");
        exp.record_config_reload();

        let output = exp.export_all();
        assert!(output.contains("shim_up 1"));
        assert!(output.contains("shim_events_total"));
        assert!(output.contains("backup_completed"));
        assert!(output.contains("shim_alerts_forwarded_total 1"));
        assert!(output.contains("shim_health_probes_total"));
        assert!(output.contains("liveness"));
        assert!(output.contains("shim_config_reloads_total 1"));
    }

    #[test]
    fn test_custom_metrics() {
        let exp = make_exporter();
        exp.set_custom_metric("backup-shim", "backups_completed", 42.0);
        exp.set_custom_metric("backup-shim", "backups_failed", 3.0);
        exp.set_custom_metric("tls-shim", "certs_renewed", 12.0);

        let metrics = exp.custom_metrics.read().unwrap();
        assert_eq!(
            metrics.get("backup-shim").unwrap().get("backups_completed"),
            Some(&42.0)
        );
        assert_eq!(
            metrics.get("backup-shim").unwrap().get("backups_failed"),
            Some(&3.0)
        );
        assert_eq!(
            metrics.get("tls-shim").unwrap().get("certs_renewed"),
            Some(&12.0)
        );
    }

    #[test]
    fn test_uptime_seconds() {
        let exp = make_exporter();
        let u1 = exp.uptime_seconds();
        assert!(u1 >= 0.0);
        // Small sleep to verify uptime increases
        std::thread::sleep(std::time::Duration::from_millis(10));
        let u2 = exp.uptime_seconds();
        assert!(u2 > u1);
    }

    #[test]
    fn test_scrape_count() {
        let exp = make_exporter();
        assert_eq!(exp.scrape_count(), 0);
        exp.scrape_count.fetch_add(1, Ordering::Relaxed);
        exp.scrape_count.fetch_add(1, Ordering::Relaxed);
        assert_eq!(exp.scrape_count(), 2);
    }

    #[test]
    fn test_prometheus_format_valid() {
        let exp = make_exporter();
        exp.mark_up();
        exp.record_event("test_event");

        let output = exp.export_all();
        for line in output.lines() {
            if !line.is_empty() && !line.starts_with('#') {
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

    #[test]
    fn test_down_metric() {
        let exp = make_exporter();
        exp.mark_down();
        let output = exp.export_all();
        assert!(output.contains("shim_up 0"));
    }

    #[test]
    fn test_base_metrics_integrated() {
        let exp = make_exporter();
        exp.base.record_error("connection");
        exp.base.set_healthy(true);

        let output = exp.export_all();
        assert!(output.contains("shim_errors_total"));
        assert!(output.contains("connection"));
        assert!(output.contains("shim_health_status 1"));
    }

    #[tokio::test]
    async fn test_metrics_server_endpoints() {
        let exp = Arc::new(make_exporter());
        exp.mark_up();
        exp.record_event("test");

        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let actual_addr = listener.local_addr().unwrap();

        let state = MetricsServerState {
            exporter: exp.clone(),
        };

        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .route("/healthz", get(healthz_handler))
            .with_state(state);

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Test /metrics
        {
            let mut stream = tokio::net::TcpStream::connect(actual_addr).await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            stream
                .write_all(b"GET /metrics HTTP/1.0\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let response = String::from_utf8_lossy(&buf[..n]);
            assert!(response.contains("shim_up 1"));
            assert!(response.contains("shim_events_total"));
        }

        // Test /healthz
        {
            let mut stream = tokio::net::TcpStream::connect(actual_addr).await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            stream
                .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let response = String::from_utf8_lossy(&buf[..n]);
            assert!(response.contains("healthy"));
            assert!(response.contains("uptime_seconds"));
        }
    }
}
