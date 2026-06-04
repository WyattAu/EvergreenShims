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
    // Handle special commands
    if let Some(exec_cmd) = cmd.strip_prefix("exec:") {
        if exec_cmd == "true" {
            return HealthStatus::Healthy;
        }
        if exec_cmd == "false" {
            return HealthStatus::Unhealthy;
        }
    }

    // Handle TCP checks
    if let Some(addr) = cmd.strip_prefix("tcp:") {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(_) => HealthStatus::Healthy,
            Err(_) => HealthStatus::Unhealthy,
        }
    }
    // Handle HTTP checks (requires "http" feature)
    else if let Some(url) = cmd.strip_prefix("http:") {
        #[cfg(feature = "http")]
        {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
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
            std::time::Duration::from_secs(timeout_secs),
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
    // Default: healthy
    else {
        HealthStatus::Healthy
    }
}
