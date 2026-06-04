//! QueueShimShim for EvergreenShims.
//!
//! TODO: Implement QueueShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// QueueShimShim.
pub struct QueueShimShim {
    config: Option<Config>,
}

impl QueueShimShim {
    /// Create a new QueueShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for QueueShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for QueueShimShim {
    fn name(&self) -> &str {
        "queue-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("QueueShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("QueueShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("QueueShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
