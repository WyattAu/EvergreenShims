//! Cross-shim event system.
//!
//! Provides `ShimEvent` (typed event payload) and `EventType` (enum of all
//! cross-shim event variants). Events flow through `ShimBus` (in-process
//! broadcast) or optionally via Redis for multi-container deployments.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Severity level for shim events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational — normal operation.
    Info,
    /// Something noteworthy happened (rotation completed, backup finished).
    Notice,
    /// Potential issue — degraded performance, approaching threshold.
    Warning,
    /// Something failed — backup error, TLS cert expired, failover triggered.
    Error,
    /// Critical — system safety at risk, immediate attention required.
    Critical,
}

/// Strongly-typed event variants emitted by shims.
///
/// Each variant maps to a specific shim domain. The `source` field on
/// `ShimEvent` identifies which shim emitted the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventType {
    // ── Health / Failover ─────────────────────────────────────────────
    /// Health status changed (healthy → unhealthy or vice versa).
    HealthStatusChanged { previous: String, current: String },
    /// Failover was triggered — primary down, promoting replica.
    FailoverTriggered {
        old_primary: String,
        new_primary: String,
    },
    /// Failover completed — new primary is serving traffic.
    FailoverCompleted { promoted: String },

    // ── Backup / Encryption ───────────────────────────────────────────
    /// Backup started.
    BackupStarted { name: String },
    /// Backup completed successfully.
    BackupCompleted {
        name: String,
        size_bytes: u64,
        checksum: String,
    },
    /// Backup failed.
    BackupFailed { name: String, reason: String },
    /// Encryption key rotation completed.
    EncryptionKeyRotated { key_id: String, algorithm: String },

    // ── Replication / Migration ───────────────────────────────────────
    /// Replication lag exceeded threshold.
    ReplicationLagWarning { lag_ms: u64, threshold_ms: u64 },
    /// Migration started.
    MigrationStarted { version: String },
    /// Migration completed.
    MigrationCompleted { version: String },
    /// Migration failed.
    MigrationFailed { version: String, reason: String },

    // ── Audit / Compliance ────────────────────────────────────────────
    /// Audit event recorded (typically fan-in — all shims emit audit events).
    AuditRecorded {
        event_type: String,
        resource: String,
        action: String,
    },
    /// Compliance rule check completed.
    ComplianceCheckCompleted {
        standard: String,
        score: f64,
        violations: usize,
    },

    // ── TLS / Auth ────────────────────────────────────────────────────
    /// TLS certificate approaching expiry.
    TlsCertExpiring {
        cert_path: String,
        days_remaining: u32,
    },
    /// TLS certificate auto-renewed.
    TlsCertRenewed { cert_path: String },
    /// Auth token expired or was revoked.
    AuthTokenRevoked { token_id: String, reason: String },

    // ── Scheduler / Queue ─────────────────────────────────────────────
    /// Scheduled task fired.
    SchedulerTaskFired { task_name: String, schedule: String },
    /// Queue job failed (sent to DLQ).
    QueueJobFailed {
        job_id: String,
        queue: String,
        retries: u32,
    },

    // ── Cache / Proxy ─────────────────────────────────────────────────
    /// Cache hit rate dropped below threshold.
    CacheHitRateLow { hit_rate: f64, threshold: f64 },
    /// Circuit breaker state changed.
    CircuitBreakerTripped { service: String, state: String },

    // ── CDC / Sharding ────────────────────────────────────────────────
    /// CDC event batch committed.
    CdcBatchCommitted { table: String, event_count: u32 },
    /// Shard rebalancing started.
    ShardRebalanceStarted {
        from_shard: String,
        to_shard: String,
    },

    // ── Archival / Cost ───────────────────────────────────────────────
    /// Data moved between tiers (hot → warm → cold).
    ArchivalTierTransition {
        resource: String,
        from_tier: String,
        to_tier: String,
    },
    /// Budget threshold reached.
    CostBudgetAlert {
        budget_name: String,
        usage_percent: f64,
    },

    // ── Chaos ─────────────────────────────────────────────────────────
    /// Chaos experiment started.
    ChaosExperimentStarted { experiment: String },
    /// Chaos experiment completed.
    ChaosExperimentCompleted { experiment: String, result: String },

    // ── Generic ───────────────────────────────────────────────────────
    /// Custom event — fallback for domain-specific extensions.
    Custom {
        event_name: String,
        payload: serde_json::Value,
    },
}

