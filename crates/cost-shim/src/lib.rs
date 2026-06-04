//! Cost shim — resource tracking per tenant.
//!
//! Tracks resource usage (CPU, memory, storage) per tenant for billing.
//!
//! ## Environment Variables
//!
//! ```text
//! COST_TRACKING_ENABLED  Enable tracking (default: true)
//! COST_TENANT_KEY        Header/key for tenant identification
//! COST_REPORT_SCHEDULE   Report schedule (default: daily)
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Cost shim.
pub struct CostShim {
    enabled: bool,
    tenant_key: String,
    report_schedule: String,
    tenants_tracked: u64,
    resources_tracked: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CostShim {
    pub fn new() -> Self {
        Self {
            enabled: std::env::var("COST_TRACKING_ENABLED").map(|v| v == "true" || v == "1").unwrap_or(true),
            tenant_key: std::env::var("COST_TENANT_KEY").unwrap_or_else(|_| "X-Tenant-ID".to_string()),
            report_schedule: std::env::var("COST_REPORT_SCHEDULE").unwrap_or_else(|_| "daily".to_string()),
            tenants_tracked: 0, resources_tracked: 0, shutdown_tx: None,
        }
    }
}

impl Default for CostShim { fn default() -> Self { Self::new() } }

#[async_trait::async_trait]
impl Capability for CostShim {
    fn name(&self) -> &str { "cost" }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("CostShim initialized (enabled={}, key={})", self.enabled, self.tenant_key);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("CostShim started (enabled={})", self.enabled);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(true); }
        tracing::info!("CostShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("cost_tenants_tracked", self.tenants_tracked as f64),
            Metric::new("cost_resources_tracked", self.resources_tracked as f64),
        ]
    }
}
