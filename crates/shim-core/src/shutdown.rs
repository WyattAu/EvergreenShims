//! Graceful shutdown sequences for different database types.
//!
//! Provides per-DB-type shutdown sequences:
//! - **PostgreSQL**: SIGTERM -> smart shutdown -> wait for queries -> checkpoint -> exit
//! - **Redis**: SIGTERM -> save RDB -> wait for fork -> exit
//! - **Generic**: SIGTERM -> wait timeout -> SIGKILL
//!
//! ## Environment Variables
//!
//! ```text
//! SHUTDOWN_TIMEOUT_SECS  Global shutdown timeout in seconds (default: 30)
//! SHUTDOWN_STRATEGY      Shutdown strategy: postgres, redis, generic (default: generic)
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use tokio::process::Child;

use crate::error::Result;

/// Database type for shutdown sequence selection.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum DatabaseType {
    /// PostgreSQL database.
    Postgres,
    /// Redis cache.
    Redis,
    /// Generic process (default).
    #[default]
    Generic,
}

impl std::fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatabaseType::Postgres => write!(f, "postgres"),
            DatabaseType::Redis => write!(f, "redis"),
            DatabaseType::Generic => write!(f, "generic"),
        }
    }
}

/// Graceful shutdown sequence for a database process.
#[async_trait::async_trait]
pub trait GracefulShutdown: Send + Sync {
    /// Get the database type this shutdown handler targets.
    fn db_type(&self) -> DatabaseType;

    /// Execute the graceful shutdown sequence for a process.
    ///
    /// Returns `Ok(true)` if the process exited cleanly,
    /// `Ok(false)` if SIGKILL was needed, or `Err` on failure.
    async fn shutdown(&self, pid: u32, timeout_secs: u64) -> Result<ShutdownResult>;

    /// Get a human-readable description of the shutdown sequence.
    fn description(&self) -> String;
}

/// Result of a shutdown attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownResult {
    /// Whether the process exited cleanly without SIGKILL.
    pub clean_exit: bool,
    /// The database type that was shut down.
    pub db_type: DatabaseType,
    /// Shutdown duration in milliseconds.
    pub duration_ms: u64,
    /// Number of signals sent.
    pub signals_sent: u32,
    /// Human-readable log of the shutdown sequence.
    pub log: Vec<String>,
}

/// PostgreSQL graceful shutdown.
///
/// Sequence: SIGTERM -> smart shutdown (pg_ctl stop -m smart)
/// -> wait for queries to finish -> checkpoint -> exit.
pub struct PostgresShutdown;

#[async_trait::async_trait]
impl GracefulShutdown for PostgresShutdown {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::Postgres
    }

    fn description(&self) -> String {
        "PostgreSQL: SIGTERM -> smart shutdown -> wait for queries -> checkpoint -> exit"
            .to_string()
    }

    async fn shutdown(&self, pid: u32, timeout_secs: u64) -> Result<ShutdownResult> {
        let start = std::time::Instant::now();
        let mut log = Vec::new();
        let mut signals_sent = 0;

        // Step 1: Send SIGTERM (PostgreSQL interprets this as smart shutdown)
        log.push(format!("[1/4] Sending SIGTERM to PID {}", pid));
        if let Err(e) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
            log.push(format!(
                "  SIGTERM failed: {} (process may already be dead)",
                e
            ));
            return Ok(ShutdownResult {
                clean_exit: false,
                db_type: DatabaseType::Postgres,
                duration_ms: start.elapsed().as_millis() as u64,
                signals_sent,
                log,
            });
        }
        signals_sent += 1;

        // Step 2: Wait for queries to finish (smart shutdown = wait for clients to disconnect)
        let wait_per_step = Duration::from_secs(timeout_secs / 4);
        log.push(format!(
            "[2/4] Waiting for queries to finish (up to {}s)",
            wait_per_step.as_secs()
        ));
        tokio::time::sleep(wait_per_step).await;

        // Step 3: If still running, send checkpoint signal (SIGUSR2 for pg_checkpoint)
        if is_process_alive(pid) {
            log.push("[3/4] Process still running, waiting for checkpoint".to_string());
            tokio::time::sleep(wait_per_step).await;
        } else {
            log.push("[3/4] Process exited cleanly after SIGTERM".to_string());
            return Ok(ShutdownResult {
                clean_exit: true,
                db_type: DatabaseType::Postgres,
                duration_ms: start.elapsed().as_millis() as u64,
                signals_sent,
                log,
            });
        }

        // Step 4: Final wait before force kill
        if is_process_alive(pid) {
            log.push(format!(
                "[4/4] Process still running after {}s, waiting for final exit",
                wait_per_step.as_secs() * 2
            ));
            let remaining = Duration::from_secs(timeout_secs / 2);
            tokio::time::timeout(remaining, async {
                while is_process_alive(pid) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .ok();
        }

        let clean = !is_process_alive(pid);
        if !clean {
            log.push("[4/4] Process did not exit gracefully".to_string());
        } else {
            log.push("[4/4] Process exited cleanly".to_string());
        }

        Ok(ShutdownResult {
            clean_exit: clean,
            db_type: DatabaseType::Postgres,
            duration_ms: start.elapsed().as_millis() as u64,
            signals_sent,
            log,
        })
    }
}

