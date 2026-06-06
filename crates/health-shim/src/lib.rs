//! Health shim for EvergreenShims.
//!
//! Provides health probes, metrics, and process management.

pub mod checker;
pub mod server;

use shim_core::{Capability, Config, Metric, Result};

use checker::HealthChecker;
use server::HealthServer;
use shim_core::CommandHealthCheck;

/// Health shim that provides liveness/readiness probes and metrics.
pub struct HealthShim {
    checker: Option<HealthChecker>,
    listen: String,
}

impl HealthShim {
    /// Create a new health shim.
    pub fn new() -> Self {
        Self {
            checker: None,
            listen: "0.0.0.0:9101".to_string(),
        }
    }
}

impl Default for HealthShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for HealthShim {
    fn name(&self) -> &str {
        "health"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.listen = config.health.listen.clone();

        let health_check = Box::new(CommandHealthCheck {
            liveness_cmd: config.health.liveness_cmd.clone(),
            readiness_cmd: config.health.readiness_cmd.clone(),
            timeout_secs: config.health.timeout_secs,
        });

        self.checker = Some(HealthChecker::new(health_check));
        tracing::info!("HealthShim initialized (listen={})", self.listen);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        if let Some(checker) = self.checker.take() {
            let server = HealthServer::new(checker, &self.listen);
            let listen = self.listen.clone();

            // Spawn server in background
            tokio::spawn(async move {
                if let Err(e) = server.start().await {
                    tracing::error!("Health server error: {}", e);
                }
            });

            tracing::info!("HealthShim started on {}", listen);
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("HealthShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shim_core::{Config, HealthConfig};

    #[test]
    fn test_health_shim_new() {
        let shim = HealthShim::new();
        assert_eq!(shim.name(), "health");
        assert!(shim.checker.is_none());
        assert_eq!(shim.listen, "0.0.0.0:9101");
    }

    #[test]
    fn test_health_shim_default_trait() {
        let shim = HealthShim::default();
        assert_eq!(shim.name(), "health");
        assert!(shim.checker.is_none());
    }

    #[tokio::test]
    async fn test_health_shim_init() {
        let mut shim = HealthShim::new();
        let config = Config {
            health: HealthConfig {
                liveness_cmd: "echo ok".to_string(),
                readiness_cmd: "echo ready".to_string(),
                listen: "127.0.0.1:9999".to_string(),
                interval_secs: 5,
                timeout_secs: 3,
            },
            ..Default::default()
        };

        let result = shim.init(&config).await;
        assert!(result.is_ok());
        assert!(shim.checker.is_some());
        assert_eq!(shim.listen, "127.0.0.1:9999");
    }

    #[tokio::test]
    async fn test_health_shim_stop_without_start() {
        let mut shim = HealthShim::new();
        let result = shim.stop().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_health_shim_metrics_empty() {
        let shim = HealthShim::new();
        let metrics = shim.metrics();
        assert!(metrics.is_empty());
    }
}
