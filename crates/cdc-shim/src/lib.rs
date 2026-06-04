//! CdcShimShim for EvergreenShims.
//!
//! TODO: Implement CdcShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// CdcShimShim.
pub struct CdcShimShim {
    config: Option<Config>,
}

impl CdcShimShim {
    /// Create a new CdcShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for CdcShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for CdcShimShim {
    fn name(&self) -> &str {
        "cdc-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("CdcShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("CdcShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("CdcShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
