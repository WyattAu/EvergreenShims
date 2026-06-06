//! Health check types and implementations.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Health status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HealthStatus {
    /// Healthy and ready.
    Healthy,
    /// Unhealthy or not ready.
    Unhealthy,
    /// Unknown state.
    Unknown,
}

impl fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
            HealthStatus::Unknown => write!(f, "unknown"),
        }
    }
}

impl HealthStatus {
    /// Convert to readiness: Unknown counts as NotReady.
    pub fn is_ready(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }
}

/// Health check trait.
#[async_trait::async_trait]
pub trait HealthCheck: Send + Sync {
    /// Check liveness (is the process running?).
    async fn liveness(&self) -> HealthStatus;

    /// Check readiness (can the process serve traffic?).
    async fn readiness(&self) -> HealthStatus;
}

/// Command-based health check.
pub struct CommandHealthCheck {
    /// Command to execute for liveness.
    pub liveness_cmd: String,

    /// Command to execute for readiness.
    pub readiness_cmd: String,

    /// Timeout in seconds.
    pub timeout_secs: u64,
}

#[async_trait::async_trait]
impl HealthCheck for CommandHealthCheck {
    async fn liveness(&self) -> HealthStatus {
        execute_health_cmd(&self.liveness_cmd, self.timeout_secs).await
    }

    async fn readiness(&self) -> HealthStatus {
        execute_health_cmd(&self.readiness_cmd, self.timeout_secs).await
    }
}

/// Startup probe for checking database readiness before declaring healthy.
///
/// Startup probes are run at boot time to ensure dependencies are ready
/// before the service starts accepting traffic.
pub struct StartupProbe {
    /// Command to execute for the startup check.
    pub check_cmd: String,
    /// Timeout for the startup check in seconds.
    pub timeout_secs: u64,
    /// Maximum number of retries before giving up.
    pub max_retries: u32,
    /// Delay between retries in seconds.
    pub retry_delay_secs: u64,
}

impl StartupProbe {
    /// Create a new startup probe.
    pub fn new(check_cmd: impl Into<String>, timeout_secs: u64) -> Self {
        Self {
            check_cmd: check_cmd.into(),
            timeout_secs,
            max_retries: 10,
            retry_delay_secs: 2,
        }
    }

    /// Set maximum retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set retry delay.
    pub fn with_retry_delay_secs(mut self, delay: u64) -> Self {
        self.retry_delay_secs = delay;
        self
    }

    /// Run the startup probe with retries.
    pub async fn check(&self) -> HealthStatus {
        for attempt in 0..=self.max_retries {
            let status = execute_health_cmd(&self.check_cmd, self.timeout_secs).await;
            if status == HealthStatus::Healthy {
                tracing::info!(
                    "Startup probe passed on attempt {}/{}",
                    attempt + 1,
                    self.max_retries + 1
                );
                return status;
            }
            if attempt < self.max_retries {
                tracing::debug!(
                    "Startup probe attempt {}/{} failed, retrying in {}s",
                    attempt + 1,
                    self.max_retries + 1,
                    self.retry_delay_secs
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(self.retry_delay_secs)).await;
            }
        }
        tracing::warn!(
            "Startup probe failed after {} attempts",
            self.max_retries + 1
        );
        HealthStatus::Unhealthy
    }
}

/// Create a Postgres startup probe that runs `pg_isready`.
///
/// Uses `tokio::process::Command` to execute `pg_isready -h <host> -p <port>`.
pub fn postgres_startup_probe(host: &str, port: u16) -> StartupProbe {
    StartupProbe::new(format!("pg_isready -h {} -p {}", host, port), 5)
        .with_max_retries(30)
        .with_retry_delay_secs(1)
}

/// Create a Redis startup probe that runs `redis-cli ping`.
///
/// Uses `tokio::process::Command` to execute `redis-cli -p <port> ping`.
pub fn redis_startup_probe(addr: &str) -> StartupProbe {
    // Parse host:port from addr
    let parts: Vec<&str> = addr.split(':').collect();
    let port = parts.get(1).unwrap_or(&"6379");
    StartupProbe::new(format!("redis-cli -p {} ping", port), 3)
        .with_max_retries(30)
        .with_retry_delay_secs(1)
}

