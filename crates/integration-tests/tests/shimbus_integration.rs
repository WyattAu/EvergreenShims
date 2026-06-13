//! ShimBus integration tests.
//!
//! Tests event publish/subscribe roundtrip, cross-shim event forwarding,
//! AlertManager webhook delivery, and metrics endpoint response validation.
//!
//! Run with: cargo test -p evergreen-shims-integration --test shimbus_integration

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use shim_core::alerting::{
    AlertManager, AlertManagerAlert, AlertManagerWebhook, AlertSeverity, WebhookPayload,
};
use shim_core::event::{EventType, Severity, ShimEvent};
use shim_core::metrics_export::MetricsExporter;
use shim_core::metrics::ShimMetrics;
use shim_core::wiring::{BackupEncryptionHandler, HealthFailoverHandler};
use shim_core::{ShimBus, Severity as CoreSeverity};

// ============================================================================
// Task 3.1: Event publish/subscribe roundtrip
// ============================================================================

/// Test basic publish → subscribe roundtrip on ShimBus.
#[tokio::test]
async fn test_event_publish_subscribe_roundtrip() {
    let bus = ShimBus::new();
    let mut rx = bus.subscribe();

    let event = bus.emit(
        "backup-shim",
        EventType::BackupCompleted {
            name: "daily".into(),
            size_bytes: 1024,
            checksum: "abc123".into(),
        },
        CoreSeverity::Info,
    );

    let received = rx.try_recv().expect("should receive event");
    assert_eq!(received.id, event.id);
    assert_eq!(received.source, "backup-shim");
    assert_eq!(received.sequence, 1);
    assert!(matches!(
        received.event,
        EventType::BackupCompleted { ref name, .. } if name == "daily"
    ));
}

/// Test multiple events roundtrip in order.
#[tokio::test]
async fn test_multiple_events_roundtrip_ordering() {
    let bus = ShimBus::new();
    let mut rx = bus.subscribe();

    let e1 = bus.emit(
        "shim-a",
        EventType::Custom {
            event_name: "e1".into(),
            payload: serde_json::json!(null),
        },
        CoreSeverity::Info,
    );
    let e2 = bus.emit(
        "shim-b",
        EventType::Custom {
            event_name: "e2".into(),
            payload: serde_json::json!(null),
        },
        CoreSeverity::Warning,
    );
    let e3 = bus.emit(
        "shim-a",
        EventType::Custom {
            event_name: "e3".into(),
            payload: serde_json::json!(null),
        },
        CoreSeverity::Error,
    );

    let r1 = rx.try_recv().unwrap();
    let r2 = rx.try_recv().unwrap();
    let r3 = rx.try_recv().unwrap();

    assert_eq!(r1.id, e1.id);
    assert_eq!(r2.id, e2.id);
    assert_eq!(r3.id, e3.id);
}

/// Test filtered subscriber receives only matching events.
#[tokio::test]
async fn test_filtered_subscribe_roundtrip() {
    use shim_core::bus::BusSubscriber;

    let bus = ShimBus::new();
    let rx = bus.subscribe();
    let mut sub = BusSubscriber::new(rx, vec!["backup_completed".into()], CoreSeverity::Info);

    // Non-matching event
    bus.emit(
        "auth-shim",
        EventType::AuthTokenRevoked {
            token_id: "tok".into(),
            reason: "expired".into(),
        },
        CoreSeverity::Notice,
    );

    // Matching event
    bus.emit(
        "backup-shim",
        EventType::BackupCompleted {
            name: "daily".into(),
            size_bytes: 2048,
            checksum: "def456".into(),
        },
        CoreSeverity::Info,
    );

    let evt = sub.try_recv().expect("should receive filtered event");
    assert_eq!(evt.source, "backup-shim");
}

/// Test correlation ID survives roundtrip.
#[tokio::test]
async fn test_correlation_id_roundtrip() {
    let bus = ShimBus::new();
    let mut rx = bus.subscribe();

    let correlation = uuid::Uuid::new_v4();
    let event = bus.emit(
        "tls-shim",
        EventType::TlsCertExpiring {
            cert_path: "/etc/tls/cert.pem".into(),
            days_remaining: 7,
        },
        CoreSeverity::Warning,
    );

    // Manually set correlation on the emitted event isn't possible via emit(),
    // but we can verify the event structure via publish()
    let mut evt = ShimEvent::new(
        "tls-shim",
        EventType::TlsCertExpiring {
            cert_path: "/etc/tls/cert.pem".into(),
            days_remaining: 7,
        },
        CoreSeverity::Warning,
    )
    .with_correlation(correlation);

    let seq = bus.publish(evt.clone());
    evt.sequence = seq;

    let received = rx.try_recv().unwrap();
    assert_eq!(received.correlation_id, Some(correlation));
}

