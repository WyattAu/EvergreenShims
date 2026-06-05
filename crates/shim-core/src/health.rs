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

/// Execute a health check command.
async fn execute_health_cmd(cmd: &str, timeout_secs: u64) -> HealthStatus {
    let timeout = std::time::Duration::from_secs(timeout_secs);

    // Handle special commands
    if let Some(exec_cmd) = cmd.strip_prefix("exec:") {
        if exec_cmd == "true" {
            return HealthStatus::Healthy;
        }
        if exec_cmd == "false" {
            return HealthStatus::Unhealthy;
        }
    }

    // Handle TCP checks (with timeout)
    if let Some(addr) = cmd.strip_prefix("tcp:") {
        return match tokio::time::timeout(
            timeout,
            tokio::net::TcpStream::connect(addr),
        )
        .await
        {
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
            tracing::warn!("HTTP health check requires 'http' feature");
            HealthStatus::Unknown
        }
    }
    // Handle exec commands
    else if let Some(exec_cmd) = cmd.strip_prefix("exec:") {
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
}