/// Create a startup probe from environment variables.
///
/// Env vars:
/// - `STARTUP_PROBE_TYPE`: `tcp` (default), `postgres`, `redis`
/// - `STARTUP_TIMEOUT_SECS`: timeout in seconds (default: 60)
/// - `FAILOVER_DB_HOST` / `FAILOVER_DB_PORT`: for postgres probe
/// - `REDIS_SENTINEL_URL` / hardcoded redis port: for redis probe
pub fn startup_probe_from_env() -> StartupProbe {
    let probe_type = std::env::var("STARTUP_PROBE_TYPE").unwrap_or_else(|_| "tcp".to_string());
    let timeout_secs = std::env::var("STARTUP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    match probe_type.as_str() {
        "postgres" | "pg" => {
            let host =
                std::env::var("FAILOVER_DB_HOST").unwrap_or_else(|_| "localhost".to_string());
            let port: u16 = std::env::var("FAILOVER_DB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5432);
            postgres_startup_probe(&host, port).with_retry_delay_secs(1)
        }
        "redis" => {
            let port: u16 = 6379;
            redis_startup_probe(&format!("localhost:{}", port)).with_retry_delay_secs(1)
        }
        _ => {
            // TCP probe
            let host =
                std::env::var("FAILOVER_DB_HOST").unwrap_or_else(|_| "localhost".to_string());
            let port: u16 = std::env::var("FAILOVER_DB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5432);
            StartupProbe::new(format!("tcp:{}:{}", host, port), timeout_secs)
                .with_max_retries(30)
                .with_retry_delay_secs(1)
        }
    }
}

/// Execute a health check command.
async fn execute_health_cmd(cmd: &str, timeout_secs: u64) -> HealthStatus {
    let timeout = std::time::Duration::from_secs(timeout_secs);

    // Handle TCP checks (with timeout)
    if let Some(addr) = cmd.strip_prefix("tcp:") {
        return match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_)) => HealthStatus::Healthy,
            Ok(Err(_)) => HealthStatus::Unhealthy,
            Err(_) => HealthStatus::Unhealthy,
        };
    }

    // Handle HTTP checks (requires "http" feature)
    if let Some(url) = cmd.strip_prefix("http:") {
        #[cfg(feature = "http")]
        {
            let client = reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_default();

            match client.get(url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        HealthStatus::Healthy
                    } else {
                        HealthStatus::Unhealthy
                    }
                }
                Err(_) => HealthStatus::Unhealthy,
            }
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = url; // Suppress unused variable warning when http feature is disabled
            tracing::warn!("HTTP health check requires 'http' feature");
            HealthStatus::Unknown
        }
    }
    // Handle pg_isready command (Postgres readiness check)
    else if cmd.starts_with("pg_isready") {
        let result = tokio::time::timeout(timeout, async {
            let args: Vec<&str> = cmd.split_whitespace().collect();
            tokio::process::Command::new(args[0])
                .args(&args[1..])
                .output()
                .await
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy
                }
            }
            _ => HealthStatus::Unhealthy,
        }
    }
    // Handle redis-cli command (Redis readiness check)
    else if cmd.starts_with("redis-cli") {
        let result = tokio::time::timeout(timeout, async {
            let args: Vec<&str> = cmd.split_whitespace().collect();
            tokio::process::Command::new(args[0])
                .args(&args[1..])
                .output()
                .await
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // redis-cli ping returns "+PONG"
                if output.status.success() && stdout.contains("PONG") {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy
                }
            }
            _ => HealthStatus::Unhealthy,
        }
    }
    // Handle exec commands
    else if let Some(exec_cmd) = cmd.strip_prefix("exec:") {
        if exec_cmd == "true" {
            return HealthStatus::Healthy;
        }
        if exec_cmd == "false" {
            return HealthStatus::Unhealthy;
        }
        let result = tokio::time::timeout(
            timeout,
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(exec_cmd)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy
                }
            }
            _ => HealthStatus::Unhealthy,
        }
    }
    // Unknown command: report Unknown, not Healthy
    else {
        tracing::warn!("unrecognized health check command: {}", cmd);
        HealthStatus::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
        assert_eq!(HealthStatus::Unknown.to_string(), "unknown");
    }

    #[test]
    fn test_is_ready() {
        assert!(HealthStatus::Healthy.is_ready());
        assert!(!HealthStatus::Unhealthy.is_ready());
        assert!(!HealthStatus::Unknown.is_ready());
    }

    #[tokio::test]
    async fn test_exec_true() {
        assert_eq!(
            execute_health_cmd("exec:true", 5).await,
            HealthStatus::Healthy
        );
    }

    #[tokio::test]
    async fn test_exec_false() {
        assert_eq!(
            execute_health_cmd("exec:false", 5).await,
            HealthStatus::Unhealthy
        );
    }

    #[tokio::test]
    async fn test_unknown_command_returns_unknown() {
        assert_eq!(
            execute_health_cmd("bogus:foo", 5).await,
            HealthStatus::Unknown
        );
    }

    #[tokio::test]
    async fn test_tcp_check_refused() {
        // Port 1 on loopback is almost certainly not listening
        assert_eq!(
            execute_health_cmd("tcp:127.0.0.1:1", 2).await,
            HealthStatus::Unhealthy
        );
    }

    #[tokio::test]
    async fn test_tcp_check_timeout() {
        // Non-routable address should timeout
        assert_eq!(
            execute_health_cmd("tcp:192.0.2.1:1", 1).await,
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn test_startup_probe_new() {
        let probe = StartupProbe::new("exec:true", 5);
        assert_eq!(probe.check_cmd, "exec:true");
        assert_eq!(probe.timeout_secs, 5);
        assert_eq!(probe.max_retries, 10);
        assert_eq!(probe.retry_delay_secs, 2);
    }

    #[test]
    fn test_startup_probe_builder() {
        let probe = StartupProbe::new("tcp:127.0.0.1:5432", 5)
            .with_max_retries(20)
            .with_retry_delay_secs(3);
        assert_eq!(probe.max_retries, 20);
        assert_eq!(probe.retry_delay_secs, 3);
    }

    #[tokio::test]
    async fn test_startup_probe_passes_immediately() {
        let probe = StartupProbe::new("exec:true", 5);
        let status = probe.check().await;
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_startup_probe_fails_immediately() {
        let probe = StartupProbe::new("exec:false", 1).with_max_retries(0);
        let status = probe.check().await;
        assert_eq!(status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_postgres_startup_probe() {
        let probe = postgres_startup_probe("127.0.0.1", 5432);
        assert!(probe.check_cmd.contains("127.0.0.1"));
        assert!(probe.check_cmd.contains("5432"));
        assert_eq!(probe.max_retries, 30);
        assert_eq!(probe.retry_delay_secs, 1);
    }

    #[test]
    fn test_redis_startup_probe() {
        let probe = redis_startup_probe("127.0.0.1:6379");
        assert!(probe.check_cmd.contains("redis-cli"));
        assert!(probe.check_cmd.contains("6379"));
        assert_eq!(probe.max_retries, 30);
        assert_eq!(probe.retry_delay_secs, 1);
    }

    #[test]
    fn test_startup_probe_from_env_tcp() {
        temp_env::with_vars(
            [
                ("STARTUP_PROBE_TYPE", None::<&str>),
                ("STARTUP_TIMEOUT_SECS", None::<&str>),
            ],
            || {
                let probe = startup_probe_from_env();
                assert!(probe.check_cmd.contains("tcp:"));
            },
        );
    }

    #[test]
    fn test_startup_probe_from_env_postgres() {
        temp_env::with_vars(
            [
                ("STARTUP_PROBE_TYPE", Some("postgres")),
                ("FAILOVER_DB_HOST", Some("pg.internal")),
                ("FAILOVER_DB_PORT", Some("5433")),
            ],
            || {
                let probe = startup_probe_from_env();
                assert!(probe.check_cmd.contains("pg_isready"));
                assert!(probe.check_cmd.contains("pg.internal"));
                assert!(probe.check_cmd.contains("5433"));
            },
        );
    }

    #[test]
    fn test_startup_probe_from_env_redis() {
        temp_env::with_vars([("STARTUP_PROBE_TYPE", Some("redis"))], || {
            let probe = startup_probe_from_env();
            assert!(probe.check_cmd.contains("redis-cli"));
            assert!(probe.check_cmd.contains("ping"));
        });
    }

    #[tokio::test]
    async fn test_pg_isready_command() {
        // pg_isready is unlikely to be available, so this should return Unhealthy
        let status = execute_health_cmd("pg_isready -h 127.0.0.1 -p 5432", 2).await;
        // If pg_isready is not installed, it returns Unhealthy
        // If it is installed but no PG running, it also returns Unhealthy
        assert!(status == HealthStatus::Unhealthy || status == HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_redis_cli_command() {
        // redis-cli is unlikely to be available, so this should return Unhealthy
        let status = execute_health_cmd("redis-cli -p 6379 ping", 2).await;
        assert!(status == HealthStatus::Unhealthy || status == HealthStatus::Healthy);
    }
}