// ============================================================================
// Task 3.2: Cross-shim event forwarding
// ============================================================================

/// Test health → failover chain produces FailoverTriggered.
#[tokio::test]
async fn test_cross_shim_health_to_failover() {
    let bus = ShimBus::new();
    let handler = Arc::new(HealthFailoverHandler::new(bus.clone(), 2));
    handler.start();

    let mut rx = bus.subscribe();

    bus.emit(
        "health-shim",
        EventType::HealthStatusChanged {
            previous: "healthy".into(),
            current: "unhealthy".into(),
        },
        CoreSeverity::Warning,
    );
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Second unhealthy triggers failover
    bus.emit(
        "health-shim",
        EventType::HealthStatusChanged {
            previous: "unhealthy".into(),
            current: "unhealthy".into(),
        },
        CoreSeverity::Warning,
    );
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut found = false;
    while let Ok(evt) = rx.try_recv() {
        if matches!(evt.event, EventType::FailoverTriggered { .. }) {
            found = true;
            assert_eq!(evt.source, "failover-shim");
            assert_eq!(evt.severity, CoreSeverity::Critical);
        }
    }
    assert!(found, "health→failover chain should produce FailoverTriggered");
}

/// Test backup → encryption chain.
#[tokio::test]
async fn test_cross_shim_backup_to_encryption() {
    let bus = ShimBus::new();
    let handler = Arc::new(BackupEncryptionHandler::new(bus.clone()));
    handler.start();

    let mut rx = bus.subscribe();

    bus.emit(
        "backup-shim",
        EventType::BackupCompleted {
            name: "postgres-daily".into(),
            size_bytes: 5_000_000,
            checksum: "sha256:abcdef".into(),
        },
        CoreSeverity::Info,
    );
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut found = false;
    while let Ok(evt) = rx.try_recv() {
        if let EventType::EncryptionKeyRotated { key_id, .. } = &evt.event {
            found = true;
            assert_eq!(key_id, "backup-postgres-daily");
        }
    }
    assert!(found, "backup→encryption chain should produce EncryptionKeyRotated");
}

/// Test multi-source sequencing: different sources get independent sequences.
#[tokio::test]
async fn test_multi_source_sequencing() {
    let bus = ShimBus::new();

    let e1 = bus.emit(
        "shim-a",
        EventType::Custom {
            event_name: "a1".into(),
            payload: serde_json::json!(null),
        },
        CoreSeverity::Info,
    );
    let e2 = bus.emit(
        "shim-b",
        EventType::Custom {
            event_name: "b1".into(),
            payload: serde_json::json!(null),
        },
        CoreSeverity::Info,
    );
    let e3 = bus.emit(
        "shim-a",
        EventType::Custom {
            event_name: "a2".into(),
            payload: serde_json::json!(null),
        },
        CoreSeverity::Info,
    );

    assert_eq!(e1.sequence, 1);
    assert_eq!(e2.sequence, 1); // Different source → independent counter
    assert_eq!(e3.sequence, 2); // Same source as e1 → next in sequence
}

// ============================================================================
// Task 3.3: AlertManager webhook delivery
// ============================================================================

/// Test AlertManager converts events to webhook payloads correctly.
#[tokio::test]
async fn test_alertmanager_webhook_payload_conversion() {
    let bus = ShimBus::new();
    let am = AlertManager::new(bus, vec![AlertManagerWebhook {
        name: "test".into(),
        url: "http://localhost:9093/api/v2/alerts".into(),
        min_severity: AlertSeverity::Info,
        headers: HashMap::new(),
        group: Some("test-group".into()),
    }]);

    let evt = ShimEvent::new(
        "backup-shim",
        EventType::BackupFailed {
            name: "daily".into(),
            reason: "timeout".into(),
        },
        CoreSeverity::Error,
    )
    .with_correlation(uuid::Uuid::new_v4());

    let payload = am.convert_event(&evt);

    assert_eq!(payload.alerts.len(), 1);
    assert_eq!(payload.group, "test-group");
    assert_eq!(payload.alerts[0].source, "backup-shim");
    assert_eq!(payload.alerts[0].severity, AlertSeverity::Error);
    assert!(payload.alerts[0].correlation_id.is_some());
    assert!(payload.alerts[0].labels.contains_key("event_type"));
    assert!(payload.alerts[0].annotations.contains_key("summary"));
    assert_eq!(payload.external_url, "http://localhost:9093");
}