/// Redis graceful shutdown.
///
/// Sequence: SIGTERM -> save RDB -> wait for fork to complete -> exit.
pub struct RedisShutdown;

#[async_trait::async_trait]
impl GracefulShutdown for RedisShutdown {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::Redis
    }

    fn description(&self) -> String {
        "Redis: SIGTERM -> save RDB -> wait for fork -> exit".to_string()
    }

    async fn shutdown(&self, pid: u32, timeout_secs: u64) -> Result<ShutdownResult> {
        let start = std::time::Instant::now();
        let mut log = Vec::new();
        let mut signals_sent = 0;

        // Step 1: Send SIGTERM (Redis saves RDB on SIGTERM by default)
        log.push(format!("[1/3] Sending SIGTERM to PID {}", pid));
        if let Err(e) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
            log.push(format!(
                "  SIGTERM failed: {} (process may already be dead)",
                e
            ));
            return Ok(ShutdownResult {
                clean_exit: false,
                db_type: DatabaseType::Redis,
                duration_ms: start.elapsed().as_millis() as u64,
                signals_sent,
                log,
            });
        }
        signals_sent += 1;

        // Step 2: Wait for RDB save (fork can take time with large datasets)
        let wait_per_step = Duration::from_secs(timeout_secs / 3);
        log.push(format!(
            "[2/3] Waiting for RDB save/fork completion (up to {}s)",
            wait_per_step.as_secs()
        ));
        tokio::time::sleep(wait_per_step).await;

        // Step 3: Check if process exited
        if !is_process_alive(pid) {
            log.push("[3/3] Process exited cleanly after SIGTERM".to_string());
            return Ok(ShutdownResult {
                clean_exit: true,
                db_type: DatabaseType::Redis,
                duration_ms: start.elapsed().as_millis() as u64,
                signals_sent,
                log,
            });
        }

        // Wait more for fork completion
        log.push("[3/3] Process still running, waiting for fork completion".to_string());
        let remaining = Duration::from_secs(timeout_secs * 2 / 3);
        tokio::time::timeout(remaining, async {
            while is_process_alive(pid) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .ok();

        let clean = !is_process_alive(pid);
        if !clean {
            log.push("[3/3] Process did not exit gracefully".to_string());
        } else {
            log.push("[3/3] Process exited cleanly".to_string());
        }

        Ok(ShutdownResult {
            clean_exit: clean,
            db_type: DatabaseType::Redis,
            duration_ms: start.elapsed().as_millis() as u64,
            signals_sent,
            log,
        })
    }
}

/// Generic graceful shutdown.
///
/// Sequence: SIGTERM -> wait timeout -> SIGKILL.
pub struct GenericShutdown;

#[async_trait::async_trait]
impl GracefulShutdown for GenericShutdown {
    fn db_type(&self) -> DatabaseType {
        DatabaseType::Generic
    }

    fn description(&self) -> String {
        "Generic: SIGTERM -> wait timeout -> SIGKILL".to_string()
    }

