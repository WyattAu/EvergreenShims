//! ProxyShimShim for EvergreenShims.
//!
//! TODO: Implement ProxyShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// ProxyShimShim.
pub struct ProxyShimShim {
    config: Option<Config>,
}

impl ProxyShimShim {
    /// Create a new ProxyShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for ProxyShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ProxyShimShim {
    fn name(&self) -> &str {
        "proxy-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("ProxyShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("ProxyShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("ProxyShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
