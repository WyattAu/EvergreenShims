//! BackupShimShim for EvergreenShims.
//!
//! TODO: Implement BackupShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// BackupShimShim.
pub struct BackupShimShim {
    config: Option<Config>,
}

impl BackupShimShim {
    /// Create a new BackupShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for BackupShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for BackupShimShim {
    fn name(&self) -> &str {
        "backup-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("BackupShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("BackupShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("BackupShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
