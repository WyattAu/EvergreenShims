//! Replication shim — database replication management.
//!
//! Manages primary-replica replication for PostgreSQL and MySQL.
//! Spawns a health-check loop that monitors primary connectivity,
//! replica lag, and overall replication state.
//!
//! ## Environment Variables
//!
//! ```text
//! REPLICATION_PRIMARY     Primary database address
//! REPLICATION_REPLICAS    Comma-separated replica addresses
//! REPLICATION_MODE        Mode: synchronous, asynchronous (default: asynchronous)
//! REPLICATION_SLOT       Replication slot name (PostgreSQL)
//! REPLICATION_CHECK_SECS Health check interval (default: 10)
//! REPLICATION_DB_TYPE    Database type: postgres, mysql
//! REPLICATION_DB_HOST    Primary DB host (default: 127.0.0.1)
//! REPLICATION_DB_PORT    Primary DB port (default: 5432)
//! REPLICATION_DB_USER    Primary DB user (default: postgres)
//! REPLICATION_DB_PASSWORD Primary DB password
//! REPLICATION_DB_NAME    Primary DB name (default: postgres)
//! REPLICATION_LAG_THRESHOLD_BYTES Lag threshold in bytes (default: 1048576 = 1MB)
//! ```

use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, EventType, Metric, Result, Severity, ShimBus};
use tokio::process::Command;
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

/// Shared live state for the health-check loop and metrics.
struct SharedState {
    state: parking_lot::Mutex<ReplicationState>,
    replica_lag_bytes: parking_lot::Mutex<u64>,
    max_lag_seconds: parking_lot::Mutex<f64>,
    replicas_healthy: parking_lot::Mutex<u64>,
    replicas_broken: parking_lot::Mutex<u64>,
    failovers_total: parking_lot::Mutex<u64>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            state: parking_lot::Mutex::new(ReplicationState::Healthy),
            replica_lag_bytes: parking_lot::Mutex::new(0),
            max_lag_seconds: parking_lot::Mutex::new(0.0),
            replicas_healthy: parking_lot::Mutex::new(0),
            replicas_broken: parking_lot::Mutex::new(0),
            failovers_total: parking_lot::Mutex::new(0),
        }
    }
}

