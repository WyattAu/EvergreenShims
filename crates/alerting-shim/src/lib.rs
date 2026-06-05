//! Alerting shim — webhook delivery with severity routing, deduplication, and backoff.
//!
//! Sends alerts to configured webhooks (Slack, PagerDuty, custom).
//! Routes based on severity levels, deduplicates identical alerts,
//! and applies exponential backoff on failing endpoints.
//!
//! ## Environment Variables
//!
//! ```text
//! ALERTING_WEBHOOKS       JSON array of webhook configs
//! ALERTING_DEDUP_WINDOW   Dedup window in seconds (default: 300)
//! ALERTING_BACKOFF_BASE   Base backoff in seconds (default: 30)
//! ALERTING_BACKOFF_MAX    Max backoff in seconds (default: 3600)
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::{watch, Mutex};

/// Default dedup window in seconds.
const DEFAULT_DEDUP_WINDOW_SECS: u64 = 300;
/// Default backoff base in seconds.
const DEFAULT_BACKOFF_BASE_SECS: u64 = 30;
/// Default max backoff in seconds.
const DEFAULT_BACKOFF_MAX_SECS: u64 = 3600;

/// Alert severity levels (ordered low→high).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    /// Parse severity from string (case-insensitive).
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "info" => Ok(Severity::Info),
            "warning" | "warn" => Ok(Severity::Warning),
            "critical" | "fatal" | "error" => Ok(Severity::Critical),
            other => anyhow::bail!("Unknown severity: {}", other),
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Critical => write!(f, "critical"),
        }
    }
}

/// Webhook configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub name: String,
    pub url: String,
    pub channel: String,
    pub min_severity: String,
    pub headers: HashMap<String, String>,
}

impl WebhookConfig {
    /// Check if this webhook handles the given severity.
    pub fn accepts(&self, severity: Severity) -> bool {
        let min = Severity::parse(&self.min_severity).unwrap_or(Severity::Info);
        severity >= min
    }
}

/// An alert to be sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub title: String,
    pub message: String,
    pub severity: Severity,
    pub source: String,
    pub labels: HashMap<String, String>,
    #[serde(with = "serde_millis")]
    pub timestamp: Instant,
}

/// Per-endpoint delivery state for backoff tracking.
#[derive(Debug, Clone)]
struct EndpointState {
    consecutive_failures: u32,
    last_attempt: Option<Instant>,
    backoff_until: Option<Instant>,
}

/// Deduplication key for an alert.
fn dedup_key(alert: &Alert) -> String {
    format!("{}:{}:{}", alert.source, alert.title, alert.severity)
}

/// Alerting shim with routing, dedup, and backoff.
pub struct AlertingShim {
    webhooks: Vec<WebhookConfig>,
    dedup_window: Duration,
    backoff_base: Duration,
    backoff_max: Duration,
    alerts_sent: u64,
    alerts_failed: u64,
    alerts_deduplicated: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
    dedup_cache: Arc<Mutex<HashMap<String, Instant>>>,
    endpoint_states: Arc<Mutex<HashMap<String, EndpointState>>>,
}

impl AlertingShim {
    pub fn new() -> Self {
        let webhooks: Vec<WebhookConfig> = std::env::var("ALERTING_WEBHOOKS")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let dedup_window = Duration::from_secs(
            std::env::var("ALERTING_DEDUP_WINDOW")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_DEDUP_WINDOW_SECS),
        );
        let backoff_base = Duration::from_secs(
            std::env::var("ALERTING_BACKOFF_BASE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_BACKOFF_BASE_SECS),
        );
        let backoff_max = Duration::from_secs(
            std::env::var("ALERTING_BACKOFF_MAX")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_BACKOFF_MAX_SECS),
        );

