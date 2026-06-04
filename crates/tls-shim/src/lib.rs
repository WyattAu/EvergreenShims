//! TlsShimShim for EvergreenShims.
//!
//! TODO: Implement TlsShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// TlsShimShim.
pub struct TlsShimShim {
    config: Option<Config>,
}

impl TlsShimShim {
    /// Create a new TlsShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for TlsShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for TlsShimShim {
    fn name(&self) -> &str {
        "tls-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("TlsShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("TlsShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("TlsShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
