//! AlertManager integration — converts ShimBus events to AlertManager webhooks.
//!
//! `AlertManager` subscribes to `ShimBus`, filters alertable events, and
//! forwards them to configured AlertManager-compatible webhook endpoints.
//! Supports correlation IDs for distributed tracing and all five severity
//! levels (info → critical).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

use crate::bus::ShimBus;
use crate::event::{Severity, ShimEvent};

/// A standalone alert (simpler than [`AlertManagerAlert`]) for direct alert creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub source: String,
    pub severity: Severity,
    pub title: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub correlation_id: Option<String>,
    pub labels: HashMap<String, String>,
}

impl Alert {
    pub fn new(source: &str, severity: Severity, title: &str, message: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            source: source.to_string(),
            severity,
            title: title.to_string(),
            message: message.to_string(),
            timestamp: Utc::now(),
            correlation_id: None,
            labels: HashMap::new(),
        }
    }

    pub fn with_correlation(mut self, id: &str) -> Self {
        self.correlation_id = Some(id.to_string());
        self
    }

    pub fn with_label(mut self, key: &str, value: &str) -> Self {
        self.labels.insert(key.to_string(), value.to_string());
        self
    }
}

/// Severity level in AlertManager webhook payload format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl From<Severity> for AlertSeverity {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Info => AlertSeverity::Info,
            Severity::Notice => AlertSeverity::Info,
            Severity::Warning => AlertSeverity::Warning,
            Severity::Error => AlertSeverity::Error,
            Severity::Critical => AlertSeverity::Critical,
        }
    }
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertSeverity::Info => write!(f, "info"),
            AlertSeverity::Warning => write!(f, "warning"),
            AlertSeverity::Error => write!(f, "error"),
            AlertSeverity::Critical => write!(f, "critical"),
        }
    }
}

/// An alert in AlertManager webhook format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertManagerAlert {
    /// Unique alert identifier.
    pub id: Uuid,
    /// Alert labels (required by AlertManager).
    pub labels: HashMap<String, String>,
    /// Alert annotations for human-readable context.
    pub annotations: HashMap<String, String>,
    /// Severity level.
    pub severity: AlertSeverity,
    /// Source shim name.
    pub source: String,
    /// Correlation ID for distributed tracing.
    pub correlation_id: Option<Uuid>,
    /// When the alert was created.
    pub timestamp: String,
    /// Source event type.
    pub event_type: String,
}

/// AlertManager webhook payload (AlertManager-compatible format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Alert group name.
    pub group: String,
    /// List of alerts in this group.
    pub alerts: Vec<AlertManagerAlert>,
    /// Group labels for routing.
    pub group_labels: HashMap<String, String>,
    /// Common labels across all alerts.
    pub common_labels: HashMap<String, String>,
    /// External URL for the AlertManager UI.
    pub external_url: String,
}

/// Configuration for an AlertManager webhook endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertManagerWebhook {
    /// Name of this webhook target.
    pub name: String,
    /// URL to POST the webhook payload.
    pub url: String,
    /// Minimum severity to forward (lower severities dropped).
    pub min_severity: AlertSeverity,
    /// Extra headers to include in the POST.
    pub headers: HashMap<String, String>,
    /// Optional group name override.
    #[serde(default)]
    pub group: Option<String>,
}

impl AlertManagerWebhook {
    /// Returns true if this webhook accepts the given severity.
    pub fn accepts(&self, severity: &AlertSeverity) -> bool {
        severity_rank(severity) >= severity_rank(&self.min_severity)
    }
}

/// AlertManager integration that subscribes to ShimBus and forwards
/// alertable events to configured webhook endpoints.
pub struct AlertManager {
    /// The ShimBus to subscribe to.
    bus: ShimBus,
    /// Configured webhook endpoints.
    webhooks: Vec<AlertManagerWebhook>,
    /// Minimum severity for global filtering (before per-webhook filtering).
    min_severity: Severity,
    /// External URL for AlertManager UI links.
    external_url: String,
    /// Deduplication window — ignore duplicate alerts within this window.
    dedup_window: Duration,
    /// Tracks last-seen timestamps per dedup key.
    dedup_cache: Arc<RwLock<HashMap<String, std::time::Instant>>>,
    /// Total alerts forwarded.
    alerts_forwarded: Arc<AtomicU64>,
    /// Total alerts dropped (below severity or deduplicated).
    alerts_dropped: Arc<AtomicU64>,
    /// Total webhook delivery failures.
    delivery_failures: Arc<AtomicU64>,
    /// Shutdown signal.
    shutdown_tx: Arc<RwLock<Option<watch::Sender<bool>>>>,
}

