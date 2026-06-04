//! ArchivalShimShim for EvergreenShims.
//!
//! TODO: Implement ArchivalShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// ArchivalShimShim.
pub struct ArchivalShimShim {
    config: Option<Config>,
}

impl ArchivalShimShim {
    /// Create a new ArchivalShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for ArchivalShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ArchivalShimShim {
    fn name(&self) -> &str {
        "archival-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("ArchivalShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("ArchivalShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("ArchivalShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