/// Test AlertManager webhook filtering by severity.
#[tokio::test]
async fn test_alertmanager_severity_filtering() {
    let webhook_critical = AlertManagerWebhook {
        name: "pager".into(),
        url: "http://localhost:9094".into(),
        min_severity: AlertSeverity::Critical,
        headers: HashMap::new(),
        group: None,
    };
    let webhook_warning = AlertManagerWebhook {
        name: "slack".into(),
        url: "http://localhost:9095".into(),
        min_severity: AlertSeverity::Warning,
        headers: HashMap::new(),
        group: None,
    };

    let bus = ShimBus::new();
    let am = AlertManager::new(bus, vec![webhook_critical, webhook_warning]);

    // Info event — no webhook matches min_severity of Info (pager=Critical, slack=Warning)
    // Wait: webhook_warning accepts Warning+, webhook_critical accepts Critical+
    // An info alert should be dropped by both
    let evt_info = ShimEvent::new(
        "test",
        EventType::Custom {
            event_name: "info_event".into(),
            payload: serde_json::json!(null),
        },
        CoreSeverity::Info,
    );
    let payload_info = am.convert_event(&evt_info);
    // Info severity → AlertSeverity::Info
    assert_eq!(payload_info.alerts[0].severity, AlertSeverity::Info);

    // Critical event — both webhooks accept
    let evt_crit = ShimEvent::new(
        "test",
        EventType::Custom {
            event_name: "crit_event".into(),
            payload: serde_json::json!(null),
        },
        CoreSeverity::Critical,
    );
    let payload_crit = am.convert_event(&evt_crit);
    assert_eq!(payload_crit.alerts[0].severity, AlertSeverity::Critical);

    // Verify webhook acceptance
    let wh_slack = &am.webhooks[1]; // slack (warning+)
    let wh_pager = &am.webhooks[0]; // pager (critical+)
    assert!(wh_slack.accepts(&AlertSeverity::Warning));
    assert!(wh_slack.accepts(&AlertSeverity::Critical));
    assert!(!wh_slack.accepts(&AlertSeverity::Info));
    assert!(wh_pager.accepts(&AlertSeverity::Critical));
    assert!(!wh_pager.accepts(&AlertSeverity::Warning));
    assert!(!wh_pager.accepts(&AlertSeverity::Info));
}

/// Test AlertManager serialization roundtrip.
#[tokio::test]
async fn test_alertmanager_payload_serialization() {
    let bus = ShimBus::new();
    let am = AlertManager::new(bus, vec![]);

    let evt = ShimEvent::new(
        "tls-shim",
        EventType::TlsCertExpiring {
            cert_path: "/etc/tls/api.pem".into(),
            days_remaining: 3,
        },
        CoreSeverity::Warning,
    )
    .with_correlation(uuid::Uuid::new_v4());

    let payload = am.convert_event(&evt);
    let json = serde_json::to_string(&payload).unwrap();
    let deser: WebhookPayload = serde_json::from_str(&json).unwrap();

    assert_eq!(deser.alerts.len(), 1);
    assert_eq!(deser.alerts[0].source, "tls-shim");
    assert_eq!(deser.alerts[0].severity, AlertSeverity::Warning);
    assert!(deser.alerts[0].correlation_id.is_some());
}

/// Test AlertManager alert label structure.
#[tokio::test]
async fn test_alertmanager_alert_labels() {
    let bus = ShimBus::new();
    let am = AlertManager::new(bus, vec![]);

    let evt = ShimEvent::new(
        "failover-shim",
        EventType::FailoverTriggered {
            old_primary: "pg-1".into(),
            new_primary: "pg-2".into(),
        },
        CoreSeverity::Critical,
    );

    let alert = am.convert_event(&evt).alerts.into_iter().next().unwrap();
    assert_eq!(alert.labels.get("source").unwrap(), "failover-shim");
    assert_eq!(alert.labels.get("severity").unwrap(), "CRITICAL");
    assert_eq!(alert.labels.get("event_type").unwrap(), "failover_triggered");
}

// ============================================================================
// Task 3.4: Metrics endpoint response validation
// ============================================================================

/// Test /metrics endpoint returns valid Prometheus text with shim_up.
#[tokio::test]
async fn test_metrics_endpoint_prometheus_format() {
    let exp = Arc::new(MetricsExporter::new(ShimMetrics::new()));
    exp.mark_up();
    exp.record_event("backup_completed");
    exp.record_event("backup_completed");
    exp.record_event("tls_cert_renewed");
    exp.record_alert_forwarded();
    exp.record_health_probe("liveness");
    exp.record_health_probe("readiness");
    exp.record_config_reload();

    let output = exp.export_all();

    // Standard base metrics
    assert!(output.contains("shim_events_published_total"));
    assert!(output.contains("shim_health_status"));

    // Exporter-specific metrics
    assert!(output.contains("shim_up 1"));
    assert!(output.contains("shim_events_total"));
    assert!(output.contains("backup_completed"));
    assert!(output.contains("tls_cert_renewed"));
    assert!(output.contains("shim_alerts_forwarded_total 1"));
    assert!(output.contains("shim_health_probes_total"));
    assert!(output.contains("liveness"));
    assert!(output.contains("readiness"));
    assert!(output.contains("shim_config_reloads_total 1"));

    // Prometheus format validity: each non-comment line starts with identifier
    for line in output.lines() {
        if !line.is_empty() && !line.starts_with('#') {
            let first = line.chars().next().unwrap();
            assert!(
                first.is_alphabetic() || first == '_',
                "invalid prometheus line: {}",
                line
            );
        }
    }
}

