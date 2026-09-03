//! Alerting module integration tests.
//!
//! Tests Alert creation, serialization roundtrip, and AlertManager webhook config.
//!
//! Run with: cargo test -p evergreen-shims-integration --test alerting_test

use std::collections::HashMap;

use shim_core::alerting::{Alert, AlertManager, AlertManagerWebhook, AlertSeverity};
use shim_core::Severity;
use shim_core::ShimBus;

#[test]
fn test_alert_creation() {
    let alert = Alert::new(
        "test-shim",
        Severity::Warning,
        "Test Alert",
        "This is a test",
    );
    assert_eq!(alert.source, "test-shim");
    assert_eq!(alert.severity, Severity::Warning);
    assert!(!alert.id.is_empty());
}

#[test]
fn test_alert_with_correlation() {
    let alert = Alert::new("test-shim", Severity::Error, "Error", "Something broke")
        .with_correlation("req-123");
    assert_eq!(alert.correlation_id, Some("req-123".to_string()));
}

#[test]
fn test_alert_with_label() {
    let alert = Alert::new("test-shim", Severity::Info, "Info", "Details")
        .with_label("env", "production")
        .with_label("region", "us-east-1");
    assert_eq!(alert.labels.get("env"), Some(&"production".to_string()));
    assert_eq!(alert.labels.get("region"), Some(&"us-east-1".to_string()));
}

#[test]
fn test_alert_roundtrip() {
    let alert = Alert::new("test-shim", Severity::Critical, "Test", "Message")
        .with_correlation("req-456")
        .with_label("env", "production");

    let json = serde_json::to_string(&alert).unwrap();
    let deserialized: Alert = serde_json::from_str(&json).unwrap();
    assert_eq!(alert.id, deserialized.id);
    assert_eq!(alert.severity, deserialized.severity);
    assert_eq!(alert.correlation_id, deserialized.correlation_id);
    assert_eq!(alert.source, deserialized.source);
    assert_eq!(alert.title, deserialized.title);
    assert_eq!(alert.message, deserialized.message);
}

#[test]
fn test_alert_manager_builder() {
    let bus = ShimBus::new();
    let manager = AlertManager::new(bus, vec![])
        .with_min_severity(Severity::Warning)
        .with_external_url("http://alertmanager:9093");
    assert_eq!(manager.alerts_forwarded(), 0);
    assert_eq!(manager.alerts_dropped(), 0);
}

#[test]
fn test_alert_manager_webhook_config() {
    let webhook = AlertManagerWebhook {
        name: "pager".into(),
        url: "http://localhost:9093/api/v1/alerts".into(),
        min_severity: AlertSeverity::Warning,
        headers: HashMap::new(),
        group: Some("critical-alerts".into()),
    };

    assert!(webhook.accepts(&AlertSeverity::Warning));
    assert!(webhook.accepts(&AlertSeverity::Error));
    assert!(webhook.accepts(&AlertSeverity::Critical));
    assert!(!webhook.accepts(&AlertSeverity::Info));
}

#[test]
fn test_alert_manager_with_webhook() {
    let bus = ShimBus::new();
    let webhook = AlertManagerWebhook {
        name: "test".into(),
        url: "http://localhost:9093/api/v1/alerts".into(),
        min_severity: AlertSeverity::Info,
        headers: HashMap::new(),
        group: None,
    };
    let manager = AlertManager::new(bus, vec![webhook]);
    assert_eq!(manager.alerts_forwarded(), 0);
}
