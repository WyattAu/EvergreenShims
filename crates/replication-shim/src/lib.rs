#![allow(dead_code)]
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

use std::collections::HashMap;

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

impl std::fmt::Display for ReplicationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Degraded => write!(f, "degraded"),
            Self::Broken => write!(f, "broken"),
        }
    }
}

/// Replica status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicaStatus {
    pub addr: String,
    pub state: ReplicationState,
    pub lag_bytes: u64,
    pub lag_seconds: f64,
    pub last_heartbeat: String,
    pub connected: bool,
}

/// WAL position tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalPosition {
    pub lsn: String,
    pub segment: u64,
    pub offset: u64,
    pub timestamp: String,
}

/// Failover result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverResult {
    pub old_primary: String,
    pub new_primary: String,
    pub promoted_at: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Replication shim.
pub struct ReplicationShim {
    primary: String,
    replicas: Vec<String>,
    mode: String,
    check_secs: u64,
    db_type: String,
    state: ReplicationState,
    slot_name: String,
    replica_status: HashMap<String, ReplicaStatus>,
    wal_position: WalPosition,
    replicas_healthy: u64,
    replicas_broken: u64,
    total_lag_bytes: u64,
    max_lag_seconds: f64,
    failovers_total: u64,
    last_check: Option<chrono::DateTime<chrono::Utc>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ReplicationShim {
    pub fn new() -> Self {
        let slot =
            std::env::var("REPLICATION_SLOT").unwrap_or_else(|_| "evergreen_shim".to_string());
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
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
            db_type: std::env::var("REPLICATION_DB_TYPE")
                .unwrap_or_else(|_| "postgres".to_string()),
            state: ReplicationState::Healthy,
            slot_name: slot,
            replica_status: HashMap::new(),
            wal_position: WalPosition {
                lsn: "0/0".to_string(),
                segment: 0,
                offset: 0,
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
            replicas_healthy: 0,
            replicas_broken: 0,
            total_lag_bytes: 0,
            max_lag_seconds: 0.0,
            failovers_total: 0,
            last_check: None,
            shutdown_tx: None,
        }
    }

    /// Register a replica with its address.
    pub fn add_replica(&mut self, addr: String) {
        let status = ReplicaStatus {
            addr: addr.clone(),
            state: ReplicationState::Healthy,
            lag_bytes: 0,
            lag_seconds: 0.0,
            last_heartbeat: chrono::Utc::now().to_rfc3339(),
            connected: false,
        };
        let addr_clone = addr.clone();
        self.replica_status.insert(addr, status);
        if !self.replicas.contains(&addr_clone) {
            self.replicas.push(addr_clone);
        }
    }

    /// Update the status of a replica.
    pub fn update_replica_status(&mut self, addr: &str, lag_bytes: u64, lag_seconds: f64) -> bool {
        if let Some(status) = self.replica_status.get_mut(addr) {
            status.lag_bytes = lag_bytes;
            status.lag_seconds = lag_seconds;
            status.last_heartbeat = chrono::Utc::now().to_rfc3339();

            let prev_state = status.state.clone();
            if lag_seconds > 30.0 {
                status.state = ReplicationState::Broken;
            } else if lag_seconds > 10.0 {
                status.state = ReplicationState::Degraded;
            } else {
                status.state = ReplicationState::Healthy;
                status.connected = true;
            }

            status.state != prev_state
        } else {
            false
        }
    }

    /// Mark a replica as disconnected.
    pub fn mark_replica_disconnected(&mut self, addr: &str) {
        if let Some(status) = self.replica_status.get_mut(addr) {
            status.connected = false;
            status.state = ReplicationState::Broken;
        }
    }

    /// Recalculate overall replication state from individual replica states.
    pub fn recalculate_state(&mut self) {
        let total = self.replica_status.len() as u64;
        let healthy = self
            .replica_status
            .values()
            .filter(|s| s.state == ReplicationState::Healthy)
            .count() as u64;
        let broken = self
            .replica_status
            .values()
            .filter(|s| s.state == ReplicationState::Broken)
            .count() as u64;

        self.replicas_healthy = healthy;
        self.replicas_broken = broken;
        self.total_lag_bytes = self.replica_status.values().map(|s| s.lag_bytes).sum();
        self.max_lag_seconds = self
            .replica_status
            .values()
            .map(|s| s.lag_seconds)
            .fold(0.0, f64::max);

        self.state = if total == 0 {
            ReplicationState::Healthy
        } else if healthy == total {
            ReplicationState::Healthy
        } else if broken > 0 && healthy == 0 {
            ReplicationState::Broken
        } else {
            ReplicationState::Degraded
        };

        self.last_check = Some(chrono::Utc::now());
    }

    /// Update WAL position.
    pub fn set_wal_position(&mut self, lsn: &str, segment: u64, offset: u64) {
        self.wal_position = WalPosition {
            lsn: lsn.to_string(),
            segment,
            offset,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
    }

    /// Promote a replica to primary (failover).
    pub fn promote(&mut self, replica_addr: &str) -> anyhow::Result<FailoverResult> {
        if !self.replica_status.contains_key(replica_addr) {
            anyhow::bail!("Replica {} not found", replica_addr);
        }

        let old_primary = self.primary.clone();
        let new_primary = replica_addr.to_string();

        self.replica_status.remove(&new_primary);
        self.primary = new_primary.clone();

        let result = FailoverResult {
            old_primary,
            new_primary: new_primary.clone(),
            promoted_at: chrono::Utc::now().to_rfc3339(),
            success: true,
            error: None,
        };

        self.failovers_total += 1;
        self.recalculate_state();

        Ok(result)
    }

    /// Get replica count.
    pub fn replica_count(&self) -> usize {
        self.replica_status.len()
    }

    /// Get status of a specific replica.
    pub fn get_replica(&self, addr: &str) -> Option<&ReplicaStatus> {
        self.replica_status.get(addr)
    }

    /// Check if all replicas are healthy.
    pub fn all_healthy(&self) -> bool {
        self.replica_status
            .values()
            .all(|s| s.state == ReplicationState::Healthy)
    }

    /// Get lag threshold warning (sync mode = 0, async = 10s).
    pub fn lag_threshold(&self) -> f64 {
        match self.mode.as_str() {
            "synchronous" => 1.0,
            _ => 10.0,
        }
    }
}

impl Default for ReplicationShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ReplicationShim {
    fn name(&self) -> &str {
        "replication"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "ReplicationShim initialized (primary={}, replicas={}, mode={})",
            self.primary,
            self.replicas.len(),
            self.mode
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let replicas: Vec<String> = self.replicas.clone();
        for replica in replicas {
            self.add_replica(replica);
        }

        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ReplicationShim started (check every {}s)", self.check_secs);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("ReplicationShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let state_val = match self.state {
            ReplicationState::Healthy => 0.0,
            ReplicationState::Degraded => 1.0,
            ReplicationState::Broken => 2.0,
        };

        vec![
            Metric::new("replication_state", state_val),
            Metric::new("replication_replicas_healthy", self.replicas_healthy as f64),
            Metric::new("replication_replicas_broken", self.replicas_broken as f64),
            Metric::new("replication_total_lag_bytes", self.total_lag_bytes as f64),
            Metric::new("replication_max_lag_seconds", self.max_lag_seconds),
            Metric::new("replication_failovers_total", self.failovers_total as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let shim = ReplicationShim::new();
        assert_eq!(shim.mode, "asynchronous");
        assert_eq!(shim.db_type, "postgres");
        assert_eq!(shim.check_secs, 10);
        assert_eq!(shim.state, ReplicationState::Healthy);
    }

    #[test]
    fn test_add_replica() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("replica1:5432".to_string());
        shim.add_replica("replica2:5432".to_string());

        assert_eq!(shim.replica_count(), 2);
        assert!(shim.get_replica("replica1:5432").is_some());
        assert!(shim.get_replica("replica2:5432").is_some());
    }

    #[test]
    fn test_update_replica_status_healthy() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());

        let changed = shim.update_replica_status("rep1:5432", 100, 2.0);
        assert!(!changed);

        let status = shim.get_replica("rep1:5432").unwrap();
        assert_eq!(status.state, ReplicationState::Healthy);
        assert_eq!(status.lag_bytes, 100);
        assert!(status.connected);
    }

    #[test]
    fn test_update_replica_status_changes_state() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());

