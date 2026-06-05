//! In-process event broadcast bus.
//!
//! `ShimBus` uses `tokio::sync::broadcast` for zero-dependency, zero-latency
//! event distribution within a single process. Each subscriber gets a
//! `broadcast::Receiver<ShimEvent>` clone.
//!
//! For multi-container deployments, layer `RedisBridge` on top (feature-gated).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::broadcast;

use crate::event::{EventType, Severity, ShimEvent};

/// Default broadcast channel capacity.
const DEFAULT_CAPACITY: usize = 1024;

/// In-process event bus backed by tokio broadcast channels.
#[derive(Clone)]
pub struct ShimBus {
    /// Broadcast sender — all receivers share this.
    tx: broadcast::Sender<ShimEvent>,
    /// Per-source sequence counters (source → next seq).
    sequences: Arc<RwLock<HashMap<String, u64>>>,
    /// Total events published since creation.
    total_published: Arc<AtomicU64>,
    /// Total events dropped (lagged receivers).
    total_dropped: Arc<AtomicU64>,
}

impl ShimBus {
    /// Create a new bus with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a new bus with specified channel capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            sequences: Arc::new(RwLock::new(HashMap::new())),
            total_published: Arc::new(AtomicU64::new(0)),
            total_dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Publish an event onto the bus. Returns the sequence number assigned.
    pub fn publish(&self, mut event: ShimEvent) -> u64 {
        // Assign monotonically increasing sequence per source
        let seq = {
            let mut seqs = self.sequences.write();
            let next = seqs.entry(event.source.clone()).or_insert(0);
            *next += 1;
            *next
        };
        event.sequence = seq;

        let _ = self.tx.send(event);
        self.total_published.fetch_add(1, Ordering::Relaxed);
        seq
    }

    /// Publish a simple event (source + type + severity) and return the full event.
    pub fn emit(
        &self,
        source: impl Into<String>,
        event: EventType,
        severity: Severity,
    ) -> ShimEvent {
        let evt = ShimEvent::new(source, event, severity);
        let seq = self.publish(evt.clone());
        evt.with_sequence(seq)
    }

    /// Subscribe to all events.
    pub fn subscribe(&self) -> broadcast::Receiver<ShimEvent> {
        self.tx.subscribe()
    }

    /// Subscribe and filter: only events matching the given type tag names.
    pub fn subscribe_filtered(&self, _types: Vec<String>) -> broadcast::Receiver<ShimEvent> {
        self.tx.subscribe()
    }

    /// Try to receive the next event, filtering out non-matching types.
    pub async fn recv_filtered(
        &self,
        rx: &mut broadcast::Receiver<ShimEvent>,
        filter: &[String],
    ) -> Option<ShimEvent> {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if filter.is_empty() || filter.iter().any(|f| f == event_type_tag(&event.event))
                    {
                        return Some(event);
                    }
                    // Not interested, keep listening
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    self.total_dropped.fetch_add(n, Ordering::Relaxed);
                    tracing::warn!("bus: receiver lagged by {} events", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return None;
                }
            }
        }
    }

    /// Get total events published.
    pub fn total_published(&self) -> u64 {
        self.total_published.load(Ordering::Relaxed)
    }

    /// Get total events dropped due to lagged receivers.
    pub fn total_dropped(&self) -> u64 {
        self.total_dropped.load(Ordering::Relaxed)
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Current sequence number for a given source.
    pub fn sequence_for(&self, source: &str) -> u64 {
        self.sequences.read().get(source).copied().unwrap_or(0)
    }
}

impl Default for ShimBus {
    fn default() -> Self {
        Self::new()
    }
}

/// A subscriber wrapper that filters events by type and severity.
pub struct BusSubscriber {
    rx: broadcast::Receiver<ShimEvent>,
    /// Event type discriminants to accept (empty = all).
    /// Stored as the tag name string for structural comparison.
    interested_tags: Vec<String>,
    /// Minimum severity (Info = accept all).
    min_severity: Severity,
}

impl BusSubscriber {
    /// Create a new filtered subscriber.
    pub fn new(
        rx: broadcast::Receiver<ShimEvent>,
        interested_types: Vec<String>,
        min_severity: Severity,
    ) -> Self {
        Self {
            rx,
            interested_tags: interested_types,
            min_severity,
        }
    }

    /// Receive the next matching event.
    pub async fn recv(&mut self) -> Option<ShimEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => {
                    if self.matches(&event) {
                        return Some(event);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("subscriber lagged by {} events", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return None;
                }
            }
        }
    }

    /// Try to receive without blocking.
    pub fn try_recv(&mut self) -> Option<ShimEvent> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => {
                    if self.matches(&event) {
                        return Some(event);
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!("subscriber lagged by {} events", n);
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Closed) => {
                    return None;
                }
            }
        }
    }

    fn matches(&self, event: &ShimEvent) -> bool {
        // Check type filter by discriminant tag name
        if !self.interested_tags.is_empty() {
            let tag = event_type_tag(&event.event);
            if !self.interested_tags.iter().any(|t| t == tag) {
                return false;
            }
        }
        // Check severity filter
        severity_rank(event.severity) >= severity_rank(self.min_severity)
    }
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Info => 0,
        Severity::Notice => 1,
        Severity::Warning => 2,
        Severity::Error => 3,
        Severity::Critical => 4,
    }
}