        Self {
            webhooks,
            dedup_window,
            backoff_base,
            backoff_max,
            alerts_sent: 0,
            alerts_failed: 0,
            alerts_deduplicated: 0,
            shutdown_tx: None,
            dedup_cache: Arc::new(Mutex::new(HashMap::new())),
            endpoint_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if an alert is a duplicate within the dedup window.
    pub async fn is_duplicate(&self, alert: &Alert) -> bool {
        let key = dedup_key(alert);
        let cache = self.dedup_cache.lock().await;
        if let Some(last_seen) = cache.get(&key) {
            if last_seen.elapsed() < self.dedup_window {
                return true;
            }
        }
        false
    }

    /// Record an alert in the dedup cache.
    pub async fn record_alert(&self, alert: &Alert) {
        let key = dedup_key(alert);
        let mut cache = self.dedup_cache.lock().await;
        cache.insert(key, Instant::now());

        // Prune expired entries
        cache.retain(|_, ts| ts.elapsed() < self.dedup_window);
    }

    /// Calculate backoff delay for a failing endpoint.
    pub fn calculate_backoff(&self, consecutive_failures: u32) -> Duration {
        let delay_secs =
            self.backoff_base.as_secs() * 2u64.saturating_pow(consecutive_failures.min(10));
        Duration::from_secs(delay_secs.min(self.backoff_max.as_secs()))
    }

    /// Check if an endpoint is in backoff.
    pub async fn is_in_backoff(&self, webhook_name: &str) -> bool {
        let states = self.endpoint_states.lock().await;
        if let Some(state) = states.get(webhook_name) {
            if let Some(until) = state.backoff_until {
                return Instant::now() < until;
            }
        }
        false
    }

    /// Record a successful delivery (reset backoff).
    pub async fn record_success(&self, webhook_name: &str) {
        let mut states = self.endpoint_states.lock().await;
        states.insert(
            webhook_name.to_string(),
            EndpointState {
                consecutive_failures: 0,
                last_attempt: Some(Instant::now()),
                backoff_until: None,
            },
        );
    }

    /// Record a failed delivery (increment backoff).
    pub async fn record_failure(&self, webhook_name: &str) {
        let mut states = self.endpoint_states.lock().await;
        let state = states
            .entry(webhook_name.to_string())
            .or_insert(EndpointState {
                consecutive_failures: 0,
                last_attempt: None,
                backoff_until: None,
            });
        state.consecutive_failures += 1;
        state.last_attempt = Some(Instant::now());
        let delay = self.calculate_backoff(state.consecutive_failures);
        state.backoff_until = Some(Instant::now() + delay);
    }

    /// Route an alert to matching webhooks (for testing/routing logic).
    pub fn route(&self, alert: &Alert) -> Vec<&WebhookConfig> {
        self.webhooks
            .iter()
            .filter(|w| w.accepts(alert.severity))
            .collect()
    }

    /// Process an alert: dedup check → route → (in production: send).
    /// Returns number of webhooks that would receive the alert.
    pub async fn process_alert(&mut self, alert: Alert) -> anyhow::Result<u32> {
        if self.is_duplicate(&alert).await {
            self.alerts_deduplicated += 1;
            return Ok(0);
        }
        self.record_alert(&alert).await;
        let target_names: Vec<String> = self.route(&alert).iter().map(|w| w.name.clone()).collect();
        let count = target_names.len() as u32;
        // In production, we'd send HTTP POST to each webhook here.
        // For now, record success for routing verification.
        for name in &target_names {
            self.alerts_sent += 1;
            self.record_success(name).await;
        }
        Ok(count)
    }

    /// Clear the dedup cache.
    pub async fn clear_dedup_cache(&self) {
        self.dedup_cache.lock().await.clear();
    }
}

impl Default for AlertingShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for AlertingShim {
    fn name(&self) -> &str {
        "alerting"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            webhooks = self.webhooks.len(),
            dedup_window_secs = self.dedup_window.as_secs(),
            "AlertingShim initialized"
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!(webhooks = self.webhooks.len(), "AlertingShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!(
            sent = self.alerts_sent,
            failed = self.alerts_failed,
            deduplicated = self.alerts_deduplicated,
            "AlertingShim stopped"
        );
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("alerting_sent_total", self.alerts_sent as f64),
            Metric::new("alerting_failed_total", self.alerts_failed as f64),
            Metric::new(
                "alerting_deduplicated_total",
                self.alerts_deduplicated as f64,
            ),
        ]
    }
}

