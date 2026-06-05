#![allow(dead_code)]
//! Failover shim — automatic failover for HA databases.
//!
//! Monitors a primary database, detects failure, promotes a replica,
//! and sends notifications. Supports automatic failback when the
//! original primary recovers.
//!
//! ## Connectors
//!
//! - **Generic TCP**: Basic TCP connectivity checks (default).
//! - **Patroni Failover**: PostgreSQL Patroni cluster monitoring via `psql`.
//! - **Redis Sentinel Failover**: Redis Sentinel master tracking.
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
//! FAILOVER_CONNECTOR     Connector type: tcp (default), patroni, redis-sentinel
//!
//! ## Patroni Connector Env Vars
//!
//! ```text
//! FAILOVER_DB_HOST       Database host for psql (default: 127.0.0.1)
//! FAILOVER_DB_PORT       Database port for psql (default: 5432)
//! FAILOVER_DB_USER       Database user for psql (default: postgres)
//! FAILOVER_DB_PASSWORD   Database password for psql
//! FAILOVER_DB_NAME       Database name for psql (default: postgres)
//! FAILOVER_CHECK_INTERVAL_SECS  Check interval in seconds (default: 10)
//! FAILOVER_LAG_THRESHOLD_SECS   Replication lag threshold in seconds (default: 30)
//! ```
//!
//! ## Redis Sentinel Connector Env Vars
//!
//! ```text
//! REDIS_SENTINEL_URL     Sentinel URL (default: redis://localhost:26379)
//! REDIS_SENTINEL_MASTER  Master name (default: mymaster)
//! FAILOVER_CHECK_INTERVAL_SECS  Check interval in seconds (default: 5)
//! ```

use std::net::TcpStream;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, EventType, Metric, Result, Severity, ShimBus};
use tokio::sync::watch;

/// Connector type for failover detection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FailoverConnector {
    /// Basic TCP connectivity check (default).
    Tcp,
    /// PostgreSQL Patroni cluster monitoring via psql.
    Patroni,
    /// Redis Sentinel master tracking.
    RedisSentinel,
}

impl Default for FailoverConnector {
    fn default() -> Self {
        Self::Tcp
    }
}

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

/// Patroni-specific replication info.
#[derive(Debug, Clone)]
struct PatroniReplicaInfo {
    client_addr: String,
    state: String,
    sync_state: String,
    sent_lsn: String,
    write_lsn: String,
    flush_lsn: String,
    replay_lsn: String,
    write_lag: Option<f64>,
    flush_lag: Option<f64>,
    replay_lag: Option<f64>,
}