/// A single event flowing through the ShimBus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShimEvent {
    /// Unique event ID.
    pub id: Uuid,
    /// Source shim name (e.g., "backup-shim", "tls-shim").
    pub source: String,
    /// Typed event payload.
    pub event: EventType,
    /// Event severity.
    pub severity: Severity,
    /// When the event was created.
    pub timestamp: DateTime<Utc>,
    /// Monotonically increasing sequence number per source.
    pub sequence: u64,
    /// Optional correlation ID for request tracing across shims.
    pub correlation_id: Option<Uuid>,
    /// Optional JSON payload for additional context.
    pub payload: Option<serde_json::Value>,
}

impl ShimEvent {
    /// Create a new event.
    pub fn new(source: impl Into<String>, event: EventType, severity: Severity) -> Self {
        Self {
            id: Uuid::new_v4(),
            source: source.into(),
            event,
            severity,
            timestamp: Utc::now(),
            sequence: 0,
            correlation_id: None,
            payload: None,
        }
    }

    /// Builder: set sequence number.
    pub fn with_sequence(mut self, seq: u64) -> Self {
        self.sequence = seq;
        self
    }

    /// Builder: set correlation ID.
    pub fn with_correlation(mut self, id: Uuid) -> Self {
        self.correlation_id = Some(id);
        self
    }

    /// Builder: set extra payload.
    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = Some(payload);
        self
    }

    /// Returns true if this event is at Warning severity or above.
    pub fn is_alertable(&self) -> bool {
        matches!(
            self.severity,
            Severity::Warning | Severity::Error | Severity::Critical
        )
    }

    /// Human-readable event name for logging.
    pub fn event_name(&self) -> &str {
        match &self.event {
            EventType::HealthStatusChanged { .. } => "health_status_changed",
            EventType::FailoverTriggered { .. } => "failover_triggered",
            EventType::FailoverCompleted { .. } => "failover_completed",
            EventType::BackupStarted { .. } => "backup_started",
            EventType::BackupCompleted { .. } => "backup_completed",
            EventType::BackupFailed { .. } => "backup_failed",
            EventType::EncryptionKeyRotated { .. } => "encryption_key_rotated",
            EventType::ReplicationLagWarning { .. } => "replication_lag_warning",
            EventType::MigrationStarted { .. } => "migration_started",
            EventType::MigrationCompleted { .. } => "migration_completed",
            EventType::MigrationFailed { .. } => "migration_failed",
            EventType::AuditRecorded { .. } => "audit_recorded",
            EventType::ComplianceCheckCompleted { .. } => "compliance_check_completed",
            EventType::TlsCertExpiring { .. } => "tls_cert_expiring",
            EventType::TlsCertRenewed { .. } => "tls_cert_renewed",
            EventType::AuthTokenRevoked { .. } => "auth_token_revoked",
            EventType::SchedulerTaskFired { .. } => "scheduler_task_fired",
            EventType::QueueJobFailed { .. } => "queue_job_failed",
            EventType::CacheHitRateLow { .. } => "cache_hit_rate_low",
            EventType::CircuitBreakerTripped { .. } => "circuit_breaker_tripped",
            EventType::CdcBatchCommitted { .. } => "cdc_batch_committed",
            EventType::ShardRebalanceStarted { .. } => "shard_rebalance_started",
            EventType::ArchivalTierTransition { .. } => "archival_tier_transition",
            EventType::CostBudgetAlert { .. } => "cost_budget_alert",
            EventType::ChaosExperimentStarted { .. } => "chaos_experiment_started",
            EventType::ChaosExperimentCompleted { .. } => "chaos_experiment_completed",
            EventType::Custom { event_name, .. } => event_name,
        }
    }

    /// Severity as a string for logging.
    pub fn severity_str(&self) -> &str {
        match self.severity {
            Severity::Info => "INFO",
            Severity::Notice => "NOTICE",
            Severity::Warning => "WARN",
            Severity::Error => "ERROR",
            Severity::Critical => "CRITICAL",
        }
    }
}

/// Trait for components that can receive shim events.
#[async_trait::async_trait]
pub trait EventHandler: Send + Sync {
    /// Handle an incoming event.
    async fn handle_event(&self, event: &ShimEvent) -> crate::error::Result<()>;