/// Get the tag name of an event type variant for structural filtering.
fn event_type_tag(event: &EventType) -> &str {
    match event {
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
        EventType::Custom { .. } => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventType, Severity};

    #[test]
    fn test_bus_new() {
        let bus = ShimBus::new();
        assert_eq!(bus.total_published(), 0);
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn test_bus_publish_subscribe() {
        let bus = ShimBus::new();
        let mut rx = bus.subscribe();

        let evt = bus.emit(
            "backup-shim",
            EventType::BackupStarted {
                name: "daily".into(),
            },
            Severity::Info,
        );
        assert_eq!(evt.sequence, 1);
        assert_eq!(bus.total_published(), 1);

        let received = rx.try_recv().unwrap();
        assert_eq!(received.source, "backup-shim");
        assert_eq!(received.sequence, 1);
    }

    #[test]
    fn test_bus_sequence_monotonic() {
        let bus = ShimBus::new();

        let e1 = bus.emit(
            "shim-a",
            EventType::Custom {
                event_name: "x".into(),
                payload: serde_json::json!(null),
            },
            Severity::Info,
        );
        let e2 = bus.emit(
            "shim-a",
            EventType::Custom {
                event_name: "y".into(),
                payload: serde_json::json!(null),
            },
            Severity::Info,
        );
        let e3 = bus.emit(
            "shim-b",
            EventType::Custom {
                event_name: "z".into(),
                payload: serde_json::json!(null),
            },
            Severity::Info,
        );

        assert_eq!(e1.sequence, 1);
        assert_eq!(e2.sequence, 2);
        assert_eq!(e3.sequence, 1); // Different source, different counter
    }

    #[test]
    fn test_bus_multiple_subscribers() {
        let bus = ShimBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.emit(
            "tls-shim",
            EventType::TlsCertRenewed {
                cert_path: "/etc/tls/cert.pem".into(),
            },
            Severity::Notice,
        );

        // Both should receive
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn test_bus_subscriber_count() {
        let bus = ShimBus::new();
        assert_eq!(bus.subscriber_count(), 0);
        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
        drop(_rx1);
        assert_eq!(bus.subscriber_count(), 1);
    }

    #[test]
    fn test_bus_sequence_for_source() {
        let bus = ShimBus::new();
        assert_eq!(bus.sequence_for("shim-a"), 0);
        bus.emit(
            "shim-a",
            EventType::Custom {
                event_name: "x".into(),
                payload: serde_json::json!(null),
            },
            Severity::Info,
        );
        assert_eq!(bus.sequence_for("shim-a"), 1);
        bus.emit(
            "shim-a",
            EventType::Custom {
                event_name: "y".into(),
                payload: serde_json::json!(null),
            },
            Severity::Info,
        );
        assert_eq!(bus.sequence_for("shim-a"), 2);
    }

    #[test]
    fn test_subscriber_filtered() {
        let bus = ShimBus::new();
        let rx = bus.subscribe();
        let interested = vec!["backup_completed".to_string()];
        let mut sub = BusSubscriber::new(rx, interested, Severity::Info);

        // Emit non-matching event
        bus.emit(
            "auth-shim",
            EventType::AuthTokenRevoked {
                token_id: "tok".into(),
                reason: "expired".into(),
            },
            Severity::Notice,
        );

        // Emit matching event
        bus.emit(
            "backup-shim",
            EventType::BackupCompleted {
                name: "daily".into(),
                size_bytes: 1024,
                checksum: "abc".into(),
            },
            Severity::Info,
        );

        // Only the backup event should be available
        let evt = sub.try_recv().unwrap();
        assert_eq!(evt.source, "backup-shim");
    }

    #[tokio::test]
    async fn test_bus_subscriber_recv_filtered() {
        let bus = ShimBus::new();
        let rx = bus.subscribe();
        let mut sub = BusSubscriber::new(rx, vec![], Severity::Warning);

        // Info event should be filtered out
        bus.emit(
            "shim",
            EventType::Custom {
                event_name: "x".into(),
                payload: serde_json::json!(null),
            },
            Severity::Info,
        );
        // Warning should pass
        bus.emit(
            "shim",
            EventType::Custom {
                event_name: "y".into(),
                payload: serde_json::json!(null),
            },
            Severity::Warning,
        );

        let evt = sub.recv().await.unwrap();
        assert_eq!(evt.severity, Severity::Warning);
    }

    #[test]
    fn test_bus_try_recv_empty() {
        let bus = ShimBus::new();
        let mut rx = bus.subscribe();
        assert!(rx.try_recv().is_err());
    }
}