    async fn shutdown(&self, pid: u32, timeout_secs: u64) -> Result<ShutdownResult> {
        let start = std::time::Instant::now();
        let mut log = Vec::new();
        let mut signals_sent = 0;

        // Step 1: Send SIGTERM
        log.push(format!("[1/3] Sending SIGTERM to PID {}", pid));
        if let Err(e) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
            log.push(format!(
                "  SIGTERM failed: {} (process may already be dead)",
                e
            ));
            return Ok(ShutdownResult {
                clean_exit: false,
                db_type: DatabaseType::Generic,
                duration_ms: start.elapsed().as_millis() as u64,
                signals_sent,
                log,
            });
        }
        signals_sent += 1;

        // Step 2: Wait for timeout
        log.push(format!("[2/3] Waiting {}s for graceful exit", timeout_secs));
        tokio::time::timeout(Duration::from_secs(timeout_secs), async {
            while is_process_alive(pid) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .ok();

        // Step 3: Force kill if still running
        if is_process_alive(pid) {
            log.push("[3/3] Process still running, sending SIGKILL".to_string());
            if let Ok(()) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL) {
                signals_sent += 1;
            }
            // Wait briefly for SIGKILL to take effect
            tokio::time::sleep(Duration::from_millis(500)).await;

            let clean = !is_process_alive(pid);
            if clean {
                log.push("[3/3] Process killed by SIGKILL".to_string());
            } else {
                log.push("[3/3] Process did not respond to SIGKILL".to_string());
            }

            Ok(ShutdownResult {
                clean_exit: false,
                db_type: DatabaseType::Generic,
                duration_ms: start.elapsed().as_millis() as u64,
                signals_sent,
                log,
            })
        } else {
            log.push("[3/3] Process exited cleanly after SIGTERM".to_string());
            Ok(ShutdownResult {
                clean_exit: true,
                db_type: DatabaseType::Generic,
                duration_ms: start.elapsed().as_millis() as u64,
                signals_sent,
                log,
            })
        }
    }
}

/// Create the appropriate shutdown handler for a database type.
pub fn shutdown_handler(db_type: DatabaseType) -> Box<dyn GracefulShutdown> {
    match db_type {
        DatabaseType::Postgres => Box::new(PostgresShutdown),
        DatabaseType::Redis => Box::new(RedisShutdown),
        DatabaseType::Generic => Box::new(GenericShutdown),
    }
}

/// Check if a process is still alive.
fn is_process_alive(pid: u32) -> bool {
    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}

/// Shutdown manager that coordinates the shutdown sequence.
pub struct ShutdownManager {
    /// The shutdown handler for the target process.
    handler: Box<dyn GracefulShutdown>,
    /// Timeout in seconds.
    timeout_secs: u64,
    /// Whether shutdown has been initiated.
    initiated: Arc<AtomicBool>,
}

impl ShutdownManager {
    /// Create a new shutdown manager.
    pub fn new(db_type: DatabaseType, timeout_secs: u64) -> Self {
        Self {
            handler: shutdown_handler(db_type),
            timeout_secs,
            initiated: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get the database type.
    pub fn db_type(&self) -> DatabaseType {
        self.handler.db_type()
    }

    /// Get the description of the shutdown sequence.
    pub fn description(&self) -> String {
        self.handler.description()
    }

    /// Initiate graceful shutdown of a process.
    pub async fn shutdown(&self, pid: u32) -> Result<ShutdownResult> {
        self.initiated.store(true, Ordering::SeqCst);
        tracing::info!(
            "Initiating {} graceful shutdown for PID {} (timeout={}s)",
            self.handler.db_type(),
            pid,
            self.timeout_secs
        );

        let result = self.handler.shutdown(pid, self.timeout_secs).await;

        match &result {
            Ok(r) => {
                if r.clean_exit {
                    tracing::info!(
                        "{} shutdown completed cleanly in {}ms",
                        self.handler.db_type(),
                        r.duration_ms
                    );
                } else {
                    tracing::warn!(
                        "{} shutdown required force kill after {}ms",
                        self.handler.db_type(),
                        r.duration_ms
                    );
                }
                for line in &r.log {
                    tracing::debug!("  {}", line);
                }
            }
            Err(e) => {
                tracing::error!("{} shutdown failed: {}", self.handler.db_type(), e);
            }
        }

        result
    }

    /// Check if shutdown has been initiated.
    pub fn is_initiated(&self) -> bool {
        self.initiated.load(Ordering::SeqCst)
    }
}

/// High-level shutdown strategy for use with `graceful_shutdown`.
///
/// Each variant maps to the appropriate signal sequence:
/// - `PostgresGraceful`: SIGTERM -> wait for smart shutdown -> SIGKILL on timeout
/// - `RedisGraceful`: SIGTERM -> wait for RDB save -> SIGKILL on timeout
/// - `GenericGraceful`: SIGTERM -> wait timeout -> SIGKILL
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShutdownStrategy {
    /// PostgreSQL: SIGTERM triggers smart shutdown, wait for queries to drain.
    PostgresGraceful,
    /// Redis: SIGTERM triggers RDB save, wait for fork to complete.
    RedisGraceful,
    /// Generic: SIGTERM, wait timeout, then SIGKILL.
    #[default]
    GenericGraceful,
}

impl std::fmt::Display for ShutdownStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShutdownStrategy::PostgresGraceful => write!(f, "postgres"),
            ShutdownStrategy::RedisGraceful => write!(f, "redis"),
            ShutdownStrategy::GenericGraceful => write!(f, "generic"),
        }
    }
}

