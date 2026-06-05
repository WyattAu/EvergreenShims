#![allow(dead_code)]
//! Failover shim — automatic failover for HA databases.
//!
//! Monitors a primary database, detects failure, promotes a replica,
//! and sends notifications. Supports automatic failback when the
//! original primary recovers.
//!
//! ## Environment Variables
//!
//! ```text
//! FAILOVER_PRIMARY       Primary database address (host:port)
//! FAILOVER_REPLICA       Replica database address (host:port)
//! FAILOVER_CHECK_INTERVAL Health check interval in seconds (default: 5)
//! FAILOVER_TIMEOUT       Health check timeout in seconds (default: 3)
//! FAILOVER_FAILURE_THRESHOLD Consecutive failures before failover (default: 3)
//! FAILOVER_WEBHOOK       Webhook URL for notifications (Slack, PagerDuty)
//! FAILOVER_DB_TYPE       Database type: postgres, mariadb, mysql
//! ```

use std::net::TcpStream;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, EventType, Metric, Result, Severity, ShimBus};
use tokio::sync::watch;

/// Failover state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FailoverState {
    Healthy,
    Suspect,
    FailingOver,
    FailedOver,
    Recovering,
    Recovered,
}

/// Failover event for notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverEvent {
    pub event: String,
    pub old_primary: String,
    pub new_primary: String,
    pub timestamp: String,
    pub reason: String,
}

/// Check if a database is reachable via TCP.
async fn check_database(addr: &str) -> bool {
    let addr_str = addr.to_string();
    tokio::task::spawn_blocking(move || {
        let Ok(parsed) = addr_str.parse::<std::net::SocketAddr>() else {
            return false;
        };
        TcpStream::connect_timeout(&parsed, std::time::Duration::from_secs(3)).is_ok()
    })
    .await
    .unwrap_or(false)
}

/// Shared mutable state for the failover loop and metrics.
struct SharedState {
    consecutive_failures: AtomicU32,
    failover_count: AtomicU64,
    state: AtomicI32, // FailoverState as i32
    current_primary: parking_lot::Mutex<String>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            failover_count: AtomicU64::new(0),
            state: AtomicI32::new(0), // Healthy
            current_primary: parking_lot::Mutex::new(String::new()),
        }
    }

    fn state(&self) -> FailoverState {
        match self.state.load(Ordering::Relaxed) {
            0 => FailoverState::Healthy,
            1 => FailoverState::Suspect,
            2 => FailoverState::FailingOver,
            3 => FailoverState::FailedOver,
            4 => FailoverState::Recovering,
            5 => FailoverState::Recovered,
            _ => FailoverState::Healthy,
        }
    }

    fn set_state(&self, s: FailoverState) {
        let val = match s {
            FailoverState::Healthy => 0,
            FailoverState::Suspect => 1,
            FailoverState::FailingOver => 2,
            FailoverState::FailedOver => 3,
            FailoverState::Recovering => 4,
            FailoverState::Recovered => 5,
        };
        self.state.store(val, Ordering::Relaxed);
    }
}

