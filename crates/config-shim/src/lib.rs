//! ConfigShimShim for EvergreenShims.
//!
//! TODO: Implement ConfigShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// ConfigShimShim.
pub struct ConfigShimShim {
    config: Option<Config>,
}

impl ConfigShimShim {
    /// Create a new ConfigShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for ConfigShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ConfigShimShim {
    fn name(&self) -> &str {
        "config-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("ConfigShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("ConfigShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("ConfigShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
