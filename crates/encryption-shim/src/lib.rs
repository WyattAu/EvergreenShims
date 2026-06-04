//! EncryptionShimShim for EvergreenShims.
//!
//! TODO: Implement EncryptionShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// EncryptionShimShim.
pub struct EncryptionShimShim {
    config: Option<Config>,
}

impl EncryptionShimShim {
    /// Create a new EncryptionShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for EncryptionShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for EncryptionShimShim {
    fn name(&self) -> &str {
        "encryption-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("EncryptionShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("EncryptionShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("EncryptionShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