        let changed = shim.update_replica_status("rep1:5432", 50000, 60.0);
        assert!(changed);

        let status = shim.get_replica("rep1:5432").unwrap();
        assert_eq!(status.state, ReplicationState::Broken);
    }

    #[test]
    fn test_update_replica_status_degraded() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());

        shim.update_replica_status("rep1:5432", 5000, 15.0);
        let status = shim.get_replica("rep1:5432").unwrap();
        assert_eq!(status.state, ReplicationState::Degraded);
    }

    #[test]
    fn test_update_replica_status_broken() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());

        shim.update_replica_status("rep1:5432", 100000, 60.0);
        let status = shim.get_replica("rep1:5432").unwrap();
        assert_eq!(status.state, ReplicationState::Broken);
    }

    #[test]
    fn test_mark_replica_disconnected() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());

        shim.mark_replica_disconnected("rep1:5432");
        let status = shim.get_replica("rep1:5432").unwrap();
        assert!(!status.connected);
        assert_eq!(status.state, ReplicationState::Broken);
    }

    #[test]
    fn test_recalculate_state_all_healthy() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());
        shim.add_replica("rep2:5432".to_string());
        shim.update_replica_status("rep1:5432", 0, 1.0);
        shim.update_replica_status("rep2:5432", 0, 2.0);
        shim.recalculate_state();

        assert_eq!(shim.state, ReplicationState::Healthy);
        assert_eq!(shim.replicas_healthy, 2);
        assert_eq!(shim.replicas_broken, 0);
    }

    #[test]
    fn test_recalculate_state_mixed() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());
        shim.add_replica("rep2:5432".to_string());
        shim.update_replica_status("rep1:5432", 0, 1.0);
        shim.update_replica_status("rep2:5432", 50000, 60.0);
        shim.recalculate_state();

        assert_eq!(shim.state, ReplicationState::Degraded);
        assert_eq!(shim.replicas_healthy, 1);
        assert_eq!(shim.replicas_broken, 1);
    }

    #[test]
    fn test_recalculate_state_all_broken() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());
        shim.mark_replica_disconnected("rep1:5432");
        shim.recalculate_state();

        assert_eq!(shim.state, ReplicationState::Broken);
    }

    #[test]
    fn test_set_wal_position() {
        let mut shim = ReplicationShim::new();
        shim.set_wal_position("0/16B8A48", 1, 1024);

        assert_eq!(shim.wal_position.lsn, "0/16B8A48");
        assert_eq!(shim.wal_position.segment, 1);
        assert_eq!(shim.wal_position.offset, 1024);
    }

    #[test]
    fn test_promote_replica() {
        let mut shim = ReplicationShim {
            primary: "primary:5432".to_string(),
            ..ReplicationShim::new()
        };
        shim.add_replica("rep1:5432".to_string());

        let result = shim.promote("rep1:5432").unwrap();
        assert_eq!(result.old_primary, "primary:5432");
        assert_eq!(result.new_primary, "rep1:5432");
        assert!(result.success);
        assert_eq!(shim.primary, "rep1:5432");
        assert_eq!(shim.failovers_total, 1);
        assert_eq!(shim.replica_count(), 0);
    }

    #[test]
    fn test_promote_nonexistent_replica() {
        let mut shim = ReplicationShim::new();
        let result = shim.promote("nonexistent:5432");
        assert!(result.is_err());
    }

    #[test]
    fn test_all_healthy() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());
        shim.add_replica("rep2:5432".to_string());
        shim.update_replica_status("rep1:5432", 0, 1.0);
        shim.update_replica_status("rep2:5432", 0, 1.0);

        assert!(shim.all_healthy());
    }

    #[test]
    fn test_lag_threshold_sync_vs_async() {
        let sync_shim = ReplicationShim {
            mode: "synchronous".to_string(),
            ..ReplicationShim::new()
        };
        assert_eq!(sync_shim.lag_threshold(), 1.0);

        let async_shim = ReplicationShim {
            mode: "asynchronous".to_string(),
            ..ReplicationShim::new()
        };
        assert_eq!(async_shim.lag_threshold(), 10.0);
    }

    #[test]
    fn test_replication_state_display() {
        assert_eq!(ReplicationState::Healthy.to_string(), "healthy");
        assert_eq!(ReplicationState::Degraded.to_string(), "degraded");
        assert_eq!(ReplicationState::Broken.to_string(), "broken");
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = ReplicationShim {
            replicas_healthy: 3,
            replicas_broken: 1,
            total_lag_bytes: 50000,
            max_lag_seconds: 25.0,
            state: ReplicationState::Degraded,
            failovers_total: 2,
            ..ReplicationShim::new()
        };
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 6);
        assert_eq!(metrics[0].name, "replication_state");
        assert_eq!(metrics[0].value, 1.0);
        assert_eq!(metrics[3].name, "replication_total_lag_bytes");
        assert_eq!(metrics[3].value, 50000.0);
    }
}