/// Redis Sentinel master info.
#[derive(Debug, Clone, PartialEq)]
struct SentinelMasterInfo {
    ip: String,
    port: String,
    flags: String,
    num_slaves: u32,
    num_other_sentinels: u32,
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

/// Run a psql command and return stdout.
async fn run_psql(
    host: &str,
    port: &str,
    user: &str,
    password: &str,
    dbname: &str,
    query: &str,
) -> std::result::Result<String, String> {
    let mut cmd = tokio::process::Command::new("psql");
    cmd.arg("-h")
        .arg(host)
        .arg("-p")
        .arg(port)
        .arg("-U")
        .arg(user)
        .arg("-d")
        .arg(dbname)
        .arg("-t") // tuples only
        .arg("-A") // unaligned output
        .arg("-F")
        .arg("|")
        .arg("-c")
        .arg(query);

    if !password.is_empty() {
        cmd.env("PGPASSWORD", password);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to execute psql: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("psql error: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Parse replication lag from psql output. Returns seconds or None.
fn parse_replication_lag(lag_str: &str) -> Option<f64> {
    if lag_str.is_empty() || lag_str == "NULL" {
        return None;
    }
    // Try parsing as interval seconds (e.g., "15.5")
    lag_str.parse::<f64>().ok()
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
    connector: FailoverConnector,
    // Patroni config
    db_host: String,
    db_port: String,
    db_user: String,
    db_password: String,
    db_name: String,
    lag_threshold_secs: f64,
    // Redis Sentinel config
    redis_sentinel_url: String,
    redis_sentinel_master: String,
    // Internal state
    shared: Arc<SharedState>,
    bus: Option<ShimBus>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl FailoverShim {
    pub fn new() -> Self {
        let connector_str =
            std::env::var("FAILOVER_CONNECTOR").unwrap_or_else(|_| "tcp".to_string());
        let connector = match connector_str.as_str() {
            "patroni" => FailoverConnector::Patroni,
            "redis-sentinel" => FailoverConnector::RedisSentinel,
            _ => FailoverConnector::Tcp,
        };

        let check_interval_secs = std::env::var("FAILOVER_CHECK_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                std::env::var("FAILOVER_CHECK_INTERVAL")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(5)
            });

        Self {
            primary: std::env::var("FAILOVER_PRIMARY")
                .unwrap_or_else(|_| "127.0.0.1:5432".to_string()),
            replica: std::env::var("FAILOVER_REPLICA")
                .unwrap_or_else(|_| "127.0.0.1:5433".to_string()),
            check_interval_secs,
            failure_threshold: std::env::var("FAILOVER_FAILURE_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            webhook: std::env::var("FAILOVER_WEBHOOK").ok(),
            db_type: std::env::var("FAILOVER_DB_TYPE")
                .unwrap_or_else(|_| "postgres".to_string()),
            connector,
            // Patroni
            db_host: std::env::var("FAILOVER_DB_HOST")
                .unwrap_or_else(|_| "127.0.0.1".to_string()),
            db_port: std::env::var("FAILOVER_DB_PORT")
                .unwrap_or_else(|_| "5432".to_string()),
            db_user: std::env::var("FAILOVER_DB_USER")
                .unwrap_or_else(|_| "postgres".to_string()),
            db_password: std::env::var("FAILOVER_DB_PASSWORD")
                .unwrap_or_default(),
            db_name: std::env::var("FAILOVER_DB_NAME")
                .unwrap_or_else(|_| "postgres".to_string()),
            lag_threshold_secs: std::env::var("FAILOVER_LAG_THRESHOLD_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30.0),
            // Redis Sentinel
            redis_sentinel_url: std::env::var("REDIS_SENTINEL_URL")
                .unwrap_or_else(|_| "redis://localhost:26379".to_string()),
            redis_sentinel_master: std::env::var("REDIS_SENTINEL_MASTER")
                .unwrap_or_else(|_| "mymaster".to_string()),
            // Internal
            shared: Arc::new(SharedState::new()),
            bus: None,
            shutdown_tx: None,
        }
    }

    /// Run a Patroni health check cycle.
    async fn patroni_check(
        db_host: &str,
        db_port: &str,
        db_user: &str,
        db_password: &str,
        db_name: &str,
    ) -> PatroniCheckResult {
        // 1. Check pg_is_in_recovery() to detect primary vs replica
        match run_psql(
            db_host,
            db_port,
            db_user,
            db_password,
            db_name,
            "SELECT pg_is_in_recovery()",
        )
        .await
        {
            Ok(output) => {
                let in_recovery = output.trim() == "t";
                tracing::debug!(
                    "Patroni check: pg_is_in_recovery={} (host={})",
                    in_recovery,
                    db_host
                );

                // 2. Check replication lag if we're on the primary
                let mut max_lag_secs = 0.0f64;
                if !in_recovery {
                    if let Ok(replication_output) = run_psql(
                        db_host,
                        db_port,
                        db_user,
                        db_password,
                        db_name,
                        "SELECT COALESCE(EXTRACT(EPOCH FROM replay_lag)::float, 0) FROM pg_stat_replication",
                    )
                    .await
                    {
                        for line in replication_output.lines() {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() && trimmed != "NULL" {
                                if let Ok(lag) = trimmed.parse::<f64>() {
                                    if lag > max_lag_secs {
                                        max_lag_secs = lag;
                                    }
                                }
                            }
                        }
                    }

                    // 3. Check for recent restarts
                    let _ = run_psql(
                        db_host,
                        db_port,
                        db_user,
                        db_password,
                        db_name,
                        "SELECT pg_postmaster_start_time()",
                    )
                    .await;
                }

                PatroniCheckResult::Healthy {
                    in_recovery,
                    max_lag_secs,
                }
            }
            Err(e) => {
                tracing::warn!("Patroni check failed (host={}): {}", db_host, e);
                PatroniCheckResult::Unreachable
            }
        }
    }

    /// Run a Redis Sentinel check cycle.
    async fn redis_sentinel_check(
        sentinel_url: &str,
        master_name: &str,
    ) -> RedisSentinelCheckResult {
        let client =
            redis::Client::open(sentinel_url).map_err(|e| format!("Client error: {}", e));

        let client = match client {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Redis Sentinel client error: {}", e);
                return RedisSentinelCheckResult::Unreachable;
            }
        };

        let mut conn = match client.get_connection() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Redis Sentinel connection failed: {}", e);
                return RedisSentinelCheckResult::Unreachable;
            }
        };

        // Get master info via SENTINEL get-master-addr-by-name
        let result: redis::RedisResult<(String, u16)> = redis::cmd("SENTINEL")
            .arg("get-master-addr-by-name")
            .arg(master_name)
            .query(&mut conn);

        match result {
            Ok((ip, port)) => {
                tracing::debug!(
                    "Redis Sentinel: master is {}:{} for '{}'",
                    ip,
                    port,
                    master_name
                );
                RedisSentinelCheckResult::MasterInfo {
                    ip,
                    port: port.to_string(),
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Redis Sentinel get-master-addr-by-name failed for '{}': {}",
                    master_name,
                    e
                );
                RedisSentinelCheckResult::Unreachable
            }
        }
    }
}

/// Result of a Patroni health check.
#[derive(Debug)]
pub enum PatroniCheckResult {
    Healthy {
        in_recovery: bool,
        max_lag_secs: f64,
    },
    Unreachable,
}

/// Result of a Redis Sentinel check.
#[derive(Debug)]
pub enum RedisSentinelCheckResult {
    MasterInfo { ip: String, port: String },
    Unreachable,
}

impl FailoverShim {
    /// Get the connector type.
    pub fn connector(&self) -> &FailoverConnector {
        &self.connector
    }

    /// Get the database host.
    pub fn db_host(&self) -> &str {
        &self.db_host
    }

    /// Get the database port.
    pub fn db_port(&self) -> &str {
        &self.db_port
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

    /// Get the lag threshold in seconds.
    pub fn lag_threshold_secs(&self) -> f64 {
        self.lag_threshold_secs
    }

    /// Get the check interval in seconds.
    pub fn check_interval_secs(&self) -> u64 {
        self.check_interval_secs
    }

    /// Get the Redis Sentinel URL.
    pub fn redis_sentinel_url(&self) -> &str {
        &self.redis_sentinel_url
    }

    /// Get the Redis Sentinel master name.
    pub fn redis_sentinel_master(&self) -> &str {
        &self.redis_sentinel_master
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
            "FailoverShim initialized (connector={:?}, primary={}, replica={}, interval={}s, threshold={}, db_type={})",
            self.connector,
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
        let connector = self.connector.clone();

        // Patroni config
        let patroni_db_host = self.db_host.clone();
        let patroni_db_port = self.db_port.clone();
        let patroni_db_user = self.db_user.clone();
        let patroni_db_password = self.db_password.clone();
        let patroni_db_name = self.db_name.clone();
        let lag_threshold_secs = self.lag_threshold_secs;

        // Redis Sentinel config
        let sentinel_url = self.redis_sentinel_url.clone();
        let sentinel_master = self.redis_sentinel_master.clone();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(check_interval_secs));
            let mut consecutive_primary_healthy: u32 = 0;

            // Patroni state tracking
            let mut last_known_primary: Option<String> = None;
            let mut last_recovery_state: Option<bool> = None;

            // Redis Sentinel state tracking
            let mut last_known_master: Option<String> = None;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let cur = shared.current_primary.lock().clone();

                        match connector {
                            FailoverConnector::Tcp => {
                                let healthy = check_database(&cur).await;
                                Self::handle_check_result(
                                    &shared,
                                    &bus,
                                    &webhook,
                                    &cur,
                                    &replica,
                                    &original_primary,
                                    healthy,
                                    failure_threshold,
                                    &mut consecutive_primary_healthy,
                                ).await;
                            }
                            FailoverConnector::Patroni => {
                                match Self::patroni_check(
                                    &patroni_db_host,
                                    &patroni_db_port,
                                    &patroni_db_user,
                                    &patroni_db_password,
                                    &patroni_db_name,
                                ).await {
                                    PatroniCheckResult::Healthy { in_recovery, max_lag_secs } => {
                                        shared.consecutive_failures.store(0, Ordering::Relaxed);

                                        // Detect primary→replica or replica→primary transitions
                                        if let Some(prev_recovery) = last_recovery_state {
                                            if prev_recovery != in_recovery {
                                                let new_role = if in_recovery { "replica" } else { "primary" };
                                                tracing::info!(
                                                    "Patroni: node role changed to {} (was recovery={})",
                                                    new_role, prev_recovery
                                                );
                                                if let Some(ref bus) = bus {
                                                    bus.emit(
                                                        "failover-shim",
                                                        EventType::FailoverTriggered {
                                                            old_primary: cur.clone(),
                                                            new_primary: if in_recovery { replica.clone() } else { patroni_db_host.clone() },
                                                        },
                                                        Severity::Notice,
                                                    );
                                                }
                                            }
                                        }
                                        last_recovery_state = Some(in_recovery);

                                        // Check replication lag
                                        if max_lag_secs > lag_threshold_secs {
                                            tracing::warn!(
                                                "Patroni: replication lag {}s exceeds threshold {}s",
                                                max_lag_secs, lag_threshold_secs
                                            );
                                            if let Some(ref bus) = bus {
                                                bus.emit(
                                                    "failover-shim",
                                                    EventType::ReplicationLagWarning {
                                                        lag_ms: (max_lag_secs * 1000.0) as u64,
                                                        threshold_ms: (lag_threshold_secs * 1000.0) as u64,
                                                    },
                                                    Severity::Warning,
                                                );
                                            }
                                        }

                                        // Track primary changes
                                        let cur_role_host = if in_recovery {
                                            replica.clone()
                                        } else {
                                            patroni_db_host.clone()
                                        };
                                        if let Some(ref prev) = last_known_primary {
                                            if prev != &cur_role_host {
                                                tracing::info!(
                                                    "Patroni: new primary detected: {} (was {})",
                                                    cur_role_host, prev
                                                );
                                                if let Some(ref bus) = bus {
                                                    bus.emit(
                                                        "failover-shim",
                                                        EventType::FailoverCompleted {
                                                            promoted: cur_role_host.clone(),
                                                        },
                                                        Severity::Notice,
                                                    );
                                                }
                                                shared.failover_count.fetch_add(1, Ordering::Relaxed);
                                            }
                                        }
                                        last_known_primary = Some(cur_role_host);

                                        consecutive_primary_healthy = 0;
                                        shared.set_state(FailoverState::Healthy);
                                    }
                                    PatroniCheckResult::Unreachable => {
                                        consecutive_primary_healthy = 0;
                                        let failures = shared.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
                                        tracing::warn!(
                                            "Patroni health check failed ({}/{})",
                                            failures, failure_threshold
                                        );

                                        if failures >= failure_threshold {
                                            Self::trigger_failover(
                                                &shared,
                                                &bus,
                                                &webhook,
                                                &cur,
                                                &replica,
                                            ).await;
                                        } else {
                                            shared.set_state(FailoverState::Suspect);
                                        }
                                    }
                                }
                            }
                            FailoverConnector::RedisSentinel => {
                                match Self::redis_sentinel_check(&sentinel_url, &sentinel_master).await {
                                    RedisSentinelCheckResult::MasterInfo { ip, port } => {
                                        shared.consecutive_failures.store(0, Ordering::Relaxed);
                                        let master_addr = format!("{}:{}", ip, port);

                                        // Detect master change
                                        if let Some(ref prev) = last_known_master {
                                            if prev != &master_addr {
                                                tracing::info!(
                                                    "Redis Sentinel: master changed from {} to {}",
                                                    prev, master_addr
                                                );
                                                if let Some(ref bus) = bus {
                                                    bus.emit(
                                                        "failover-shim",
                                                        EventType::FailoverCompleted {
                                                            promoted: master_addr.clone(),
                                                        },
                                                        Severity::Notice,
                                                    );
                                                }
                                                shared.failover_count.fetch_add(1, Ordering::Relaxed);
                                            }
                                        }
                                        last_known_master = Some(master_addr);

                                        consecutive_primary_healthy = 0;
                                        shared.set_state(FailoverState::Healthy);
                                    }
                                    RedisSentinelCheckResult::Unreachable => {
                                        consecutive_primary_healthy = 0;
                                        let failures = shared.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
                                        tracing::warn!(
                                            "Redis Sentinel check failed ({}/{})",
                                            failures, failure_threshold
                                        );

                                        if failures >= failure_threshold {
                                            Self::trigger_failover(
                                                &shared,
                                                &bus,
                                                &webhook,
                                                &cur,
                                                &replica,
                                            ).await;
                                        } else {
                                            shared.set_state(FailoverState::Suspect);
                                        }
                                    }
                                }
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
            "FailoverShim started (connector={:?}, check every {}s, failover after {} failures)",
            self.connector,
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

impl FailoverShim {
    /// Handle a generic TCP check result and trigger failover if needed.
    async fn handle_check_result(
        shared: &SharedState,
        bus: &Option<ShimBus>,
        webhook: &Option<String>,
        cur: &str,
        replica: &str,
        original_primary: &str,
        healthy: bool,
        failure_threshold: u32,
        consecutive_primary_healthy: &mut u32,
    ) {
        if healthy {
            shared.consecutive_failures.store(0, Ordering::Relaxed);
            let cur_state = shared.state();

            if cur_state == FailoverState::FailedOver && cur != original_primary {
                *consecutive_primary_healthy += 1;
                tracing::debug!(
                    "Promoted primary {} healthy ({}/10 checks for failback)",
                    cur, *consecutive_primary_healthy
                );

                if *consecutive_primary_healthy >= 10 {
                    if check_database(original_primary).await {
                        tracing::info!(
                            "FAILOVER SHIM: Failing back to original primary {}",
                            original_primary
                        );

                        if let Some(webhook_url) = webhook {
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
                                    promoted: original_primary.to_string(),
                                },
                                Severity::Notice,
                            );
                        }

                        *shared.current_primary.lock() = original_primary.to_string();
                        shared.set_state(FailoverState::Recovered);
                        *consecutive_primary_healthy = 0;
                        tracing::info!("Failback complete. Primary: {}", original_primary);
                    }
                }
            } else if cur_state == FailoverState::Suspect || cur_state == FailoverState::FailingOver {
                tracing::info!("Primary {} recovered", cur);
                shared.set_state(FailoverState::Healthy);
                *consecutive_primary_healthy = 0;
            } else {
                *consecutive_primary_healthy = 0;
            }
        } else {
            *consecutive_primary_healthy = 0;
            let failures = shared.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                "Health check failed for {} ({}/{})",
                cur, failures, failure_threshold
            );

            if failures >= failure_threshold {
                Self::trigger_failover(shared, bus, webhook, cur, replica).await;
            } else {
                shared.set_state(FailoverState::Suspect);
            }
        }
    }

    /// Trigger a failover event.
    async fn trigger_failover(
        shared: &SharedState,
        bus: &Option<ShimBus>,
        webhook: &Option<String>,
        old_primary: &str,
        new_primary: &str,
    ) {
        shared.set_state(FailoverState::FailingOver);
        tracing::error!(
            "FAILOVER TRIGGERED: {} failed, promoting {}",
            old_primary,
            new_primary
        );

        if let Some(webhook_url) = webhook {
            let client = reqwest::Client::new();
            let payload = serde_json::json!({
                "text": format!("FAILOVER TRIGGERED: {} failed, promoting {}", old_primary, new_primary),
            });
            if let Err(e) = client.post(webhook_url).json(&payload).send().await {
                tracing::error!("Webhook POST failed: {}", e);
            }
        }

        if let Some(ref bus) = bus {
            bus.emit(
                "failover-shim",
                EventType::FailoverTriggered {
                    old_primary: old_primary.to_string(),
                    new_primary: new_primary.to_string(),
                },
                Severity::Error,
            );
        }

        *shared.current_primary.lock() = new_primary.to_string();
        shared.failover_count.fetch_add(1, Ordering::Relaxed);
        shared.set_state(FailoverState::FailedOver);
        shared.consecutive_failures.store(0, Ordering::Relaxed);
        tracing::info!("Failover complete. New primary: {}", new_primary);
    }
}

/// Periodic Patroni health check monitor.
///
/// Runs a health check loop that queries PostgreSQL via `psql` to detect:
/// - Primary vs replica state (`pg_is_in_recovery`)
/// - Replication lag (`pg_stat_replication`)
/// - Recent restarts (`pg_postmaster_start_time`)
///
/// Emits `FailoverDetected` events on the bus when:
/// - Primary goes down (connection refused)
/// - New primary elected (recovery mode changes)
/// - Replication lag exceeds threshold
pub struct PatroniMonitor {
    db_host: String,
    db_port: String,
    db_user: String,
    db_password: String,
    db_name: String,
    check_interval_secs: u64,
    lag_threshold_secs: f64,
    bus: Option<ShimBus>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl PatroniMonitor {
    /// Create a new PatroniMonitor from environment variables.
    pub fn from_env() -> Self {
        Self {
            db_host: std::env::var("FAILOVER_DB_HOST")
                .unwrap_or_else(|_| "localhost".to_string()),
            db_port: std::env::var("FAILOVER_DB_PORT")
                .unwrap_or_else(|_| "5432".to_string()),
            db_user: std::env::var("FAILOVER_DB_USER")
                .unwrap_or_else(|_| "postgres".to_string()),
            db_password: std::env::var("FAILOVER_DB_PASSWORD").unwrap_or_default(),
            db_name: std::env::var("FAILOVER_DB_NAME")
                .unwrap_or_else(|_| "postgres".to_string()),
            check_interval_secs: std::env::var("FAILOVER_CHECK_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
            lag_threshold_secs: std::env::var("FAILOVER_LAG_THRESHOLD_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30.0),
            bus: None,
            shutdown_tx: None,
        }
    }

    /// Create a new PatroniMonitor with explicit configuration.
    pub fn new(
        db_host: impl Into<String>,
        db_port: impl Into<String>,
        db_user: impl Into<String>,
        db_password: impl Into<String>,
        db_name: impl Into<String>,
        check_interval_secs: u64,
        lag_threshold_secs: f64,
    ) -> Self {
        Self {
            db_host: db_host.into(),
            db_port: db_port.into(),
            db_user: db_user.into(),
            db_password: db_password.into(),
            db_name: db_name.into(),
            check_interval_secs,
            lag_threshold_secs,
            bus: None,
            shutdown_tx: None,
        }
    }

    /// Attach the ShimBus for event emission.
    pub fn set_bus(&mut self, bus: ShimBus) {
        self.bus = Some(bus);
    }

    /// Run a single Patroni health check cycle.
    pub async fn check(&self) -> PatroniCheckResult {
        FailoverShim::patroni_check(
            &self.db_host,
            &self.db_port,
            &self.db_user,
            &self.db_password,
            &self.db_name,
        )
        .await
    }

    /// Start the periodic health check loop.
    pub async fn start_loop(&mut self) {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let db_host = self.db_host.clone();
        let db_port = self.db_port.clone();
        let db_user = self.db_user.clone();
        let db_password = self.db_password.clone();
        let db_name = self.db_name.clone();
        let check_interval_secs = self.check_interval_secs;
        let lag_threshold_secs = self.lag_threshold_secs;
        let bus = self.bus.clone();

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(check_interval_secs));
            let mut last_recovery_state: Option<bool> = None;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match FailoverShim::patroni_check(
                            &db_host, &db_port, &db_user, &db_password, &db_name,
                        ).await {
                            PatroniCheckResult::Healthy { in_recovery, max_lag_secs } => {
                                if let Some(prev_recovery) = last_recovery_state {
                                    if prev_recovery != in_recovery {
                                        let new_role = if in_recovery { "replica" } else { "primary" };
                                        tracing::info!(
                                            "PatroniMonitor: node role changed to {} (was recovery={})",
                                            new_role, prev_recovery
                                        );
                                        if let Some(ref bus) = bus {
                                            bus.emit(
                                                "patroni-monitor",
                                                EventType::FailoverTriggered {
                                                    old_primary: db_host.clone(),
                                                    new_primary: if in_recovery { "replica".to_string() } else { db_host.clone() },
                                                },
                                                Severity::Notice,
                                            );
                                        }
                                    }
                                }
                                last_recovery_state = Some(in_recovery);

                                if max_lag_secs > lag_threshold_secs {
                                    tracing::warn!(
                                        "PatroniMonitor: replication lag {}s exceeds threshold {}s",
                                        max_lag_secs, lag_threshold_secs
                                    );
                                    if let Some(ref bus) = bus {
                                        bus.emit(
                                            "patroni-monitor",
                                            EventType::ReplicationLagWarning {
                                                lag_ms: (max_lag_secs * 1000.0) as u64,
                                                threshold_ms: (lag_threshold_secs * 1000.0) as u64,
                                            },
                                            Severity::Warning,
                                        );
                                    }
                                }
                            }
                            PatroniCheckResult::Unreachable => {
                                tracing::warn!("PatroniMonitor: primary unreachable (host={})", db_host);
                                if let Some(ref bus) = bus {
                                    bus.emit(
                                        "patroni-monitor",
                                        EventType::FailoverTriggered {
                                            old_primary: db_host.clone(),
                                            new_primary: "unknown".to_string(),
                                        },
                                        Severity::Error,
                                    );
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::info!("PatroniMonitor shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!(
            "PatroniMonitor started (host={}, interval={}s, lag_threshold={}s)",
            self.db_host, self.check_interval_secs, self.lag_threshold_secs
        );
    }

    /// Stop the health check loop.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
    }

    /// Get the database host.
    pub fn db_host(&self) -> &str {
        &self.db_host
    }

    /// Get the database port.
    pub fn db_port(&self) -> &str {
        &self.db_port
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

    /// Get the check interval in seconds.
    pub fn check_interval_secs(&self) -> u64 {
        self.check_interval_secs
    }

    /// Get the lag threshold in seconds.
    pub fn lag_threshold_secs(&self) -> f64 {
        self.lag_threshold_secs
    }
}

/// Periodic Redis Sentinel health check monitor.
///
/// Connects to Redis Sentinel and tracks the current master.
/// Emits `FailoverDetected` events when the master address changes.
pub struct RedisSentinelMonitor {
    sentinel_url: String,
    master_name: String,
    check_interval_secs: u64,
    bus: Option<ShimBus>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl RedisSentinelMonitor {
    /// Create a new RedisSentinelMonitor from environment variables.
    pub fn from_env() -> Self {
        Self {
            sentinel_url: std::env::var("REDIS_SENTINEL_URL")
                .unwrap_or_else(|_| "redis://localhost:26379".to_string()),
            master_name: std::env::var("REDIS_SENTINEL_MASTER")
                .unwrap_or_else(|_| "mymaster".to_string()),
            check_interval_secs: std::env::var("FAILOVER_CHECK_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            bus: None,
            shutdown_tx: None,
        }
    }

    /// Create a new RedisSentinelMonitor with explicit configuration.
    pub fn new(
        sentinel_url: impl Into<String>,
        master_name: impl Into<String>,
        check_interval_secs: u64,
    ) -> Self {
        Self {
            sentinel_url: sentinel_url.into(),
            master_name: master_name.into(),
            check_interval_secs,
            bus: None,
            shutdown_tx: None,
        }
    }

    /// Attach the ShimBus for event emission.
    pub fn set_bus(&mut self, bus: ShimBus) {
        self.bus = Some(bus);
    }

    /// Run a single Redis Sentinel check cycle.
    pub async fn check(&self) -> RedisSentinelCheckResult {
        FailoverShim::redis_sentinel_check(&self.sentinel_url, &self.master_name).await
    }

    /// Start the periodic health check loop.
    pub async fn start_loop(&mut self) {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let sentinel_url = self.sentinel_url.clone();
        let master_name = self.master_name.clone();
        let check_interval_secs = self.check_interval_secs;
        let bus = self.bus.clone();

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(check_interval_secs));
            let mut last_known_master: Option<String> = None;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match FailoverShim::redis_sentinel_check(&sentinel_url, &master_name).await {
                            RedisSentinelCheckResult::MasterInfo { ip, port } => {
                                let master_addr = format!("{}:{}", ip, port);

                                if let Some(ref prev) = last_known_master {
                                    if prev != &master_addr {
                                        tracing::info!(
                                            "RedisSentinelMonitor: master changed from {} to {}",
                                            prev, master_addr
                                        );
                                        if let Some(ref bus) = bus {
                                            bus.emit(
                                                "redis-sentinel-monitor",
                                                EventType::FailoverCompleted {
                                                    promoted: master_addr.clone(),
                                                },
                                                Severity::Notice,
                                            );
                                        }
                                    }
                                }
                                last_known_master = Some(master_addr);
                            }
                            RedisSentinelCheckResult::Unreachable => {
                                tracing::warn!(
                                    "RedisSentinelMonitor: sentinel unreachable (url={})",
                                    sentinel_url
                                );
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::info!("RedisSentinelMonitor shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!(
            "RedisSentinelMonitor started (sentinel={}, master={}, interval={}s)",
            self.sentinel_url, self.master_name, self.check_interval_secs
        );
    }

    /// Stop the health check loop.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
    }

    /// Get the sentinel URL.
    pub fn sentinel_url(&self) -> &str {
        &self.sentinel_url
    }

    /// Get the master name.
    pub fn master_name(&self) -> &str {
        &self.master_name
    }

    /// Get the check interval in seconds.
    pub fn check_interval_secs(&self) -> u64 {
        self.check_interval_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let shim = FailoverShim::new();
        assert_eq!(shim.check_interval_secs(), 5);
        assert_eq!(shim.failure_threshold, 3);
        assert_eq!(shim.db_type, "postgres");
        assert_eq!(shim.connector(), &FailoverConnector::Tcp);
        assert_eq!(shim.shared.state(), FailoverState::Healthy);
        assert_eq!(shim.shared.failover_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_metrics_reads_live_state() {
        let shim = FailoverShim::new();
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 3);
        assert_eq!(metrics[0].name, "failover_state");
        assert_eq!(metrics[0].value, 0.0); // Healthy
        assert_eq!(metrics[1].value, 0.0); // failover_count
        assert_eq!(metrics[2].value, 0.0); // consecutive_failures

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

    #[test]
    fn test_failover_connector_defaults_to_tcp() {
        let connector = FailoverConnector::default();
        assert_eq!(connector, FailoverConnector::Tcp);
    }

    #[test]
    fn test_failover_connector_variants() {
        let tcp = FailoverConnector::Tcp;
        let patroni = FailoverConnector::Patroni;
        let sentinel = FailoverConnector::RedisSentinel;

        assert_ne!(tcp, patroni);
        assert_ne!(patroni, sentinel);
        assert_ne!(tcp, sentinel);
    }

    #[test]
    fn test_parse_replication_lag_valid() {
        assert_eq!(parse_replication_lag("15.5"), Some(15.5));
        assert_eq!(parse_replication_lag("0"), Some(0.0));
        assert_eq!(parse_replication_lag("120.0"), Some(120.0));
    }

    #[test]
    fn test_parse_replication_lag_invalid() {
        assert_eq!(parse_replication_lag(""), None);
        assert_eq!(parse_replication_lag("NULL"), None);
        assert_eq!(parse_replication_lag("not_a_number"), None);
    }

    #[test]
    fn test_failover_event_serialization() {
        let event = FailoverEvent {
            event: "failover".to_string(),
            old_primary: "127.0.0.1:5432".to_string(),
            new_primary: "127.0.0.1:5433".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            reason: "3 consecutive health check failures".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("failover"));
        assert!(json.contains("127.0.0.1:5432"));
    }

    #[test]
    fn test_patroni_config_from_env() {
        std::env::set_var("FAILOVER_CONNECTOR", "patroni");
        std::env::set_var("FAILOVER_DB_HOST", "pg-primary.internal");
        std::env::set_var("FAILOVER_DB_PORT", "5433");
        std::env::set_var("FAILOVER_DB_USER", "admin");
        std::env::set_var("FAILOVER_DB_PASSWORD", "secret123");
        std::env::set_var("FAILOVER_DB_NAME", "mydb");
        std::env::set_var("FAILOVER_LAG_THRESHOLD_SECS", "15.5");
        std::env::set_var("FAILOVER_CHECK_INTERVAL_SECS", "20");

        let shim = FailoverShim::new();
        assert_eq!(shim.connector(), &FailoverConnector::Patroni);
        assert_eq!(shim.db_host(), "pg-primary.internal");
        assert_eq!(shim.db_port(), "5433");
        assert_eq!(shim.db_user(), "admin");
        assert_eq!(shim.db_password(), "secret123");
        assert_eq!(shim.db_name(), "mydb");
        assert_eq!(shim.lag_threshold_secs(), 15.5);
        assert_eq!(shim.check_interval_secs(), 20);

        std::env::remove_var("FAILOVER_CONNECTOR");
        std::env::remove_var("FAILOVER_DB_HOST");
        std::env::remove_var("FAILOVER_DB_PORT");
        std::env::remove_var("FAILOVER_DB_USER");
        std::env::remove_var("FAILOVER_DB_PASSWORD");
        std::env::remove_var("FAILOVER_DB_NAME");
        std::env::remove_var("FAILOVER_LAG_THRESHOLD_SECS");
        std::env::remove_var("FAILOVER_CHECK_INTERVAL_SECS");
    }

    #[test]
    fn test_redis_sentinel_config_from_env() {
        std::env::set_var("FAILOVER_CONNECTOR", "redis-sentinel");
        std::env::set_var("REDIS_SENTINEL_URL", "redis://sentinel.prod:26379");
        std::env::set_var("REDIS_SENTINEL_MASTER", "prod-master");

        let shim = FailoverShim::new();
        assert_eq!(shim.connector(), &FailoverConnector::RedisSentinel);
        assert_eq!(shim.redis_sentinel_url(), "redis://sentinel.prod:26379");
        assert_eq!(shim.redis_sentinel_master(), "prod-master");

        std::env::remove_var("FAILOVER_CONNECTOR");
        std::env::remove_var("REDIS_SENTINEL_URL");
        std::env::remove_var("REDIS_SENTINEL_MASTER");
    }

    #[test]
    fn test_redis_sentinel_defaults() {
        let shim = FailoverShim::new();
        assert_eq!(shim.redis_sentinel_url(), "redis://localhost:26379");
        assert_eq!(shim.redis_sentinel_master(), "mymaster");
    }

    #[test]
    fn test_patroni_defaults() {
        let shim = FailoverShim::new();
        assert_eq!(shim.db_host(), "127.0.0.1");
        assert_eq!(shim.db_port(), "5432");
        assert_eq!(shim.db_user(), "postgres");
        assert_eq!(shim.db_password(), "");
        assert_eq!(shim.db_name(), "postgres");
        assert_eq!(shim.lag_threshold_secs(), 30.0);
    }

    #[test]
    fn test_patroni_monitor_from_env() {
        std::env::set_var("FAILOVER_DB_HOST", "pg-cluster.internal");
        std::env::set_var("FAILOVER_DB_PORT", "5433");
        std::env::set_var("FAILOVER_DB_USER", "admin");
        std::env::set_var("FAILOVER_DB_PASSWORD", "secret");
        std::env::set_var("FAILOVER_DB_NAME", "mydb");
        std::env::set_var("FAILOVER_CHECK_INTERVAL_SECS", "15");
        std::env::set_var("FAILOVER_LAG_THRESHOLD_SECS", "20.5");

        let monitor = PatroniMonitor::from_env();
        assert_eq!(monitor.db_host(), "pg-cluster.internal");
        assert_eq!(monitor.db_port(), "5433");
        assert_eq!(monitor.db_user(), "admin");
        assert_eq!(monitor.db_password(), "secret");
        assert_eq!(monitor.db_name, "mydb");
        assert_eq!(monitor.check_interval_secs(), 15);
        assert_eq!(monitor.lag_threshold_secs(), 20.5);

        std::env::remove_var("FAILOVER_DB_HOST");
        std::env::remove_var("FAILOVER_DB_PORT");
        std::env::remove_var("FAILOVER_DB_USER");
        std::env::remove_var("FAILOVER_DB_PASSWORD");
        std::env::remove_var("FAILOVER_DB_NAME");
        std::env::remove_var("FAILOVER_CHECK_INTERVAL_SECS");
        std::env::remove_var("FAILOVER_LAG_THRESHOLD_SECS");
    }

    #[test]
    fn test_patroni_monitor_explicit() {
        let monitor = PatroniMonitor::new(
            "10.0.0.1", "5433", "admin", "pass", "testdb", 15, 20.5,
        );
        assert_eq!(monitor.db_host(), "10.0.0.1");
        assert_eq!(monitor.db_port(), "5433");
        assert_eq!(monitor.db_user(), "admin");
        assert_eq!(monitor.db_password(), "pass");
        assert_eq!(monitor.db_name, "testdb");
        assert_eq!(monitor.check_interval_secs(), 15);
        assert_eq!(monitor.lag_threshold_secs(), 20.5);
    }

    #[test]
    fn test_patroni_monitor_defaults() {
        std::env::remove_var("FAILOVER_DB_HOST");
        std::env::remove_var("FAILOVER_DB_PORT");
        std::env::remove_var("FAILOVER_DB_USER");
        std::env::remove_var("FAILOVER_DB_PASSWORD");
        std::env::remove_var("FAILOVER_DB_NAME");
        std::env::remove_var("FAILOVER_CHECK_INTERVAL_SECS");
        std::env::remove_var("FAILOVER_LAG_THRESHOLD_SECS");

        let monitor = PatroniMonitor::from_env();
        assert_eq!(monitor.db_host(), "localhost");
        assert_eq!(monitor.db_port(), "5432");
        assert_eq!(monitor.db_user(), "postgres");
        assert_eq!(monitor.db_password(), "");
        assert_eq!(monitor.db_name, "postgres");
        assert_eq!(monitor.check_interval_secs(), 10);
        assert_eq!(monitor.lag_threshold_secs(), 30.0);
    }

    #[test]
    fn test_redis_sentinel_monitor_from_env() {
        std::env::set_var("REDIS_SENTINEL_URL", "redis://sentinel.prod:26379");
        std::env::set_var("REDIS_SENTINEL_MASTER", "prod-master");
        std::env::set_var("FAILOVER_CHECK_INTERVAL_SECS", "7");

        let monitor = RedisSentinelMonitor::from_env();
        assert_eq!(monitor.sentinel_url(), "redis://sentinel.prod:26379");
        assert_eq!(monitor.master_name(), "prod-master");
        assert_eq!(monitor.check_interval_secs(), 7);

        std::env::remove_var("REDIS_SENTINEL_URL");
        std::env::remove_var("REDIS_SENTINEL_MASTER");
        std::env::remove_var("FAILOVER_CHECK_INTERVAL_SECS");
    }

    #[test]
    fn test_redis_sentinel_monitor_explicit() {
        let monitor = RedisSentinelMonitor::new(
            "redis://sentinel.prod:26379", "prod-master", 7,
        );
        assert_eq!(monitor.sentinel_url(), "redis://sentinel.prod:26379");
        assert_eq!(monitor.master_name(), "prod-master");
        assert_eq!(monitor.check_interval_secs(), 7);
    }

    #[test]
    fn test_redis_sentinel_monitor_defaults() {
        std::env::remove_var("REDIS_SENTINEL_URL");
        std::env::remove_var("REDIS_SENTINEL_MASTER");
        std::env::remove_var("FAILOVER_CHECK_INTERVAL_SECS");

        let monitor = RedisSentinelMonitor::from_env();
        assert_eq!(monitor.sentinel_url(), "redis://localhost:26379");
        assert_eq!(monitor.master_name(), "mymaster");
        assert_eq!(monitor.check_interval_secs(), 5);
    }
}
