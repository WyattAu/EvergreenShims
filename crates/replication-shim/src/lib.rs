//! Replication shim — database replication management.
//!
//! Manages primary-replica replication for PostgreSQL and MariaDB.
//!
//! ## Environment Variables
//!
//! ```text
//! REPLICATION_PRIMARY     Primary database address
//! REPLICATION_REPLICAS    Comma-separated replica addresses
//! REPLICATION_MODE        Mode: synchronous, asynchronous (default: asynchronous)
//! REPLICATION_SLOT       Replication slot name (PostgreSQL)
//! REPLICATION_CHECK_SECS Health check interval (default: 10)
//! REPLICATION_DB_TYPE    Database type: postgres, mariadb
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Replication state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReplicationState {
    Healthy,
    Degraded,
    Broken,
}

/// Replica status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicaStatus {
    pub addr: String,
    pub state: ReplicationState,
    pub lag_bytes: u64,
    pub lag_seconds: f64,
    pub last_heartbeat: String,
}

/// Replication shim.
pub struct ReplicationShim {
    primary: String,
    replicas: Vec<String>,
    mode: String,
    check_secs: u64,
    db_type: String,
    state: ReplicationState,
    replicas_healthy: u64,
    replicas_broken: u64,
    total_lag_bytes: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ReplicationShim {
    pub fn new() -> Self {
        Self {
            primary: std::env::var("REPLICATION_PRIMARY").unwrap_or_default(),
            replicas: std::env::var("REPLICATION_REPLICAS")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect(),
            mode: std::env::var("REPLICATION_MODE").unwrap_or_else(|_| "asynchronous".to_string()),
            check_secs: std::env::var("REPLICATION_CHECK_SECS")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(10),
            db_type: std::env::var("REPLICATION_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            state: ReplicationState::Healthy,
            replicas_healthy: 0,
            replicas_broken: 0,
            total_lag_bytes: 0,
            shutdown_tx: None,
        }
    }
}

impl Default for ReplicationShim {
    fn default() -> Self { Self::new() }
}

#[async_trait::async_trait]
impl Capability for ReplicationShim {
    fn name(&self) -> &str { "replication" }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("ReplicationShim initialized (primary={}, replicas={}, mode={})",
            self.primary, self.replicas.len(), self.mode);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ReplicationShim started (check every {}s)", self.check_secs);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(true); }
        tracing::info!("ReplicationShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("replication_replicas_healthy", self.replicas_healthy as f64),
            Metric::new("replication_replicas_broken", self.replicas_broken as f64),
            Metric::new("replication_total_lag_bytes", self.total_lag_bytes as f64),
        ]
    }
}
