//! MigrationShimShim for EvergreenShims.
//!
//! TODO: Implement MigrationShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// MigrationShimShim.
pub struct MigrationShimShim {
    config: Option<Config>,
}

impl MigrationShimShim {
    /// Create a new MigrationShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for MigrationShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for MigrationShimShim {
    fn name(&self) -> &str {
        "migration-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("MigrationShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("MigrationShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("MigrationShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
