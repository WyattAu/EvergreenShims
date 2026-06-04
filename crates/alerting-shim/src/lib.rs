//! Alerting shim — alert routing to PagerDuty, Slack, etc.
//!
//! Routes alerts based on severity and rules.
//!
//! ## Environment Variables
//!
//! ```text
//! ALERTING_WEBHOOKS      JSON array of webhook configs
//! ALERTING_SEVERITY_MAP  Map severity levels to alert channels
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Webhook configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub name: String,
    pub url: String,
    pub channel: String,
    pub min_severity: String,
}

/// Alerting shim.
pub struct AlertingShim {
    webhooks: Vec<WebhookConfig>,
    alerts_sent: u64,
    alerts_failed: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl AlertingShim {
    pub fn new() -> Self {
        Self {
            webhooks: std::env::var("ALERTING_WEBHOOKS")
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            alerts_sent: 0, alerts_failed: 0, shutdown_tx: None,
        }
    }
}

impl Default for AlertingShim { fn default() -> Self { Self::new() } }

#[async_trait::async_trait]
impl Capability for AlertingShim {
    fn name(&self) -> &str { "alerting" }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("AlertingShim initialized ({} webhooks)", self.webhooks.len());
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("AlertingShim started ({} webhooks)", self.webhooks.len());
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(true); }
        tracing::info!("AlertingShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("alerting_sent_total", self.alerts_sent as f64),
            Metric::new("alerting_failed_total", self.alerts_failed as f64),
        ]
    }
}
