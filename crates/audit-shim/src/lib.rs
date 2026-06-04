//! AuditShimShim for EvergreenShims.
//!
//! TODO: Implement AuditShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// AuditShimShim.
pub struct AuditShimShim {
    config: Option<Config>,
}

impl AuditShimShim {
    /// Create a new AuditShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for AuditShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for AuditShimShim {
    fn name(&self) -> &str {
        "audit-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("AuditShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("AuditShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("AuditShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
