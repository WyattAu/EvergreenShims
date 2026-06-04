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