impl AlertManager {
    /// Create a new AlertManager.
    pub fn new(bus: ShimBus, webhooks: Vec<AlertManagerWebhook>) -> Self {
        Self {
            bus,
            webhooks,
            min_severity: Severity::Info,
            external_url: "http://localhost:9093".to_string(),
            dedup_window: Duration::from_secs(300),
            dedup_cache: Arc::new(RwLock::new(HashMap::new())),
            alerts_forwarded: Arc::new(AtomicU64::new(0)),
            alerts_dropped: Arc::new(AtomicU64::new(0)),
            delivery_failures: Arc::new(AtomicU64::new(0)),
            shutdown_tx: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the global minimum severity filter.
    pub fn with_min_severity(mut self, severity: Severity) -> Self {
        self.min_severity = severity;
        self
    }

    /// Set the external URL for AlertManager UI links.
    pub fn with_external_url(mut self, url: impl Into<String>) -> Self {
        self.external_url = url.into();
        self
    }

    /// Set the deduplication window.
    pub fn with_dedup_window(mut self, window: Duration) -> Self {
        self.dedup_window = window;
        self
    }

    /// Start the AlertManager event loop in a background task.
    pub fn start(self: &Arc<Self>) {
        let mut rx = self.bus.subscribe();
        let manager = Arc::clone(self);

        let (tx, mut shutdown_rx) = watch::channel(false);
        *self.shutdown_tx.write() = Some(tx);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        tracing::info!("alert-manager: shutdown received");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                manager.process_event(&event).await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("alert-manager: lagged by {} events", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    /// Stop the background event loop.
    pub fn stop(&self) {
        if let Some(tx) = self.shutdown_tx.write().take() {
            let _ = tx.send(true);
        }
    }

    /// Process a single ShimEvent: filter, dedup, and forward to webhooks.
    async fn process_event(&self, event: &ShimEvent) {
        // Filter: only alertable events at or above global min severity
        if !event.is_alertable() {
            self.alerts_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if severity_rank_event(&event.severity) < severity_rank_event(&self.min_severity) {
            self.alerts_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        // Build dedup key
        let dedup_key = format!(
            "{}:{}:{}",
            event.source,
            event.event_name(),
            event.severity_str()
        );

        // Check dedup cache
        {
            let cache = self.dedup_cache.read();
            if let Some(last_seen) = cache.get(&dedup_key) {
                if last_seen.elapsed() < self.dedup_window {
                    self.alerts_dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }

        // Record in dedup cache
        {
            let mut cache = self.dedup_cache.write();
            cache.insert(dedup_key, std::time::Instant::now());
        }

        // Build AlertManager alert
        let alert = self.event_to_alert(event);

        // Forward to webhooks that accept this severity
        let matching: Vec<&AlertManagerWebhook> = self
            .webhooks
            .iter()
            .filter(|w| w.accepts(&alert.severity))
            .collect();

        if matching.is_empty() {
            self.alerts_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        for webhook in &matching {
            let payload = self.build_webhook_payload(alert.clone(), webhook);
            match self.send_webhook(webhook, &payload).await {
                Ok(()) => {
                    self.alerts_forwarded.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    self.delivery_failures.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        webhook = %webhook.name,
                        error = %e,
                        "alert-manager: webhook delivery failed"
                    );
                }
            }
        }
    }

    /// Convert a ShimEvent into an AlertManagerAlert.
    fn event_to_alert(&self, event: &ShimEvent) -> AlertManagerAlert {
        let mut labels = HashMap::new();
        labels.insert("source".to_string(), event.source.clone());
        labels.insert("severity".to_string(), event.severity_str().to_string());
        labels.insert("event_type".to_string(), event.event_name().to_string());

        let mut annotations = HashMap::new();
        annotations.insert(
            "summary".to_string(),
            format!("[{}] {}", event.severity_str(), event.event_name()),
        );
        annotations.insert("description".to_string(), format!("{:?}", event.event));

        AlertManagerAlert {
            id: event.id,
            labels,
            annotations,
            severity: AlertSeverity::from(event.severity),
            source: event.source.clone(),
            correlation_id: event.correlation_id,
            timestamp: event.timestamp.to_rfc3339(),
            event_type: event.event_name().to_string(),
        }
    }

    /// Build an AlertManager-compatible webhook payload.
    fn build_webhook_payload(
        &self,
        alert: AlertManagerAlert,
        webhook: &AlertManagerWebhook,
    ) -> WebhookPayload {
        let group = webhook
            .group
            .clone()
            .unwrap_or_else(|| "shim-alerts".to_string());

        let mut group_labels = HashMap::new();
        group_labels.insert("group".to_string(), group.clone());

        let mut common_labels = HashMap::new();
        common_labels.insert("source".to_string(), alert.source.clone());
        common_labels.insert("severity".to_string(), alert.severity.to_string());

        WebhookPayload {
            group,
            alerts: vec![alert],
            group_labels,
            common_labels,
            external_url: self.external_url.clone(),
        }
    }

    /// Send a webhook payload to a configured endpoint.
    async fn send_webhook(
        &self,
        webhook: &AlertManagerWebhook,
        payload: &WebhookPayload,
    ) -> crate::error::Result<()> {
        let body = serde_json::to_vec(payload).map_err(crate::error::Error::Serialization)?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| crate::error::Error::Connection(format!("http client: {}", e)))?;

        let mut req = client
            .post(&webhook.url)
            .header("Content-Type", "application/json");

        for (key, value) in &webhook.headers {
            req = req.header(key.as_str(), value.as_str());
        }

        req.body(body)
            .send()
            .await
            .map_err(|e| crate::error::Error::Connection(format!("webhook: {}", e)))?;

        Ok(())
    }

    /// Manually convert a ShimEvent to a WebhookPayload (for testing / external use).
    pub fn convert_event(&self, event: &ShimEvent) -> WebhookPayload {
        let alert = self.event_to_alert(event);
        let webhook = self
            .webhooks
            .first()
            .cloned()
            .unwrap_or(AlertManagerWebhook {
                name: "default".to_string(),
                url: "http://localhost:9093".to_string(),
                min_severity: AlertSeverity::Info,
                headers: HashMap::new(),
                group: None,
            });
        self.build_webhook_payload(alert, &webhook)
    }

    /// Total alerts forwarded successfully.
    pub fn alerts_forwarded(&self) -> u64 {
        self.alerts_forwarded.load(Ordering::Relaxed)
    }

    /// Total alerts dropped (below severity or deduplicated).
    pub fn alerts_dropped(&self) -> u64 {
        self.alerts_dropped.load(Ordering::Relaxed)
    }

    /// Total webhook delivery failures.
    pub fn delivery_failures(&self) -> u64 {
        self.delivery_failures.load(Ordering::Relaxed)
    }

    /// Clean up expired dedup cache entries.
    pub fn cleanup_dedup_cache(&self) {
        let mut cache = self.dedup_cache.write();
        cache.retain(|_, ts| ts.elapsed() < self.dedup_window);
    }
}

fn severity_rank(s: &AlertSeverity) -> u8 {
    match s {
        AlertSeverity::Info => 0,
        AlertSeverity::Warning => 1,
        AlertSeverity::Error => 2,
        AlertSeverity::Critical => 3,
    }
}

fn severity_rank_event(s: &Severity) -> u8 {
    match s {
        Severity::Info => 0,
        Severity::Notice => 1,
        Severity::Warning => 2,
        Severity::Error => 3,
        Severity::Critical => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventType, Severity, ShimEvent};

    fn make_event(source: &str, event_type: EventType, severity: Severity) -> ShimEvent {
        ShimEvent::new(source, event_type, severity)
    }

    #[test]
    fn test_alert_severity_from_shim_severity() {
        assert_eq!(AlertSeverity::from(Severity::Info), AlertSeverity::Info);
        assert_eq!(AlertSeverity::from(Severity::Notice), AlertSeverity::Info);
        assert_eq!(
            AlertSeverity::from(Severity::Warning),
            AlertSeverity::Warning
        );
        assert_eq!(AlertSeverity::from(Severity::Error), AlertSeverity::Error);
        assert_eq!(
            AlertSeverity::from(Severity::Critical),
            AlertSeverity::Critical
        );
    }

    #[test]
    fn test_alert_severity_display() {
        assert_eq!(AlertSeverity::Info.to_string(), "info");
        assert_eq!(AlertSeverity::Warning.to_string(), "warning");
        assert_eq!(AlertSeverity::Error.to_string(), "error");
        assert_eq!(AlertSeverity::Critical.to_string(), "critical");
    }

    #[test]
    fn test_webhook_accepts_severity() {
        let wh = AlertManagerWebhook {
            name: "pager".into(),
            url: "http://localhost".into(),
            min_severity: AlertSeverity::Warning,
            headers: HashMap::new(),
            group: None,
        };
        assert!(!wh.accepts(&AlertSeverity::Info));
        assert!(wh.accepts(&AlertSeverity::Warning));
        assert!(wh.accepts(&AlertSeverity::Error));
        assert!(wh.accepts(&AlertSeverity::Critical));
    }

    #[test]
    fn test_event_to_alert() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![]);

        let evt = make_event(
            "backup-shim",
            EventType::BackupFailed {
                name: "daily".into(),
                reason: "timeout".into(),
            },
            Severity::Error,
        );

        let alert = am.event_to_alert(&evt);
        assert_eq!(alert.source, "backup-shim");
        assert_eq!(alert.severity, AlertSeverity::Error);
        assert_eq!(alert.event_type, "backup_failed");
        assert_eq!(alert.id, evt.id);
        assert!(alert.annotations.contains_key("summary"));
        assert!(alert.annotations.contains_key("description"));
        assert!(alert.labels.contains_key("source"));
        assert!(alert.labels.contains_key("severity"));
        assert!(alert.labels.contains_key("event_type"));
    }

    #[test]
    fn test_event_to_alert_with_correlation_id() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![]);

        let correlation = Uuid::new_v4();
        let evt = make_event(
            "tls-shim",
            EventType::TlsCertExpiring {
                cert_path: "/etc/tls/cert.pem".into(),
                days_remaining: 3,
            },
            Severity::Warning,
        )
        .with_correlation(correlation);

        let alert = am.event_to_alert(&evt);
        assert_eq!(alert.correlation_id, Some(correlation));
    }

    #[test]
    fn test_build_webhook_payload() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![]);

        let alert = AlertManagerAlert {
            id: Uuid::new_v4(),
            labels: HashMap::new(),
            annotations: HashMap::new(),
            severity: AlertSeverity::Warning,
            source: "test-shim".into(),
            correlation_id: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
            event_type: "test".into(),
        };

        let webhook = AlertManagerWebhook {
            name: "test".into(),
            url: "http://localhost".into(),
            min_severity: AlertSeverity::Info,
            headers: HashMap::new(),
            group: Some("test-group".into()),
        };

        let payload = am.build_webhook_payload(alert, &webhook);
        assert_eq!(payload.group, "test-group");
        assert_eq!(payload.alerts.len(), 1);
        assert!(payload.group_labels.contains_key("group"));
        assert!(payload.common_labels.contains_key("source"));
    }

    #[test]
    fn test_build_webhook_payload_default_group() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![]);

