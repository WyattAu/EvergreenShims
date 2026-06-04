//! AlertingShimShim for EvergreenShims.
//!
//! TODO: Implement AlertingShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// AlertingShimShim.
pub struct AlertingShimShim {
    config: Option<Config>,
}

impl AlertingShimShim {
    /// Create a new AlertingShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for AlertingShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for AlertingShimShim {
    fn name(&self) -> &str {
        "alerting-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("AlertingShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("AlertingShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("AlertingShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