    /// Return the event types this handler is interested in.
    /// Empty = all events.
    fn interested_in(&self) -> Vec<EventType> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_new() {
        let evt = ShimEvent::new(
            "backup-shim",
            EventType::BackupCompleted {
                name: "daily".into(),
                size_bytes: 1024,
                checksum: "abc123".into(),
            },
            Severity::Info,
        );
        assert_eq!(evt.source, "backup-shim");
        assert_eq!(evt.sequence, 0);
        assert!(!evt.is_alertable());
    }

    #[test]
    fn test_event_builder() {
        let id = Uuid::new_v4();
        let evt = ShimEvent::new(
            "tls-shim",
            EventType::TlsCertExpiring {
                cert_path: "/etc/tls/cert.pem".into(),
                days_remaining: 7,
            },
            Severity::Warning,
        )
        .with_sequence(42)
        .with_correlation(id)
        .with_payload(serde_json::json!({"action": "renew"}));

        assert_eq!(evt.sequence, 42);
        assert_eq!(evt.correlation_id, Some(id));
        assert!(evt.is_alertable());
        assert!(evt.payload.is_some());
    }

    #[test]
    fn test_severity_serialization() {
        let s = serde_json::to_string(&Severity::Critical).unwrap();
        assert_eq!(s, "\"critical\"");
    }

