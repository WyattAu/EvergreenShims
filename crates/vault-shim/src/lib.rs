//! VaultShimShim for EvergreenShims.
//!
//! TODO: Implement VaultShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// VaultShimShim.
pub struct VaultShimShim {
    config: Option<Config>,
}

impl VaultShimShim {
    /// Create a new VaultShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for VaultShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for VaultShimShim {
    fn name(&self) -> &str {
        "vault-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("VaultShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("VaultShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("VaultShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