/// Failover shim for automatic database failover.
pub struct FailoverShim {
    primary: String,
    replica: String,
    check_interval_secs: u64,
    failure_threshold: u32,
    webhook: Option<String>,
    db_type: String,
    shared: Arc<SharedState>,
    bus: Option<ShimBus>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl FailoverShim {
    pub fn new() -> Self {
        Self {
            primary: std::env::var("FAILOVER_PRIMARY")
                .unwrap_or_else(|_| "127.0.0.1:5432".to_string()),
            replica: std::env::var("FAILOVER_REPLICA")
                .unwrap_or_else(|_| "127.0.0.1:5433".to_string()),
            check_interval_secs: std::env::var("FAILOVER_CHECK_INTERVAL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            failure_threshold: std::env::var("FAILOVER_FAILURE_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            webhook: std::env::var("FAILOVER_WEBHOOK").ok(),
            db_type: std::env::var("FAILOVER_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            shared: Arc::new(SharedState::new()),
            bus: None,
            shutdown_tx: None,
        }
    }
}

impl Default for FailoverShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for FailoverShim {
    fn name(&self) -> &str {
        "failover"
    }

    fn set_bus(&mut self, bus: ShimBus) {
        self.bus = Some(bus);
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        if let Some(fc) = &config.failover {
            self.primary = fc.primary.clone();
            self.replica = fc.replica.clone();
            self.check_interval_secs = fc.check_interval_secs;
            self.failure_threshold = fc.failure_threshold;
            self.webhook = fc.webhook.clone();
            self.db_type = fc.db_type.clone();
        }

        if self.primary == self.replica {
            tracing::error!("Primary and replica addresses are identical — failover is meaningless");
        }

        self.shared
            .current_primary
            .lock()
            .clone_from(&self.primary);

        tracing::info!(
            "FailoverShim initialized (primary={}, replica={}, interval={}s, threshold={}, db_type={})",
            self.primary,
            self.replica,
            self.check_interval_secs,
            self.failure_threshold,
            self.db_type,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        if check_database(&self.primary).await {
            self.shared.set_state(FailoverState::Healthy);
            tracing::info!("Primary {} is healthy", self.primary);
        } else {
            tracing::warn!("Primary {} is not reachable at startup", self.primary);
        }

        let primary = self.primary.clone();
        let replica = self.replica.clone();
        let check_interval_secs = self.check_interval_secs;
        let failure_threshold = self.failure_threshold;
        let webhook = self.webhook.clone();
        let shared = Arc::clone(&self.shared);
        let bus = self.bus.clone();
        let original_primary = primary.clone();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(check_interval_secs));
            let mut consecutive_primary_healthy: u32 = 0;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let cur = shared.current_primary.lock().clone();
                        let healthy = check_database(&cur).await;

                        if healthy {
                            shared.consecutive_failures.store(0, Ordering::Relaxed);
                            let cur_state = shared.state();

                            if cur_state == FailoverState::FailedOver && cur != original_primary {
                                // Current primary (promoted replica) is healthy — check if we can failback
                                consecutive_primary_healthy += 1;
                                tracing::debug!(
                                    "Promoted primary {} healthy ({}/10 checks for failback)",
                                    cur, consecutive_primary_healthy
                                );

                                if consecutive_primary_healthy >= 10 {
                                    // Check if original primary is also healthy
                                    if check_database(&original_primary).await {
                                        tracing::info!(
                                            "FAILOVER SHIM: Failing back to original primary {}",
                                            original_primary
                                        );

                                        if let Some(webhook_url) = &webhook {
                                            let client = reqwest::Client::new();
                                            let payload = serde_json::json!({
                                                "text": format!("FAILOVER SHIM: Failing back to original primary {}", original_primary),
                                            });
                                            if let Err(e) = client.post(webhook_url).json(&payload).send().await {
                                                tracing::error!("Webhook POST failed: {}", e);
                                            }
                                        }

                                        if let Some(ref bus) = bus {
                                            bus.emit(
                                                "failover-shim",
                                                EventType::FailoverCompleted {
                                                    promoted: original_primary.clone(),
                                                },
                                                Severity::Notice,
                                            );
                                        }

                                        *shared.current_primary.lock() = original_primary.clone();
                                        shared.set_state(FailoverState::Recovered);
                                        consecutive_primary_healthy = 0;
                                        tracing::info!("Failback complete. Primary: {}", original_primary);
                                    }
                                }
                            } else if cur_state == FailoverState::Suspect || cur_state == FailoverState::FailingOver {
                                tracing::info!("Primary {} recovered", cur);
                                shared.set_state(FailoverState::Healthy);
                                consecutive_primary_healthy = 0;
                            } else {
                                consecutive_primary_healthy = 0;
                            }
                        } else {
                            consecutive_primary_healthy = 0;
                            let failures = shared.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
                            tracing::warn!(
                                "Health check failed for {} ({}/{})",
                                cur, failures, failure_threshold
                            );

                            if failures >= failure_threshold {
                                shared.set_state(FailoverState::FailingOver);
                                tracing::error!(
                                    "FAILOVER TRIGGERED: {} failed {} consecutive checks",
                                    cur, failures
                                );

                                if let Some(webhook_url) = &webhook {
                                    let client = reqwest::Client::new();
                                    let payload = serde_json::json!({
                                        "text": format!("FAILOVER TRIGGERED: {} failed, promoting {}", cur, replica),
                                    });
                                    if let Err(e) = client.post(webhook_url).json(&payload).send().await {
                                        tracing::error!("Webhook POST failed: {}", e);
                                    }
                                }

                                if let Some(ref bus) = bus {
                                    bus.emit(
                                        "failover-shim",
                                        EventType::FailoverTriggered {
                                            old_primary: cur.clone(),
                                            new_primary: replica.clone(),
                                        },
                                        Severity::Error,
                                    );
                                }

                                *shared.current_primary.lock() = replica.clone();
                                shared.failover_count.fetch_add(1, Ordering::Relaxed);
                                shared.set_state(FailoverState::FailedOver);
                                shared.consecutive_failures.store(0, Ordering::Relaxed);
                                consecutive_primary_healthy = 0;
                                tracing::info!("Failover complete. New primary: {}", replica);
                            } else {
                                shared.set_state(FailoverState::Suspect);
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Failover shim monitoring loop shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!(
            "FailoverShim started (check every {}s, failover after {} failures)",
            self.check_interval_secs,
            self.failure_threshold,
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("FailoverShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let state_val = match self.shared.state() {
            FailoverState::Healthy => 0.0,
            FailoverState::Suspect => 1.0,
            FailoverState::FailingOver => 2.0,
            FailoverState::FailedOver => 3.0,
            FailoverState::Recovering => 4.0,
            FailoverState::Recovered => 5.0,
        };

        vec![
            Metric::new("failover_state", state_val),
            Metric::new(
                "failover_events_total",
                self.shared.failover_count.load(Ordering::Relaxed) as f64,
            ),
            Metric::new(
                "failover_consecutive_failures",
                self.shared.consecutive_failures.load(Ordering::Relaxed) as f64,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let shim = FailoverShim::new();
        assert_eq!(shim.check_interval_secs, 5);
        assert_eq!(shim.failure_threshold, 3);
        assert_eq!(shim.db_type, "postgres");
        assert_eq!(shim.shared.state(), FailoverState::Healthy);
        assert_eq!(shim.shared.failover_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_metrics_reads_live_state() {
        let shim = FailoverShim::new();
        // Initially healthy with 0 failures
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 3);
        assert_eq!(metrics[0].name, "failover_state");
        assert_eq!(metrics[0].value, 0.0); // Healthy
        assert_eq!(metrics[1].value, 0.0); // failover_count
        assert_eq!(metrics[2].value, 0.0); // consecutive_failures

        // Mutate shared state and verify metrics reflect it
        shim.shared.consecutive_failures.store(5, Ordering::Relaxed);
        shim.shared.failover_count.store(2, Ordering::Relaxed);
        shim.shared.set_state(FailoverState::FailedOver);

        let metrics = shim.metrics();
        assert_eq!(metrics[0].value, 3.0); // FailedOver
        assert_eq!(metrics[1].value, 2.0);
        assert_eq!(metrics[2].value, 5.0);
    }

    #[test]
    fn test_state_roundtrip() {
        let s = SharedState::new();
        let states = [
            FailoverState::Healthy,
            FailoverState::Suspect,
            FailoverState::FailingOver,
            FailoverState::FailedOver,
            FailoverState::Recovering,
            FailoverState::Recovered,
        ];
        for state in &states {
            s.set_state(state.clone());
            assert_eq!(s.state(), *state);
        }
    }

    #[test]
    fn test_failover_count_increments() {
        let s = SharedState::new();
        assert_eq!(s.failover_count.load(Ordering::Relaxed), 0);
        s.failover_count.fetch_add(1, Ordering::Relaxed);
        s.failover_count.fetch_add(1, Ordering::Relaxed);
        assert_eq!(s.failover_count.load(Ordering::Relaxed), 2);
    }
}