    #[test]
    fn test_event_type_serialization() {
        let evt = EventType::FailoverTriggered {
            old_primary: "pg-1".into(),
            new_primary: "pg-2".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("failover_triggered"));
        // Test roundtrip
        let deser: EventType = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, evt);
    }

    #[test]
    fn test_event_serialization_roundtrip() {
        let evt = ShimEvent::new(
            "auth-shim",
            EventType::AuthTokenRevoked {
                token_id: "tok-123".into(),
                reason: "expired".into(),
            },
            Severity::Notice,
        );
        let json = serde_json::to_string(&evt).unwrap();
        let deser: ShimEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.source, "auth-shim");
        assert_eq!(deser.event, evt.event);
        assert_eq!(deser.severity, Severity::Notice);
    }

    #[test]
    fn test_custom_event() {
        let evt = EventType::Custom {
            event_name: "myapp.metric".into(),
            payload: serde_json::json!({"value": 42}),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("myapp.metric"));
        let deser: EventType = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, evt);
    }

    // --- property-based: Severity parsing ---

    #[test]
    fn test_severity_parse_all_valid_json_values() {
        let variants = ["info", "notice", "warning", "error", "critical"];
        for v in variants {
            let json = format!("\"{}\"", v);
            let parsed: Severity = serde_json::from_str(&json).unwrap();
            let roundtrip = serde_json::to_string(&parsed).unwrap();
            assert_eq!(roundtrip, json);
        }
    }

    #[test]
    fn test_severity_case_insensitive_parse() {
        // serde rename_all = "lowercase" means only lowercase is valid
        let json = "\"info\"";
        let parsed: Severity = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, Severity::Info);

        // Uppercase should fail
        assert!(serde_json::from_str::<Severity>("\"INFO\"").is_err());
    }

    #[test]
    fn test_severity_invalid_values_rejected() {
        assert!(serde_json::from_str::<Severity>("\"verbose\"").is_err());
        assert!(serde_json::from_str::<Severity>("\"trace\"").is_err());
        assert!(serde_json::from_str::<Severity>("\"debug\"").is_err());
        assert!(serde_json::from_str::<Severity>("\"fatal\"").is_err());
        assert!(serde_json::from_str::<Severity>("\"\"").is_err());
        assert!(serde_json::from_str::<Severity>("null").is_err());
    }

    #[test]
    fn test_severity_ordering_by_alertability() {
        let severities = [
            Severity::Info,
            Severity::Notice,
            Severity::Warning,
            Severity::Error,
            Severity::Critical,
        ];
        // Only Warning+ should be alertable
        for (i, s) in severities.iter().enumerate() {
            let evt = ShimEvent::new(
                "x",
                EventType::Custom {
                    event_name: "t".into(),
                    payload: serde_json::json!(null),
                },
                *s,
            );
            assert_eq!(
                evt.is_alertable(),
                i >= 2,
                "severity {:?} alertability mismatch",
                s
            );
        }
    }

    #[test]
    fn test_event_type_json_roundtrip_all_variants() {
        let events = vec![
            EventType::HealthStatusChanged {
                previous: "down".into(),
                current: "up".into(),
            },
            EventType::FailoverTriggered {
                old_primary: "p1".into(),
                new_primary: "p2".into(),
            },
            EventType::FailoverCompleted {
                promoted: "p2".into(),
            },
            EventType::BackupStarted {
                name: "daily".into(),
            },
            EventType::BackupCompleted {
                name: "daily".into(),
                size_bytes: 1024,
                checksum: "abc".into(),
            },
            EventType::BackupFailed {
                name: "daily".into(),
                reason: "timeout".into(),
            },
            EventType::EncryptionKeyRotated {
                key_id: "k1".into(),
                algorithm: "aes256".into(),
            },
            EventType::ReplicationLagWarning {
                lag_ms: 500,
                threshold_ms: 100,
            },
            EventType::MigrationStarted {
                version: "v1".into(),
            },
            EventType::MigrationCompleted {
                version: "v1".into(),
            },
            EventType::MigrationFailed {
                version: "v1".into(),
                reason: "syntax".into(),
            },
            EventType::AuditRecorded {
                event_type: "read".into(),
                resource: "users".into(),
                action: "SELECT".into(),
            },
            EventType::ComplianceCheckCompleted {
                standard: "SOC2".into(),
                score: 95.0,
                violations: 2,
            },
            EventType::TlsCertExpiring {
                cert_path: "/etc/tls/cert.pem".into(),
                days_remaining: 7,
            },
            EventType::TlsCertRenewed {
                cert_path: "/etc/tls/cert.pem".into(),
            },
            EventType::AuthTokenRevoked {
                token_id: "tok1".into(),
                reason: "expired".into(),
            },
            EventType::SchedulerTaskFired {
                task_name: "backup".into(),
                schedule: "0 2 * * *".into(),
            },
            EventType::QueueJobFailed {
                job_id: "j1".into(),
                queue: "email".into(),
                retries: 3,
            },
            EventType::CacheHitRateLow {
                hit_rate: 0.3,
                threshold: 0.8,
            },
            EventType::CircuitBreakerTripped {
                service: "db".into(),
                state: "open".into(),
            },
            EventType::CdcBatchCommitted {
                table: "users".into(),
                event_count: 50,
            },
            EventType::ShardRebalanceStarted {
                from_shard: "s1".into(),
                to_shard: "s2".into(),
            },
            EventType::ArchivalTierTransition {
                resource: "logs".into(),
                from_tier: "hot".into(),
                to_tier: "cold".into(),
            },
            EventType::CostBudgetAlert {
                budget_name: "aws".into(),
                usage_percent: 90.0,
            },
            EventType::ChaosExperimentStarted {
                experiment: "pod-kill".into(),
            },
            EventType::ChaosExperimentCompleted {
                experiment: "pod-kill".into(),
                result: "recovered".into(),
            },
            EventType::Custom {
                event_name: "myapp.event".into(),
                payload: serde_json::json!({"key": "val"}),
            },
        ];

        for evt in events {
            let json = serde_json::to_string(&evt).unwrap();
            let deser: EventType = serde_json::from_str(&json).unwrap();
            assert_eq!(deser, evt, "roundtrip failed for variant");
        }
    }

    #[test]
    fn test_is_alertable() {
        let info = ShimEvent::new(
            "x",
            EventType::Custom {
                event_name: "test".into(),
                payload: serde_json::json!(null),
            },
            Severity::Info,
        );
        let warn = ShimEvent::new(
            "x",
            EventType::Custom {
                event_name: "test".into(),
                payload: serde_json::json!(null),
            },
            Severity::Warning,
        );
        let err = ShimEvent::new(
            "x",
            EventType::Custom {
                event_name: "test".into(),
                payload: serde_json::json!(null),
            },
            Severity::Error,
        );
        let crit = ShimEvent::new(
            "x",
            EventType::Custom {
                event_name: "test".into(),
                payload: serde_json::json!(null),
            },
            Severity::Critical,
        );
        assert!(!info.is_alertable());
        assert!(warn.is_alertable());
        assert!(err.is_alertable());
        assert!(crit.is_alertable());
    }
}
