//! AuthShimShim for EvergreenShims.
//!
//! TODO: Implement AuthShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// AuthShimShim.
pub struct AuthShimShim {
    config: Option<Config>,
}

impl AuthShimShim {
    /// Create a new AuthShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for AuthShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for AuthShimShim {
    fn name(&self) -> &str {
        "auth-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("AuthShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("AuthShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("AuthShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