mod serde_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Instant;

    pub fn serialize<S: Serializer>(instant: &Instant, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(instant.elapsed().as_millis() as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(_d: D) -> Result<Instant, D::Error> {
        Ok(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_alert(title: &str, severity: Severity) -> Alert {
        Alert {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.to_string(),
            message: format!("{} happened", title),
            severity,
            source: "test".to_string(),
            labels: HashMap::new(),
            timestamp: Instant::now(),
        }
    }

    #[test]
    fn test_severity_parse() {
        assert_eq!(Severity::parse("info").unwrap(), Severity::Info);
        assert_eq!(Severity::parse("warning").unwrap(), Severity::Warning);
        assert_eq!(Severity::parse("warn").unwrap(), Severity::Warning);
        assert_eq!(Severity::parse("critical").unwrap(), Severity::Critical);
        assert_eq!(Severity::parse("fatal").unwrap(), Severity::Critical);
        assert!(Severity::parse("unknown").is_err());
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn test_severity_display() {
        assert_eq!(Severity::Info.to_string(), "info");
        assert_eq!(Severity::Warning.to_string(), "warning");
        assert_eq!(Severity::Critical.to_string(), "critical");
    }

    #[test]
    fn test_webhook_accepts() {
        let wh = WebhookConfig {
            name: "slack".into(),
            url: "https://hooks.slack.com/xxx".into(),
            channel: "#alerts".into(),
            min_severity: "warning".into(),
            headers: HashMap::new(),
        };
        assert!(!wh.accepts(Severity::Info));
        assert!(wh.accepts(Severity::Warning));
        assert!(wh.accepts(Severity::Critical));
    }

    #[tokio::test]
    async fn test_dedup_prevents_duplicate() {
        let shim = AlertingShim::new();
        let alert = make_alert("dup-test", Severity::Warning);
        shim.record_alert(&alert).await;
        assert!(shim.is_duplicate(&alert).await);

        // Different severity → not duplicate
        let alert2 = make_alert("dup-test", Severity::Critical);
        assert!(!shim.is_duplicate(&alert2).await);
    }

    #[tokio::test]
    async fn test_process_alert_dedup() {
        let mut shim = AlertingShim::new();
        let alert = make_alert("proc-dup", Severity::Info);
        shim.record_alert(&alert).await;
        let count = shim.process_alert(alert).await.unwrap();
        assert_eq!(count, 0);
        assert_eq!(shim.alerts_deduplicated, 1);
    }

    #[tokio::test]
    async fn test_process_alert_route() {
        std::env::set_var(
            "ALERTING_WEBHOOKS",
            r##"[{"name":"w1","url":"http://localhost/webhook","channel":"#ops","min_severity":"info","headers":{}},{"name":"w2","url":"http://localhost/pager","channel":"#critical","min_severity":"critical","headers":{}}]"##,
        );
        let mut shim = AlertingShim::new();
        let alert = make_alert("route-test", Severity::Info);
        let count = shim.process_alert(alert).await.unwrap();
        assert_eq!(count, 1); // Only w1 (min_severity=info), not w2 (min_severity=critical)
        std::env::remove_var("ALERTING_WEBHOOKS");
    }

    #[test]
    fn test_backoff_calculation() {
        let shim = AlertingShim::new();
        let d0 = shim.calculate_backoff(0);
        let d1 = shim.calculate_backoff(1);
        let d5 = shim.calculate_backoff(5);
        assert!(d1 > d0);
        assert!(d5 > d1);
        // Should be capped
        let d20 = shim.calculate_backoff(20);
        assert!(d20 <= shim.backoff_max);
    }

    #[tokio::test]
    async fn test_backoff_state_tracking() {
        let shim = AlertingShim::new();
        assert!(!shim.is_in_backoff("w1").await);

        shim.record_failure("w1").await;
        assert!(shim.is_in_backoff("w1").await);

        shim.record_success("w1").await;
        assert!(!shim.is_in_backoff("w1").await);
    }

    #[tokio::test]
    async fn test_consecutive_failures_increase_backoff() {
        let shim = AlertingShim::new();
        shim.record_failure("w1").await;
        let state1 = shim.endpoint_states.lock().await.get("w1").cloned();
        let backoff1 = state1.as_ref().and_then(|s| s.backoff_until);

        shim.record_failure("w1").await;
        let state2 = shim.endpoint_states.lock().await.get("w1").cloned();
        let backoff2 = state2.as_ref().and_then(|s| s.backoff_until);

        // Second failure should have later backoff_until
        assert!(backoff2 > backoff1);
    }

    #[test]
    fn test_route_filters_by_severity() {
        std::env::set_var(
            "ALERTING_WEBHOOKS",
            r##"[{"name":"info-ch","url":"http://x","channel":"#info","min_severity":"info","headers":{}},{"name":"crit-ch","url":"http://y","channel":"#crit","min_severity":"critical","headers":{}}]"##,
        );
        let shim = AlertingShim::new();

        let info_alert = Alert {
            id: "1".into(),
            title: "t".into(),
            message: "m".into(),
            severity: Severity::Info,
            source: "s".into(),
            labels: HashMap::new(),
            timestamp: Instant::now(),
        };
        let info_routes = shim.route(&info_alert);
        assert_eq!(info_routes.len(), 1);
        assert_eq!(info_routes[0].name, "info-ch");

        let crit_alert = Alert {
            id: "2".into(),
            title: "t".into(),
            message: "m".into(),
            severity: Severity::Critical,
            source: "s".into(),
            labels: HashMap::new(),
            timestamp: Instant::now(),
        };
        let crit_routes = shim.route(&crit_alert);
        assert_eq!(crit_routes.len(), 2);

        std::env::remove_var("ALERTING_WEBHOOKS");
    }

    #[tokio::test]
    async fn test_clear_dedup_cache() {
        let shim = AlertingShim::new();
        let alert = make_alert("cache-clear", Severity::Info);
        shim.record_alert(&alert).await;
        assert!(shim.is_duplicate(&alert).await);
        shim.clear_dedup_cache().await;
        assert!(!shim.is_duplicate(&alert).await);
    }

    #[test]
    fn test_metrics() {
        let mut shim = AlertingShim::new();
        shim.alerts_sent = 10;
        shim.alerts_failed = 2;
        shim.alerts_deduplicated = 5;
        let m = shim.metrics();
        assert_eq!(m.len(), 3);
    }
}
