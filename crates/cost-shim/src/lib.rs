//! CostShimShim for EvergreenShims.
//!
//! TODO: Implement CostShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// CostShimShim.
pub struct CostShimShim {
    config: Option<Config>,
}

impl CostShimShim {
    /// Create a new CostShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for CostShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for CostShimShim {
    fn name(&self) -> &str {
        "cost-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("CostShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("CostShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("CostShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
