//! ChaosShimShim for EvergreenShims.
//!
//! TODO: Implement ChaosShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// ChaosShimShim.
pub struct ChaosShimShim {
    config: Option<Config>,
}

impl ChaosShimShim {
    /// Create a new ChaosShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for ChaosShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ChaosShimShim {
    fn name(&self) -> &str {
        "chaos-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("ChaosShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("ChaosShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("ChaosShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
