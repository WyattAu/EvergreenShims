//! ReplicationShimShim for EvergreenShims.
//!
//! TODO: Implement ReplicationShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// ReplicationShimShim.
pub struct ReplicationShimShim {
    config: Option<Config>,
}

impl ReplicationShimShim {
    /// Create a new ReplicationShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for ReplicationShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ReplicationShimShim {
    fn name(&self) -> &str {
        "replication-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("ReplicationShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("ReplicationShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("ReplicationShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
