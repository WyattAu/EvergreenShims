//! FailoverShimShim for EvergreenShims.
//!
//! TODO: Implement FailoverShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// FailoverShimShim.
pub struct FailoverShimShim {
    config: Option<Config>,
}

impl FailoverShimShim {
    /// Create a new FailoverShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for FailoverShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for FailoverShimShim {
    fn name(&self) -> &str {
        "failover-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("FailoverShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("FailoverShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("FailoverShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