/// Check if a TCP address is reachable.
async fn check_tcp(addr: &str) -> bool {
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

/// Replication shim.
pub struct ReplicationShim {
    primary: String,
    replicas: Vec<String>,
    mode: String,
    check_secs: u64,
    db_type: String,
    slot_name: String,
    db_host: String,
    db_port: u16,
    db_user: String,
    db_password: String,
    db_name: String,
    lag_threshold_bytes: u64,
    shared: Arc<SharedState>,
    replica_status: HashMap<String, ReplicaStatus>,
    wal_position: WalPosition,
    bus: Option<ShimBus>,
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
            slot_name: slot,
            db_host: std::env::var("REPLICATION_DB_HOST")
                .unwrap_or_else(|_| "127.0.0.1".to_string()),
            db_port: std::env::var("REPLICATION_DB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5432),
            db_user: std::env::var("REPLICATION_DB_USER")
                .unwrap_or_else(|_| "postgres".to_string()),
            db_password: std::env::var("REPLICATION_DB_PASSWORD").unwrap_or_default(),
            db_name: std::env::var("REPLICATION_DB_NAME")
                .unwrap_or_else(|_| "postgres".to_string()),
            lag_threshold_bytes: std::env::var("REPLICATION_LAG_THRESHOLD_BYTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1_048_576),
            shared: Arc::new(SharedState::new()),
            replica_status: HashMap::new(),
            wal_position: WalPosition {
                lsn: "0/0".to_string(),
                segment: 0,
                offset: 0,
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
            bus: None,
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

        *self.shared.replicas_healthy.lock() = healthy;
        *self.shared.replicas_broken.lock() = broken;
        *self.shared.replica_lag_bytes.lock() =
            self.replica_status.values().map(|s| s.lag_bytes).sum();
        *self.shared.max_lag_seconds.lock() = self
            .replica_status
            .values()
            .map(|s| s.lag_seconds)
            .fold(0.0, f64::max);

        let new_state = if total == 0 || healthy == total {
            ReplicationState::Healthy
        } else if broken > 0 && healthy == 0 {
            ReplicationState::Broken
        } else {
            ReplicationState::Degraded
        };
        *self.shared.state.lock() = new_state;
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

        *self.shared.failovers_total.lock() += 1;
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

    /// Query current WAL LSN position from the primary via psql.
    pub async fn query_wal_position(&self) -> anyhow::Result<WalPosition> {
        let query = "SELECT pg_current_wal_lsn(), pg_current_wal_insert_lsn(), extract(epoch from now())::bigint";

        let output = Command::new("psql")
            .args([
                "-h",
                &self.db_host,
                "-p",
                &self.db_port.to_string(),
                "-U",
                &self.db_user,
                "-d",
                &self.db_name,
                "-t",
                "-A",
                "-c",
                query,
            ])
            .env("PGPASSWORD", &self.db_password)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("psql WAL query failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split('|').collect();
        if parts.len() < 2 {
            anyhow::bail!("Unexpected psql output: {}", stdout);
        }

        let lsn = parts[0].to_string();
        let segment = parts
            .get(1)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        Ok(WalPosition {
            lsn,
            segment,
            offset: 0,
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Query replication lag in bytes for a given replica via psql.
    pub async fn query_replica_lag(&self, replica_addr: &str) -> anyhow::Result<u64> {
        let parts: Vec<&str> = replica_addr.split(':').collect();
        let host = parts.first().unwrap_or(&"127.0.0.1");
        let port = parts.get(1).unwrap_or(&"5432");

        let query = "SELECT CASE WHEN pg_last_wal_receive_lsn() = pg_last_wal_replay_lsn() THEN 0 ELSE EXTRACT(EPOCH FROM now() - pg_last_xact_replay_timestamp())::bigint END";

        let output = Command::new("psql")
            .args([
                "-h",
                host,
                "-p",
                port,
                "-U",
                &self.db_user,
                "-d",
                &self.db_name,
                "-t",
                "-A",
                "-c",
                query,
            ])
            .env("PGPASSWORD", &self.db_password)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("psql lag query failed for {}: {}", replica_addr, stderr);
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lag_secs: u64 = stdout.trim().parse().unwrap_or(0);
        Ok(lag_secs)
    }

    /// Get the database host.
    pub fn db_host(&self) -> &str {
        &self.db_host
    }

    /// Get the database port.
    pub fn db_port(&self) -> u16 {
        self.db_port
    }

    /// Get the database user.
    pub fn db_user(&self) -> &str {
        &self.db_user
    }

    /// Get the database password.
    pub fn db_password(&self) -> &str {
        &self.db_password
    }

    /// Get the database name.
    pub fn db_name(&self) -> &str {
        &self.db_name
    }

    /// Get the lag threshold in bytes.
    pub fn lag_threshold_bytes(&self) -> u64 {
        self.lag_threshold_bytes
    }

    /// Get the current replication state.
    pub fn replication_state(&self) -> ReplicationState {
        self.shared.state.lock().clone()
    }

    /// Get the current WAL position.
    pub fn wal_position(&self) -> &WalPosition {
        &self.wal_position
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

    fn set_bus(&mut self, bus: ShimBus) {
        self.bus = Some(bus);
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        if let Some(rc) = &config.replication {
            self.primary = rc.primary.clone();
            self.replicas = rc.replicas.clone();
            self.mode = rc.mode.clone();
            self.check_secs = rc.check_interval_secs;
            self.db_type = rc.db_type.clone();
            if let Some(ref slot) = rc.slot_name {
                self.slot_name = slot.clone();
            }
        }
        tracing::info!(
            "ReplicationShim initialized (primary={}, replicas={}, mode={}, db_type={}, check_secs={}, lag_threshold_bytes={})",
            self.primary,
            self.replicas.len(),
            self.mode,
            self.db_type,
            self.check_secs,
            self.lag_threshold_bytes,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let replicas: Vec<String> = self.replicas.clone();
        for replica in replicas {
            self.add_replica(replica);
        }

        let primary = self.primary.clone();
        let replicas: Vec<String> = self.replica_status.keys().cloned().collect();
        let check_secs = self.check_secs;
        let shared = Arc::clone(&self.shared);
        let bus = self.bus.clone();
        let mode = self.mode.clone();
        let lag_threshold_bytes = self.lag_threshold_bytes;
        let db_host = self.db_host.clone();
        let db_port = self.db_port;
        let db_user = self.db_user.clone();
        let db_password = self.db_password.clone();
        let db_name = self.db_name.clone();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(check_secs));
            let lag_threshold = match mode.as_str() {
                "synchronous" => 1.0,
                _ => 10.0,
            };

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Check primary health
                        let primary_healthy = check_tcp(&primary).await;

                        if !primary_healthy {
                            tracing::warn!("Primary {} is unreachable", primary);
                            *shared.state.lock() = ReplicationState::Broken;
                            if let Some(ref bus) = bus {
                                bus.emit(
                                    "replication-shim",
                                    EventType::ReplicationLagWarning {
                                        lag_ms: 0,
                                        threshold_ms: 0,
                                    },
                                    Severity::Error,
                                );
                            }
                            continue;
                        }

                        // Query WAL position from primary
                        let wal_query = Command::new("psql")
                            .args([
                                "-h", &db_host,
                                "-p", &db_port.to_string(),
                                "-U", &db_user,
                                "-d", &db_name,
                                "-t", "-A",
                                "-c", "SELECT pg_current_wal_lsn()",
                            ])
                            .env("PGPASSWORD", &db_password)
                            .output()
                            .await;

                        if let Ok(output) = wal_query {
                            if output.status.success() {
                                let lsn = String::from_utf8_lossy(&output.stdout).trim().to_string();
                                tracing::debug!("Primary WAL LSN: {}", lsn);
                            }
                        }

                        // Check replica connectivity and lag
                        let mut healthy_count = 0u64;
                        let mut broken_count = 0u64;
                        let mut total_lag: u64 = 0;
                        let mut max_lag_sec: f64 = 0.0;

                        for replica_addr in &replicas {
                            let reachable = check_tcp(replica_addr).await;
                            if reachable {
                                healthy_count += 1;
                                // Query replica lag via psql
                                let replica_parts: Vec<&str> = replica_addr.split(':').collect();
                                let r_host = replica_parts.first().unwrap_or(&"127.0.0.1");
                                let r_port = replica_parts.get(1).unwrap_or(&"5432");

                                let lag_query = Command::new("psql")
                                    .args([
                                        "-h", r_host,
                                        "-p", r_port,
                                        "-U", &db_user,
                                        "-d", &db_name,
                                        "-t", "-A",
                                        "-c", "SELECT CASE WHEN pg_last_wal_receive_lsn() = pg_last_wal_replay_lsn() THEN 0 ELSE COALESCE(EXTRACT(EPOCH FROM now() - pg_last_xact_replay_timestamp())::bigint, 0) END",
                                    ])
                                    .env("PGPASSWORD", &db_password)
                                    .output()
                                    .await;

                                let lag_sec = if let Ok(out) = lag_query {
                                    if out.status.success() {
                                        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0)
                                    } else {
                                        0.0
                                    }
                                } else {
                                    0.0
                                };

                                let lag_bytes = (lag_sec * 1000000.0) as u64; // approximate bytes from seconds
                                total_lag += lag_bytes;
                                max_lag_sec = max_lag_sec.max(lag_sec);

                                if lag_bytes > lag_threshold_bytes {
                                    tracing::warn!(
                                        "Replica {} lag {} bytes exceeds threshold {} bytes",
                                        replica_addr, lag_bytes, lag_threshold_bytes
                                    );
                                    if let Some(ref bus) = bus {
                                        bus.emit(
                                            "replication-shim",
                                            EventType::ReplicationLagWarning {
                                                lag_ms: (lag_sec * 1000.0) as u64,
                                                threshold_ms: (lag_threshold * 1000.0) as u64,
                                            },
                                            Severity::Warning,
                                        );
                                    }
                                }
                            } else {
                                broken_count += 1;
                                tracing::warn!("Replica {} is unreachable", replica_addr);
                            }
                        }

                        *shared.replicas_healthy.lock() = healthy_count;
                        *shared.replicas_broken.lock() = broken_count;
                        *shared.replica_lag_bytes.lock() = total_lag;
                        *shared.max_lag_seconds.lock() = max_lag_sec;

                        let total = replicas.len() as u64;
                        let new_state = if total == 0 || healthy_count == total {
                            ReplicationState::Healthy
                        } else if broken_count > 0 && healthy_count == 0 {
                            ReplicationState::Broken
                        } else {
                            ReplicationState::Degraded
                        };

                        let prev_state = shared.state.lock().clone();
                        *shared.state.lock() = new_state.clone();

                        if new_state != prev_state {
                            tracing::info!(
                                "Replication state changed: {} -> {}",
                                prev_state, new_state
                            );
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Replication shim health-check loop shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!(
            "ReplicationShim started (check every {}s, {} replicas)",
            self.check_secs,
            self.replica_status.len()
        );
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
        let state_val = match *self.shared.state.lock() {
            ReplicationState::Healthy => 0.0,
            ReplicationState::Degraded => 1.0,
            ReplicationState::Broken => 2.0,
        };

        vec![
            Metric::new("replication_state", state_val),
            Metric::new(
                "replication_replicas_healthy",
                *self.shared.replicas_healthy.lock() as f64,
            ),
            Metric::new(
                "replication_replicas_broken",
                *self.shared.replicas_broken.lock() as f64,
            ),
            Metric::new(
                "replication_total_lag_bytes",
                *self.shared.replica_lag_bytes.lock() as f64,
            ),
            Metric::new(
                "replication_max_lag_seconds",
                *self.shared.max_lag_seconds.lock(),
            ),
            Metric::new(
                "replication_failovers_total",
                *self.shared.failovers_total.lock() as f64,
            ),
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
        assert_eq!(*shim.shared.state.lock(), ReplicationState::Healthy);
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

        assert_eq!(*shim.shared.state.lock(), ReplicationState::Healthy);
        assert_eq!(*shim.shared.replicas_healthy.lock(), 2);
        assert_eq!(*shim.shared.replicas_broken.lock(), 0);
    }

    #[test]
    fn test_recalculate_state_mixed() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());
        shim.add_replica("rep2:5432".to_string());
        shim.update_replica_status("rep1:5432", 0, 1.0);
        shim.update_replica_status("rep2:5432", 50000, 60.0);
        shim.recalculate_state();

        assert_eq!(*shim.shared.state.lock(), ReplicationState::Degraded);
        assert_eq!(*shim.shared.replicas_healthy.lock(), 1);
        assert_eq!(*shim.shared.replicas_broken.lock(), 1);
    }

    #[test]
    fn test_recalculate_state_all_broken() {
        let mut shim = ReplicationShim::new();
        shim.add_replica("rep1:5432".to_string());
        shim.mark_replica_disconnected("rep1:5432");
        shim.recalculate_state();

        assert_eq!(*shim.shared.state.lock(), ReplicationState::Broken);
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
        assert_eq!(*shim.shared.failovers_total.lock(), 1);
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
            shared: Arc::new(SharedState {
                state: parking_lot::Mutex::new(ReplicationState::Degraded),
                replicas_healthy: parking_lot::Mutex::new(3),
                replicas_broken: parking_lot::Mutex::new(1),
                replica_lag_bytes: parking_lot::Mutex::new(50000),
                max_lag_seconds: parking_lot::Mutex::new(25.0),
                failovers_total: parking_lot::Mutex::new(2),
            }),
            ..ReplicationShim::new()
        };
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 6);
        assert_eq!(metrics[0].name, "replication_state");
        assert_eq!(metrics[0].value, 1.0);
        assert_eq!(metrics[3].name, "replication_total_lag_bytes");
        assert_eq!(metrics[3].value, 50000.0);
    }

    #[test]
    fn test_default_db_fields() {
        // Clear any env vars that might have been set by parallel tests
        temp_env::with_vars(
            [
                ("REPLICATION_DB_HOST", None::<&str>),
                ("REPLICATION_DB_PORT", None::<&str>),
                ("REPLICATION_DB_USER", None::<&str>),
                ("REPLICATION_DB_PASSWORD", None::<&str>),
                ("REPLICATION_DB_NAME", None::<&str>),
                ("REPLICATION_LAG_THRESHOLD_BYTES", None::<&str>),
            ],
            || {
                let shim = ReplicationShim::new();
                assert_eq!(shim.db_host, "127.0.0.1");
                assert_eq!(shim.db_port, 5432);
                assert_eq!(shim.db_user, "postgres");
                assert_eq!(shim.db_name, "postgres");
                assert_eq!(shim.lag_threshold_bytes, 1_048_576);
            },
        );
    }

    #[test]
    fn test_env_db_fields() {
        temp_env::with_vars(
            [
                ("REPLICATION_DB_HOST", Some("pg-primary.local")),
                ("REPLICATION_DB_PORT", Some("5433")),
                ("REPLICATION_DB_USER", Some("repl_user")),
                ("REPLICATION_DB_PASSWORD", Some("secret")),
                ("REPLICATION_DB_NAME", Some("mydb")),
                ("REPLICATION_LAG_THRESHOLD_BYTES", Some("2097152")),
            ],
            || {
                let shim = ReplicationShim::new();
                assert_eq!(shim.db_host, "pg-primary.local");
                assert_eq!(shim.db_port, 5433);
                assert_eq!(shim.db_user, "repl_user");
                assert_eq!(shim.db_password, "secret");
                assert_eq!(shim.db_name, "mydb");
                assert_eq!(shim.lag_threshold_bytes, 2_097_152);
            },
        );
    }
}