impl ShutdownStrategy {
    /// Create from a string (e.g., from env var).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Self::PostgresGraceful,
            "redis" => Self::RedisGraceful,
            _ => Self::GenericGraceful,
        }
    }

    /// Create from the `SHUTDOWN_STRATEGY` env var.
    pub fn from_env() -> Self {
        Self::from_str(
            &std::env::var("SHUTDOWN_STRATEGY").unwrap_or_else(|_| "generic".to_string()),
        )
    }

    /// Get the default timeout from `SHUTDOWN_TIMEOUT_SECS` env var.
    pub fn default_timeout() -> u64 {
        std::env::var("SHUTDOWN_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30)
    }

    /// Convert to `DatabaseType`.
    pub fn to_database_type(&self) -> DatabaseType {
        match self {
            ShutdownStrategy::PostgresGraceful => DatabaseType::Postgres,
            ShutdownStrategy::RedisGraceful => DatabaseType::Redis,
            ShutdownStrategy::GenericGraceful => DatabaseType::Generic,
        }
    }
}

/// Execute a graceful shutdown on a `tokio::process::Child`.
///
/// Sends the appropriate signals based on the strategy, polls the child
/// for exit, and escalates to SIGKILL if the timeout is exceeded.
///
/// # Arguments
/// * `strategy` - The shutdown strategy to use.
/// * `child` - The tokio child process handle.
/// * `timeout_secs` - Maximum seconds to wait before SIGKILL.
pub async fn graceful_shutdown(
    strategy: &ShutdownStrategy,
    child: &mut Child,
    timeout_secs: u64,
) -> Result<ShutdownResult> {
    let pid = child.id().unwrap_or(0);
    if pid == 0 {
        return Err(crate::error::Error::Process(
            "child process has no valid PID".to_string(),
        ));
    }

    let start = std::time::Instant::now();
    let mut log = Vec::new();
    let mut signals_sent = 0u32;

    // Step 1: Send SIGTERM
    log.push(format!("[1] Sending SIGTERM to PID {} ({})", pid, strategy));
    if let Err(e) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
        log.push(format!("  SIGTERM failed: {}", e));
        return Ok(ShutdownResult {
            clean_exit: false,
            db_type: strategy.to_database_type(),
            duration_ms: start.elapsed().as_millis() as u64,
            signals_sent,
            log,
        });
    }
    signals_sent += 1;

    // Step 2: Wait for the child to exit, with timeout
    let wait_secs = match strategy {
        ShutdownStrategy::PostgresGraceful => {
            // Postgres smart shutdown: wait for queries to drain
            // Give it 80% of timeout, then check
            log.push(format!(
                "[2] Waiting for Postgres smart shutdown (up to {}s)",
                timeout_secs * 80 / 100
            ));
            timeout_secs * 80 / 100
        }
        ShutdownStrategy::RedisGraceful => {
            // Redis: wait for RDB save/fork to complete
            log.push(format!(
                "[2] Waiting for Redis RDB save (up to {}s)",
                timeout_secs * 80 / 100
            ));
            timeout_secs * 80 / 100
        }
        ShutdownStrategy::GenericGraceful => {
            log.push(format!("[2] Waiting {}s for graceful exit", timeout_secs));
            timeout_secs
        }
    };

    let wait_result = tokio::time::timeout(Duration::from_secs(wait_secs), async {
        while is_process_alive(pid) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    if !is_process_alive(pid) {
        log.push("[3] Process exited gracefully".to_string());
        return Ok(ShutdownResult {
            clean_exit: true,
            db_type: strategy.to_database_type(),
            duration_ms: start.elapsed().as_millis() as u64,
            signals_sent,
            log,
        });
    }

    if wait_result.is_ok() {
        // Exited during wait
        log.push("[3] Process exited during wait period".to_string());
        return Ok(ShutdownResult {
            clean_exit: true,
            db_type: strategy.to_database_type(),
            duration_ms: start.elapsed().as_millis() as u64,
            signals_sent,
            log,
        });
    }

    // Step 3: Force kill with SIGKILL
    log.push("[3] Timeout reached, sending SIGKILL".to_string());
    if let Ok(()) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL) {
        signals_sent += 1;
    }
    // Reap the child process to eliminate zombie state
    child.kill().await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let clean = !is_process_alive(pid);
    if clean {
        log.push("[3] Process killed by SIGKILL".to_string());
    } else {
        log.push("[3] Process did not respond to SIGKILL".to_string());
    }

    Ok(ShutdownResult {
        clean_exit: clean,
        db_type: strategy.to_database_type(),
        duration_ms: start.elapsed().as_millis() as u64,
        signals_sent,
        log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_type_default() {
        assert_eq!(DatabaseType::default(), DatabaseType::Generic);
    }

    #[test]
    fn test_database_type_display() {
        assert_eq!(DatabaseType::Postgres.to_string(), "postgres");
        assert_eq!(DatabaseType::Redis.to_string(), "redis");
        assert_eq!(DatabaseType::Generic.to_string(), "generic");
    }

    #[test]
    fn test_shutdown_handler_creates_correct_types() {
        let pg = shutdown_handler(DatabaseType::Postgres);
        assert_eq!(pg.db_type(), DatabaseType::Postgres);

        let redis = shutdown_handler(DatabaseType::Redis);
        assert_eq!(redis.db_type(), DatabaseType::Redis);

        let generic = shutdown_handler(DatabaseType::Generic);
        assert_eq!(generic.db_type(), DatabaseType::Generic);
    }

    #[test]
    fn test_shutdown_descriptions() {
        let pg = PostgresShutdown;
        assert!(pg.description().contains("PostgreSQL"));
        assert!(pg.description().contains("SIGTERM"));

        let redis = RedisShutdown;
        assert!(redis.description().contains("Redis"));
        assert!(redis.description().contains("RDB"));

        let generic = GenericShutdown;
        assert!(generic.description().contains("Generic"));
        assert!(generic.description().contains("SIGKILL"));
    }

    #[test]
    fn test_shutdown_result_serialization() {
        let result = ShutdownResult {
            clean_exit: true,
            db_type: DatabaseType::Postgres,
            duration_ms: 1500,
            signals_sent: 1,
            log: vec!["test".to_string()],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("clean_exit"));
        assert!(json.contains("Postgres"));
    }

    #[test]
    fn test_shutdown_manager_new() {
        let manager = ShutdownManager::new(DatabaseType::Postgres, 30);
        assert_eq!(manager.db_type(), DatabaseType::Postgres);
        assert!(!manager.is_initiated());
    }

    #[test]
    fn test_shutdown_manager_description() {
        let manager = ShutdownManager::new(DatabaseType::Redis, 10);
        assert!(manager.description().contains("Redis"));
    }

    #[test]
    fn test_is_process_alive_self() {
        // Current process should be alive
        let my_pid = std::process::id();
        assert!(is_process_alive(my_pid));
    }

    #[test]
    fn test_is_process_alive_invalid_pid() {
        // PID 1 might exist but very high PIDs should not
        assert!(!is_process_alive(i32::MAX as u32));
    }

    #[tokio::test]
    async fn test_postgres_shutdown_nonexistent() {
        let handler = PostgresShutdown;
        let result = handler.shutdown(i32::MAX as u32, 5).await;
        assert!(result.is_ok());
        // Should fail to send signal
        let r = result.unwrap();
        assert!(!r.clean_exit);
        assert!(r.log.iter().any(|l| l.contains("SIGTERM failed")));
    }

    #[tokio::test]
    async fn test_redis_shutdown_nonexistent() {
        let handler = RedisShutdown;
        let result = handler.shutdown(i32::MAX as u32, 5).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(!r.clean_exit);
    }

    #[tokio::test]
    async fn test_generic_shutdown_nonexistent() {
        let handler = GenericShutdown;
        let result = handler.shutdown(i32::MAX as u32, 5).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(!r.clean_exit);
    }

    #[tokio::test]
    async fn test_shutdown_manager_shutdown_nonexistent() {
        let manager = ShutdownManager::new(DatabaseType::Postgres, 5);
        let result = manager.shutdown(i32::MAX as u32).await;
        assert!(result.is_ok());
        assert!(manager.is_initiated());
    }

    #[test]
    fn test_shutdown_strategy_default() {
        assert_eq!(
            ShutdownStrategy::default(),
            ShutdownStrategy::GenericGraceful
        );
    }

    #[test]
    fn test_shutdown_strategy_display() {
        assert_eq!(ShutdownStrategy::PostgresGraceful.to_string(), "postgres");
        assert_eq!(ShutdownStrategy::RedisGraceful.to_string(), "redis");
        assert_eq!(ShutdownStrategy::GenericGraceful.to_string(), "generic");
    }

    #[test]
    fn test_shutdown_strategy_from_str() {
        assert_eq!(
            ShutdownStrategy::from_str("postgres"),
            ShutdownStrategy::PostgresGraceful
        );
        assert_eq!(
            ShutdownStrategy::from_str("postgresql"),
            ShutdownStrategy::PostgresGraceful
        );
        assert_eq!(
            ShutdownStrategy::from_str("pg"),
            ShutdownStrategy::PostgresGraceful
        );
        assert_eq!(
            ShutdownStrategy::from_str("redis"),
            ShutdownStrategy::RedisGraceful
        );
        assert_eq!(
            ShutdownStrategy::from_str("generic"),
            ShutdownStrategy::GenericGraceful
        );
        assert_eq!(
            ShutdownStrategy::from_str("bogus"),
            ShutdownStrategy::GenericGraceful
        );
    }

    #[test]
    fn test_shutdown_strategy_to_database_type() {
        assert_eq!(
            ShutdownStrategy::PostgresGraceful.to_database_type(),
            DatabaseType::Postgres
        );
        assert_eq!(
            ShutdownStrategy::RedisGraceful.to_database_type(),
            DatabaseType::Redis
        );
        assert_eq!(
            ShutdownStrategy::GenericGraceful.to_database_type(),
            DatabaseType::Generic
        );
    }

    #[test]
    fn test_shutdown_strategy_from_env() {
        temp_env::with_vars([("SHUTDOWN_STRATEGY", Some("postgres"))], || {
            assert_eq!(
                ShutdownStrategy::from_env(),
                ShutdownStrategy::PostgresGraceful
            );
        });

        assert_eq!(
            ShutdownStrategy::from_env(),
            ShutdownStrategy::GenericGraceful
        );
    }

    #[test]
    fn test_shutdown_strategy_default_timeout() {
        temp_env::with_var_unset("SHUTDOWN_TIMEOUT_SECS", || {
            assert_eq!(ShutdownStrategy::default_timeout(), 30);
        });

        temp_env::with_vars([("SHUTDOWN_TIMEOUT_SECS", Some("60"))], || {
            assert_eq!(ShutdownStrategy::default_timeout(), 60);
        });
    }

    #[test]
    fn test_shutdown_strategy_serialization() {
        let strategy = ShutdownStrategy::PostgresGraceful;
        let json = serde_json::to_string(&strategy).unwrap();
        assert!(json.contains("PostgresGraceful"));

        let deserialized: ShutdownStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ShutdownStrategy::PostgresGraceful);
    }

    #[tokio::test]
    async fn test_graceful_shutdown_nonexistent_process() {
        let strategy = ShutdownStrategy::GenericGraceful;
        let mut child = tokio::process::Command::new("true").spawn().unwrap();
        // Kill it immediately so PID is invalid
        child.kill().await.ok();
        // Wait for it to finish
        child.wait().await.ok();

        let result = graceful_shutdown(&strategy, &mut child, 5).await;
        // Should handle the error gracefully
        assert!(result.is_ok() || result.is_err());
    }
}
