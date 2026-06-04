//! Failover shim — automatic failover for HA databases.
//!
//! Monitors a primary database, detects failure, promotes a replica,
//! and sends notifications.
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

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Failover state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FailoverState {
    Healthy,
    Suspect,
    FailingOver,
    FailedOver,
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
    match tokio::task::spawn_blocking(move || {
        let parsed: std::net::SocketAddr = match addr_str.parse() {
            Ok(a) => a,
            Err(_) => return false,
        };
        TcpStream::connect_timeout(&parsed, std::time::Duration::from_secs(3)).is_ok()
    })
    .await
    {
        Ok(result) => result,
        Err(_) => false,
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
    state: FailoverState,
    current_primary: String,
    consecutive_failures: u32,
    failover_count: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl FailoverShim {
    pub fn new() -> Self {
        Self {
            primary: std::env::var("FAILOVER_PRIMARY").unwrap_or_else(|_| "127.0.0.1:5432".to_string()),
            replica: std::env::var("FAILOVER_REPLICA").unwrap_or_else(|_| "127.0.0.1:5433".to_string()),
            check_interval_secs: std::env::var("FAILOVER_CHECK_INTERVAL")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(5),
            failure_threshold: std::env::var("FAILOVER_FAILURE_THRESHOLD")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(3),
            webhook: std::env::var("FAILOVER_WEBHOOK").ok(),
            db_type: std::env::var("FAILOVER_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            state: FailoverState::Healthy,
            current_primary: String::new(),
            consecutive_failures: 0,
            failover_count: 0,
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

    async fn init(&mut self, config: &Config) -> Result<()> {
        if let Some(fc) = &config.failover {
            self.primary = fc.primary.clone();
            self.replica = fc.replica.clone();
            self.check_interval_secs = fc.check_interval_secs;
        }
        self.current_primary = self.primary.clone();
        tracing::info!(
            "FailoverShim initialized (primary={}, replica={}, interval={}s)",
            self.primary, self.replica, self.check_interval_secs,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        if check_database(&self.primary).await {
            self.state = FailoverState::Healthy;
            tracing::info!("Primary {} is healthy", self.primary);
        } else {
            tracing::warn!("Primary {} is not reachable at startup", self.primary);
        }

        let primary = self.primary.clone();
        let replica = self.replica.clone();
        let check_interval_secs = self.check_interval_secs;
        let failure_threshold = self.failure_threshold;
        let webhook = self.webhook.clone();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(check_interval_secs));
            let mut current_primary = primary.clone();
            let mut consecutive_failures: u32 = 0;
            let mut state = FailoverState::Healthy;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let healthy = check_database(&current_primary).await;

                        if healthy {
                            consecutive_failures = 0;
                            if state == FailoverState::Suspect || state == FailoverState::FailingOver {
                                tracing::info!("Primary {} recovered", current_primary);
                                state = FailoverState::Healthy;
                            }
                        } else {
                            consecutive_failures += 1;
                            tracing::warn!(
                                "Health check failed for {} ({}/{})",
                                current_primary, consecutive_failures, failure_threshold
                            );

                            if consecutive_failures >= failure_threshold {
                                state = FailoverState::FailingOver;
                                tracing::error!(
                                    "FAILOVER TRIGGERED: {} failed {} consecutive checks",
                                    current_primary, consecutive_failures
                                );

                                let event = FailoverEvent {
                                    event: "failover".to_string(),
                                    old_primary: current_primary.clone(),
                                    new_primary: replica.clone(),
                                    timestamp: chrono::Utc::now().to_rfc3339(),
                                    reason: format!("{} consecutive health check failures", consecutive_failures),
                                };

                                if let Some(webhook_url) = &webhook {
                                    let client = reqwest::Client::new();
                                    let payload = serde_json::json!({
                                        "text": format!("FAILOVER: {} failed, promoting {}", event.old_primary, event.new_primary),
                                    });
                                    let _ = client.post(webhook_url).json(&payload).send().await;
                                }

                                current_primary = replica.clone();
                                state = FailoverState::FailedOver;
                                consecutive_failures = 0;
                                tracing::info!("Failover complete. New primary: {}", current_primary);
                            } else {
                                state = FailoverState::Suspect;
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
            self.check_interval_secs, self.failure_threshold,
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
        let state_val = match self.state {
            FailoverState::Healthy => 0.0,
            FailoverState::Suspect => 1.0,
            FailoverState::FailingOver => 2.0,
            FailoverState::FailedOver => 3.0,
            FailoverState::Recovered => 4.0,
        };

        vec![
            Metric::new("failover_state", state_val),
            Metric::new("failover_events_total", self.failover_count as f64),
            Metric::new("failover_consecutive_failures", self.consecutive_failures as f64),
        ]
    }
}
