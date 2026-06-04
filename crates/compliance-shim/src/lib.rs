//! ComplianceShimShim for EvergreenShims.
//!
//! TODO: Implement ComplianceShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// ComplianceShimShim.
pub struct ComplianceShimShim {
    config: Option<Config>,
}

impl ComplianceShimShim {
    /// Create a new ComplianceShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for ComplianceShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ComplianceShimShim {
    fn name(&self) -> &str {
        "compliance-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("ComplianceShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("ComplianceShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("ComplianceShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
