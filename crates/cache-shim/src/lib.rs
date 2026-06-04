//! CacheShimShim for EvergreenShims.
//!
//! TODO: Implement CacheShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// CacheShimShim.
pub struct CacheShimShim {
    config: Option<Config>,
}

impl CacheShimShim {
    /// Create a new CacheShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for CacheShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for CacheShimShim {
    fn name(&self) -> &str {
        "cache-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("CacheShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("CacheShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("CacheShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
