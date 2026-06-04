//! SchedulerShimShim for EvergreenShims.
//!
//! TODO: Implement SchedulerShimShim.

use shim_core::{Capability, Config, Metric, Result};

/// SchedulerShimShim.
pub struct SchedulerShimShim {
    config: Option<Config>,
}

impl SchedulerShimShim {
    /// Create a new SchedulerShimShim.
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for SchedulerShimShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for SchedulerShimShim {
    fn name(&self) -> &str {
        "scheduler-shim"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("SchedulerShimShim initialized");
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        tracing::info!("SchedulerShimShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("SchedulerShimShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}