        let alert = AlertManagerAlert {
            id: Uuid::new_v4(),
            labels: HashMap::new(),
            annotations: HashMap::new(),
            severity: AlertSeverity::Info,
            source: "test".into(),
            correlation_id: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
            event_type: "test".into(),
        };

        let webhook = AlertManagerWebhook {
            name: "test".into(),
            url: "http://localhost".into(),
            min_severity: AlertSeverity::Info,
            headers: HashMap::new(),
            group: None,
        };

        let payload = am.build_webhook_payload(alert, &webhook);
        assert_eq!(payload.group, "shim-alerts");
    }

    #[test]
    fn test_severity_rank_event_ordering() {
        assert!(severity_rank_event(&Severity::Info) < severity_rank_event(&Severity::Notice));
        assert!(severity_rank_event(&Severity::Notice) < severity_rank_event(&Severity::Warning));
        assert!(severity_rank_event(&Severity::Warning) < severity_rank_event(&Severity::Error));
        assert!(severity_rank_event(&Severity::Error) < severity_rank_event(&Severity::Critical));
    }

    #[test]
    fn test_alert_manager_counters() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![]);
        assert_eq!(am.alerts_forwarded(), 0);
        assert_eq!(am.alerts_dropped(), 0);
        assert_eq!(am.delivery_failures(), 0);
    }

    #[test]
    fn test_alert_manager_builder() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![])
            .with_min_severity(Severity::Warning)
            .with_external_url("http://alertmanager:9093")
            .with_dedup_window(Duration::from_secs(60));
        assert_eq!(am.min_severity, Severity::Warning);
        assert_eq!(am.external_url, "http://alertmanager:9093");
        assert_eq!(am.dedup_window, Duration::from_secs(60));
    }

    #[test]
    fn test_convert_event() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![]);

        let evt = make_event(
            "backup-shim",
            EventType::BackupFailed {
                name: "daily".into(),
                reason: "timeout".into(),
            },
            Severity::Error,
        );

        let payload = am.convert_event(&evt);
        assert_eq!(payload.alerts.len(), 1);
        assert_eq!(payload.alerts[0].source, "backup-shim");
    }

    #[test]
    fn test_dedup_key_uniqueness() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![]);

        let evt1 = make_event(
            "backup-shim",
            EventType::BackupFailed {
                name: "daily".into(),
                reason: "timeout".into(),
            },
            Severity::Error,
        );
        let evt2 = make_event(
            "backup-shim",
            EventType::BackupFailed {
                name: "weekly".into(),
                reason: "timeout".into(),
            },
            Severity::Error,
        );

        let alert1 = am.event_to_alert(&evt1);
        let alert2 = am.event_to_alert(&evt2);

        // Different event names → different dedup keys
        let key1 = format!(
            "{}:{}:{}",
            alert1.source, alert1.event_type, alert1.severity
        );
        let _key2 = format!(
            "{}:{}:{}",
            alert2.source, alert2.event_type, alert2.severity
        );
        // Same event_type but different underlying event — but dedup key
        // only uses event_name which is the same for same EventType variant
        // Different source or severity would differ
        let evt3 = make_event(
            "tls-shim",
            EventType::BackupFailed {
                name: "daily".into(),
                reason: "timeout".into(),
            },
            Severity::Critical,
        );
        let alert3 = am.event_to_alert(&evt3);
        let key3 = format!(
            "{}:{}:{}",
            alert3.source, alert3.event_type, alert3.severity
        );
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_webhook_payload_serialization_roundtrip() {
        let bus = ShimBus::new();
        let am = AlertManager::new(bus, vec![]);

        let evt = make_event(
            "backup-shim",
            EventType::BackupFailed {
                name: "daily".into(),
                reason: "timeout".into(),
            },
            Severity::Error,
        );

        let payload = am.convert_event(&evt);
        let json = serde_json::to_string(&payload).unwrap();
        let deser: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.alerts.len(), 1);
        assert_eq!(deser.alerts[0].source, "backup-shim");
    }

    #[test]
    fn test_alert_manager_severity_serialization() {
        let sev = AlertSeverity::Critical;
        let json = serde_json::to_string(&sev).unwrap();
        assert_eq!(json, "\"critical\"");
        let deser: AlertSeverity = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, AlertSeverity::Critical);
    }

    #[test]
    fn test_severity_rank_comparison() {
        assert!(severity_rank(&AlertSeverity::Info) < severity_rank(&AlertSeverity::Warning));
        assert!(severity_rank(&AlertSeverity::Warning) < severity_rank(&AlertSeverity::Error));
        assert!(severity_rank(&AlertSeverity::Error) < severity_rank(&AlertSeverity::Critical));
    }
}
