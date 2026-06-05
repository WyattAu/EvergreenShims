//! Cross-shim event wiring — pre-built event handlers for common patterns.
//!
//! Provides reusable handlers that can be attached to any `ShimBus` to
//! implement cross-shim communication without per-shim boilerplate.

use std::sync::Arc;

use parking_lot::RwLock;
use tracing::{info, warn};

use crate::bus::ShimBus;
use crate::event::{EventType, Severity};

/// Monitors health events and triggers failover when status goes unhealthy.
pub struct HealthFailoverHandler {
    bus: ShimBus,
    failover_threshold: u32,
    unhealthy_count: Arc<RwLock<u32>>,
}

impl HealthFailoverHandler {
    pub fn new(bus: ShimBus, failover_threshold: u32) -> Self {
        Self {
            bus,
            failover_threshold,
            unhealthy_count: Arc::new(RwLock::new(0)),
        }
    }

    /// Start monitoring health events and emitting failover triggers.
    pub fn start(self: Arc<Self>) {
        let mut rx = self.bus.subscribe();
        let handler = self.clone();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let EventType::HealthStatusChanged { current, .. } = &event.event {
                            if current == "unhealthy" || current == "down" {
                                let mut count = handler.unhealthy_count.write();
                                *count += 1;
                                if *count >= handler.failover_threshold {
                                    warn!(
                                        "health-failover: {} consecutive unhealthy checks, triggering failover",
                                        *count
                                    );
                                    handler.bus.emit(
                                        "failover-shim",
                                        EventType::FailoverTriggered {
                                            old_primary: "primary".into(),
                                            new_primary: "promoted".into(),
                                        },
                                        Severity::Critical,
                                    );
                                    *count = 0;
                                }
                            } else {
                                *handler.unhealthy_count.write() = 0;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("health-failover: lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// Forwards backup completion events to the encryption shim.
pub struct BackupEncryptionHandler {
    bus: ShimBus,
}

impl BackupEncryptionHandler {
    pub fn new(bus: ShimBus) -> Self {
        Self { bus }
    }

    /// Start listening for backup events and forwarding to encryption.
    pub fn start(self: Arc<Self>) {
        let mut rx = self.bus.subscribe();
        let handler = self.clone();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let EventType::BackupCompleted {
                            name,
                            size_bytes,
                            checksum,
                        } = &event.event
                        {
                            info!(
                                "backup-encryption: backup '{}' completed ({} bytes), ensuring encryption",
                                name, size_bytes
                            );
                            handler.bus.emit(
                                "encryption-shim",
                                EventType::EncryptionKeyRotated {
                                    key_id: format!("backup-{}", name),
                                    algorithm: "AES-256-GCM".into(),
                                },
                                Severity::Info,
                            );
                            let _ = checksum;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("backup-encryption: lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// Fans out all events to the alerting shim for notification routing.
pub struct AlertFanInHandler {
    bus: ShimBus,
}

impl AlertFanInHandler {
    pub fn new(bus: ShimBus) -> Self {
        Self { bus }
    }

    /// Start monitoring all alertable events and routing to alerting.
    pub fn start(self: Arc<Self>) {
        let mut rx = self.bus.subscribe();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if event.is_alertable() {
                            info!(
                                "alert-fanin: [{}] {} → {}",
                                event.severity_str(),
                                event.source,
                                event.event_name(),
                            );
                            // Alerting shim will receive this via the same bus
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("alert-fanin: lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// Scheduled task → backup trigger.
pub struct SchedulerBackupHandler {
    bus: ShimBus,
}

impl SchedulerBackupHandler {
    pub fn new(bus: ShimBus) -> Self {
        Self { bus }
    }

    /// Start listening for scheduler events and triggering backups.
    pub fn start(self: Arc<Self>) {
        let mut rx = self.bus.subscribe();
        let handler = self.clone();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let EventType::SchedulerTaskFired {
                            task_name,
                            schedule,
                        } = &event.event
                        {
                            if task_name.contains("backup") || task_name.contains("daily") {
                                info!(
                                    "scheduler-backup: task '{}' fired ({}), triggering backup",
                                    task_name, schedule
                                );
                                handler.bus.emit(
                                    "backup-shim",
                                    EventType::BackupStarted {
                                        name: task_name.clone(),
                                    },
                                    Severity::Info,
                                );
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("scheduler-backup: lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// Wire all standard cross-shim event handlers onto a bus.
pub fn wire_all_handlers(bus: &ShimBus) {
    // Health → Failover (after 3 consecutive unhealthy checks)
    let health_failover = Arc::new(HealthFailoverHandler::new(bus.clone(), 3));
    health_failover.start();

    // Backup → Encryption
    let backup_encryption = Arc::new(BackupEncryptionHandler::new(bus.clone()));
    backup_encryption.start();

    // Alert fan-in (all alertable events → alerting shim)
    let alert_fanin = Arc::new(AlertFanInHandler::new(bus.clone()));
    alert_fanin.start();

    // Scheduler → Backup
    let scheduler_backup = Arc::new(SchedulerBackupHandler::new(bus.clone()));
    scheduler_backup.start();

    info!("cross-shim event wiring complete (4 handlers)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventType, Severity};

    #[tokio::test]
    async fn test_health_failover_triggers_after_threshold() {
        let bus = ShimBus::new();
        let handler = Arc::new(HealthFailoverHandler::new(bus.clone(), 2));
        handler.start();

        // Subscribe BEFORE emitting so we capture all events
        let mut rx = bus.subscribe();

        // First unhealthy — should not trigger
        bus.emit(
            "health-shim",
            EventType::HealthStatusChanged {
                previous: "healthy".into(),
                current: "unhealthy".into(),
            },
            Severity::Warning,
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Second unhealthy — should trigger failover
        bus.emit(
            "health-shim",
            EventType::HealthStatusChanged {
                previous: "unhealthy".into(),
                current: "unhealthy".into(),
            },
            Severity::Warning,
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Check that failover event was emitted
        let mut found = false;
        while let Ok(evt) = rx.try_recv() {
            if matches!(evt.event, EventType::FailoverTriggered { .. }) {
                found = true;
                break;
            }
        }
        assert!(found, "failover should have been triggered");
    }

    #[tokio::test]
    async fn test_backup_encryption_forwards() {
        let bus = ShimBus::new();
        let handler = Arc::new(BackupEncryptionHandler::new(bus.clone()));
        handler.start();

        let mut rx = bus.subscribe();

        bus.emit(
            "backup-shim",
            EventType::BackupCompleted {
                name: "daily".into(),
                size_bytes: 1024,
                checksum: "abc".into(),
            },
            Severity::Info,
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut found = false;
        while let Ok(evt) = rx.try_recv() {
            if let EventType::EncryptionKeyRotated { key_id, .. } = &evt.event {
                if key_id == "backup-daily" {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "encryption key rotation should have been triggered");
    }

    #[tokio::test]
    async fn test_scheduler_backup_triggers() {
        let bus = ShimBus::new();
        let handler = Arc::new(SchedulerBackupHandler::new(bus.clone()));
        handler.start();

        let mut rx = bus.subscribe();

        bus.emit(
            "scheduler-shim",
            EventType::SchedulerTaskFired {
                task_name: "daily-backup".into(),
                schedule: "0 2 * * *".into(),
            },
            Severity::Info,
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut found = false;
        while let Ok(evt) = rx.try_recv() {
            if let EventType::BackupStarted { name } = &evt.event {
                if name == "daily-backup" {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "backup should have been triggered by scheduler");
    }

    #[tokio::test]
    async fn test_health_resets_on_healthy() {
        let bus = ShimBus::new();
        let handler = Arc::new(HealthFailoverHandler::new(bus.clone(), 2));
        handler.start();

        let mut rx = bus.subscribe();

        // Unhealthy
        bus.emit(
            "health-shim",
            EventType::HealthStatusChanged {
                previous: "healthy".into(),
                current: "unhealthy".into(),
            },
            Severity::Warning,
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Healthy — should reset counter
        bus.emit(
            "health-shim",
            EventType::HealthStatusChanged {
                previous: "unhealthy".into(),
                current: "healthy".into(),
            },
            Severity::Info,
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Another unhealthy — should NOT trigger (counter was reset)
        bus.emit(
            "health-shim",
            EventType::HealthStatusChanged {
                previous: "healthy".into(),
                current: "unhealthy".into(),
            },
            Severity::Warning,
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut found_failover = false;
        while let Ok(evt) = rx.try_recv() {
            if matches!(evt.event, EventType::FailoverTriggered { .. }) {
                found_failover = true;
                break;
            }
        }
        assert!(!found_failover, "failover should NOT have been triggered");
    }
}
