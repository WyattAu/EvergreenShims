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
    HealthStatusChanged {
        previous: String,
        current: String,
    },
    /// Failover was triggered — primary down, promoting replica.
    FailoverTriggered {
        old_primary: String,
        new_primary: String,
    },
    /// Failover completed — new primary is serving traffic.
    FailoverCompleted {
        promoted: String,
    },

    // ── Backup / Encryption ───────────────────────────────────────────
    /// Backup started.
    BackupStarted {
        name: String,
    },
    /// Backup completed successfully.
    BackupCompleted {
        name: String,
        size_bytes: u64,
        checksum: String,
    },
    /// Backup failed.
    BackupFailed {
        name: String,
        reason: String,
    },
    /// Encryption key rotation completed.
    EncryptionKeyRotated {
        key_id: String,
        algorithm: String,
    },

    // ── Replication / Migration ───────────────────────────────────────
    /// Replication lag exceeded threshold.
    ReplicationLagWarning {
        lag_ms: u64,
        threshold_ms: u64,
    },
    /// Migration started.
    MigrationStarted {
        version: String,
    },
    /// Migration completed.
    MigrationCompleted {
        version: String,
    },
    /// Migration failed.
    MigrationFailed {
        version: String,
        reason: String,
    },

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
    TlsCertRenewed {
        cert_path: String,
    },
    /// Auth token expired or was revoked.
    AuthTokenRevoked {
        token_id: String,
        reason: String,
    },

    // ── Scheduler / Queue ─────────────────────────────────────────────
    /// Scheduled task fired.
    SchedulerTaskFired {
        task_name: String,
        schedule: String,
    },
    /// Queue job failed (sent to DLQ).
    QueueJobFailed {
        job_id: String,
        queue: String,
        retries: u32,
    },

    // ── Cache / Proxy ─────────────────────────────────────────────────
    /// Cache hit rate dropped below threshold.
    CacheHitRateLow {
        hit_rate: f64,
        threshold: f64,
    },
    /// Circuit breaker state changed.
    CircuitBreakerTripped {
        service: String,
        state: String,
    },

    // ── CDC / Sharding ────────────────────────────────────────────────
    /// CDC event batch committed.
    CdcBatchCommitted {
        table: String,
        event_count: u32,
    },
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
    ChaosExperimentStarted {
        experiment: String,
    },
    /// Chaos experiment completed.
    ChaosExperimentCompleted {
        experiment: String,
        result: String,
    },

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

    #[test]
    fn test_is_alertable() {
        let info = ShimEvent::new("x", EventType::Custom { event_name: "test".into(), payload: serde_json::json!(null) }, Severity::Info);
        let warn = ShimEvent::new("x", EventType::Custom { event_name: "test".into(), payload: serde_json::json!(null) }, Severity::Warning);
        let err = ShimEvent::new("x", EventType::Custom { event_name: "test".into(), payload: serde_json::json!(null) }, Severity::Error);
        let crit = ShimEvent::new("x", EventType::Custom { event_name: "test".into(), payload: serde_json::json!(null) }, Severity::Critical);
        assert!(!info.is_alertable());
        assert!(warn.is_alertable());
        assert!(err.is_alertable());
        assert!(crit.is_alertable());
    }
}
