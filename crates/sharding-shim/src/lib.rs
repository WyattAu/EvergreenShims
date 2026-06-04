//! ShardingShimShim for EvergreenShims.
//!
//! TODO: Implement ShardingShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// ShardingShimShim.
pub struct ShardingShimShim {
    config: Option<Config>,
}

impl ShardingShimShim {
    /// Create a new ShardingShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for ShardingShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ShardingShimShim {
    fn name(&self) -> &str {
        "sharding-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("ShardingShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("ShardingShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("ShardingShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