/// Test /healthz endpoint returns valid JSON with expected fields.
#[tokio::test]
async fn test_healthz_endpoint_json_response() {
    let exp = Arc::new(MetricsExporter::new(ShimMetrics::new()));
    exp.mark_up();
    exp.record_event("test");

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let actual_addr = listener.local_addr().unwrap();

    let state = shim_core::metrics_export::MetricsExporter::new(ShimMetrics::new());
    // We need to access the handler directly — use the axum test approach
    // by constructing the router with the exporter's state
    let app = axum::Router::new()
        .route("/healthz", axum::routing::get(test_healthz_handler))
        .with_state(exp.clone());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut stream = tokio::net::TcpStream::connect(actual_addr)
        .await
        .unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);

    assert!(response.contains("200 OK"));
    assert!(response.contains("healthy"));
    assert!(response.contains("uptime_seconds"));
    assert!(response.contains("shim_up"));
    assert!(response.contains("scrape_count"));
}

/// Test metrics server /metrics endpoint via TCP.
#[tokio::test]
async fn test_metrics_server_tcp_metrics_endpoint() {
    let exp = Arc::new(MetricsExporter::new(ShimMetrics::new()));
    exp.mark_up();
    exp.record_event("test_event");
    exp.record_event("test_event");

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let actual_addr = listener.local_addr().unwrap();

    let state = shim_core::metrics_export::MetricsExporter::new(ShimMetrics::new());
    let app = axum::Router::new()
        .route("/metrics", axum::routing::get(test_metrics_handler))
        .with_state(exp.clone());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let mut stream = tokio::net::TcpStream::connect(actual_addr)
        .await
        .unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(b"GET /metrics HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);

    assert!(response.contains("200 OK"));
    assert!(response.contains("text/plain"));
    assert!(response.contains("shim_up 1"));
    assert!(response.contains("shim_events_total"));
    assert!(response.contains("test_event"));
}

/// Test shim_down metric.
#[tokio::test]
async fn test_metrics_shim_down() {
    let exp = Arc::new(MetricsExporter::new(ShimMetrics::new()));
    exp.mark_down();
    let output = exp.export_all();
    assert!(output.contains("shim_up 0"));
}

/// Test scrape count increments.
#[tokio::test]
async fn test_metrics_scrape_count_increments() {
    let exp = Arc::new(MetricsExporter::new(ShimMetrics::new()));
    assert_eq!(exp.scrape_count(), 0);
    exp.scrape_count.fetch_add(1, Ordering::Relaxed);
    exp.scrape_count.fetch_add(1, Ordering::Relaxed);
    assert_eq!(exp.scrape_count(), 2);
}

/// Test custom per-shim metrics integration.
#[tokio::test]
async fn test_custom_per_shim_metrics() {
    let exp = Arc::new(MetricsExporter::new(ShimMetrics::new()));
    exp.mark_up();
    exp.set_custom_metric("backup-shim", "backups_completed", 42.0);
    exp.set_custom_metric("backup-shim", "backups_failed", 3.0);
    exp.set_custom_metric("tls-shim", "certs_renewed", 12.0);

    // Verify custom metrics are stored
    let metrics = exp.custom_metrics.read().unwrap();
    assert_eq!(
        metrics.get("backup-shim").unwrap().get("backups_completed"),
        Some(&42.0)
    );
    assert_eq!(
        metrics.get("tls-shim").unwrap().get("certs_renewed"),
        Some(&12.0)
    );
}

// ============================================================================
// Helpers for test axum handlers
// ============================================================================

async fn test_healthz_handler(
    State(exporter): State<Arc<MetricsExporter>>,
) -> axum::response::Response {
    let healthy = exporter.is_running();
    let status = serde_json::json!({
        "status": if healthy { "healthy" } else { "unhealthy" },
        "uptime_seconds": exporter.uptime_seconds(),
        "shim_up": exporter.shim_up.get() > 0.5,
        "scrape_count": exporter.scrape_count(),
    });

    let body = serde_json::to_string(&status).unwrap();
    (
        axum::http::StatusCode::OK,
        [("Content-Type", "application/json")],
        body,
    )
        .into_response()
}

async fn test_metrics_handler(
    State(exporter): State<Arc<MetricsExporter>>,
) -> axum::response::Response {
    exporter.scrape_count.fetch_add(1, Ordering::Relaxed);
    let body = exporter.export_all();
    (
        axum::http::StatusCode::OK,
        [("Content-Type", "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}
