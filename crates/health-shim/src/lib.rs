//! Health shim for EvergreenShims.
//!
//! Provides health probes, metrics, and process management.
//!
//! ## Environment Variables
//!
//! ```text
//! HEALTH_WEBHOOK_URL          Webhook URL to push health status via POST
//! HEALTH_WEBHOOK_INTERVAL_SECS Push interval in seconds (default: 30)
//! ```

pub mod checker;
pub mod server;

use shim_core::{Capability, Config, Metric, Result};

use checker::HealthChecker;
use server::HealthServer;
use shim_core::CommandHealthCheck;

/// Health shim that provides liveness/readiness probes and metrics.
pub struct HealthShim {
    checker: Option<HealthChecker>,
    listen: String,
}

impl HealthShim {
    /// Create a new health shim.
    pub fn new() -> Self {
        Self {
            checker: None,
            listen: "0.0.0.0:9101".to_string(),
        }
    }
}

impl Default for HealthShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for HealthShim {
    fn name(&self) -> &str {
        "health"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        if shim_core::config::validation_enabled() {
            let errors = config.validate();
            let health_errors: Vec<_> = errors
                .iter()
                .filter(|e| e.field.starts_with("health."))
                .collect();
            if !health_errors.is_empty() {
                let msg = health_errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(shim_core::Error::Config(format!(
                    "health config validation failed: {}",
                    msg
                )));
            }
        }

        self.listen = config.health.listen.clone();

        let health_check = Box::new(CommandHealthCheck {
            liveness_cmd: config.health.liveness_cmd.clone(),
            readiness_cmd: config.health.readiness_cmd.clone(),
            timeout_secs: config.health.timeout_secs,
        });

        self.checker = Some(HealthChecker::new(health_check));
        tracing::info!("HealthShim initialized (listen={})", self.listen);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        if let Some(checker) = self.checker.take() {
            let server = HealthServer::new(checker, &self.listen);
            let listen = self.listen.clone();

            // Spawn server in background
            tokio::spawn(async move {
                if let Err(e) = server.start().await {
                    tracing::error!("Health server error: {}", e);
                }
            });

            tracing::info!("HealthShim started on {}", listen);
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("HealthShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![]
    }
}

/// Health status payload sent to the webhook.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthPayload {
    /// Current liveness status ("healthy" or "unhealthy").
    pub liveness: String,
    /// Current readiness status ("healthy" or "unhealthy").
    pub readiness: String,
    /// ISO 8601 timestamp of when the payload was created.
    pub timestamp: String,
    /// Name of the shim that produced this payload.
    pub shim: String,
}

/// Webhook-based health exporter that pushes liveness/readiness status
/// to a configurable URL on an interval.
pub struct HealthExporter {
    webhook_url: String,
    interval_secs: u64,
}

impl HealthExporter {
    /// Create a new exporter from environment variables.
    ///
    /// Returns `None` if `HEALTH_WEBHOOK_URL` is not set.
    pub fn from_env() -> Option<Self> {
        let webhook_url = std::env::var("HEALTH_WEBHOOK_URL").ok()?;
        if webhook_url.is_empty() {
            return None;
        }
        let interval_secs = std::env::var("HEALTH_WEBHOOK_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        Some(Self {
            webhook_url,
            interval_secs,
        })
    }

    /// Create a new exporter with explicit parameters.
    pub fn new(webhook_url: String, interval_secs: u64) -> Self {
        Self {
            webhook_url,
            interval_secs,
        }
    }

    /// Build the health payload from the given checker state.
    pub fn build_payload(liveness: &str, readiness: &str) -> HealthPayload {
        HealthPayload {
            liveness: liveness.to_string(),
            readiness: readiness.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            shim: "health".to_string(),
        }
    }

    /// Push health status to the webhook.
    pub async fn push(&self, payload: &HealthPayload) -> anyhow::Result<()> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;
        let resp = client.post(&self.webhook_url).json(payload).send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("Webhook returned status {}", resp.status());
        }
        tracing::debug!("Health status pushed to {}", self.webhook_url);
        Ok(())
    }

    /// Spawn a background task that checks health and pushes to the webhook
    /// on the configured interval.
    pub fn spawn_background(self, mut checker: HealthChecker) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(self.interval_secs));
            loop {
                interval.tick().await;
                let liveness = checker.check_liveness().await;
                let readiness = checker.check_readiness().await;
                let liveness_str = format!("{:?}", liveness).to_lowercase();
                let readiness_str = format!("{:?}", readiness).to_lowercase();
                let payload = Self::build_payload(&liveness_str, &readiness_str);
                if let Err(e) = self.push(&payload).await {
                    tracing::warn!("Health webhook push failed: {}", e);
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shim_core::{Config, HealthConfig};

    #[test]
    fn test_health_shim_new() {
        let shim = HealthShim::new();
        assert_eq!(shim.name(), "health");
        assert!(shim.checker.is_none());
        assert_eq!(shim.listen, "0.0.0.0:9101");
    }

    #[test]
    fn test_health_shim_default_trait() {
        let shim = HealthShim::default();
        assert_eq!(shim.name(), "health");
        assert!(shim.checker.is_none());
    }

    #[tokio::test]
    async fn test_health_shim_init() {
        let mut shim = HealthShim::new();
        let config = Config {
            health: HealthConfig {
                liveness_cmd: "echo ok".to_string(),
                readiness_cmd: "echo ready".to_string(),
                listen: "127.0.0.1:9999".to_string(),
                interval_secs: 5,
                timeout_secs: 3,
            },
            ..Default::default()
        };

        let result = shim.init(&config).await;
        assert!(result.is_ok());
        assert!(shim.checker.is_some());
        assert_eq!(shim.listen, "127.0.0.1:9999");
    }

    #[tokio::test]
    async fn test_health_shim_stop_without_start() {
        let mut shim = HealthShim::new();
        let result = shim.stop().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_health_shim_metrics_empty() {
        let shim = HealthShim::new();
        let metrics = shim.metrics();
        assert!(metrics.is_empty());
    }

    #[test]
    fn test_health_exporter_from_env_returns_none_without_url() {
        temp_env::with_var_unset("HEALTH_WEBHOOK_URL", || {
            assert!(HealthExporter::from_env().is_none());
        });
    }

    #[test]
    fn test_health_exporter_from_env_returns_none_empty_url() {
        temp_env::with_var("HEALTH_WEBHOOK_URL", Some(""), || {
            assert!(HealthExporter::from_env().is_none());
        });
    }

    #[test]
    fn test_health_exporter_from_env_returns_some_with_url() {
        temp_env::with_var(
            "HEALTH_WEBHOOK_URL",
            Some("http://localhost:8080/webhook"),
            || {
                let exporter = HealthExporter::from_env().unwrap();
                assert_eq!(exporter.webhook_url, "http://localhost:8080/webhook");
                assert_eq!(exporter.interval_secs, 30);
            },
        );
    }

    #[test]
    fn test_health_exporter_custom_interval() {
        temp_env::with_vars(
            [
                ("HEALTH_WEBHOOK_URL", Some("http://localhost:8080/webhook")),
                ("HEALTH_WEBHOOK_INTERVAL_SECS", Some("10")),
            ],
            || {
                let exporter = HealthExporter::from_env().unwrap();
                assert_eq!(exporter.interval_secs, 10);
            },
        );
    }

    #[test]
    fn test_health_exporter_invalid_interval_defaults_to_30() {
        temp_env::with_vars(
            [
                ("HEALTH_WEBHOOK_URL", Some("http://localhost:8080/webhook")),
                ("HEALTH_WEBHOOK_INTERVAL_SECS", Some("not_a_number")),
            ],
            || {
                let exporter = HealthExporter::from_env().unwrap();
                assert_eq!(exporter.interval_secs, 30);
            },
        );
    }

    #[test]
    fn test_health_exporter_new_explicit() {
        let exporter = HealthExporter::new("http://example.com/hook".to_string(), 60);
        assert_eq!(exporter.webhook_url, "http://example.com/hook");
        assert_eq!(exporter.interval_secs, 60);
    }

    #[test]
    fn test_health_exporter_build_payload() {
        let payload = HealthExporter::build_payload("healthy", "unhealthy");
        assert_eq!(payload.liveness, "healthy");
        assert_eq!(payload.readiness, "unhealthy");
        assert_eq!(payload.shim, "health");
        assert!(!payload.timestamp.is_empty());
    }

    #[test]
    fn test_health_payload_serializes_to_json() {
        let payload = HealthExporter::build_payload("healthy", "healthy");
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["liveness"], "healthy");
        assert_eq!(json["readiness"], "healthy");
        assert_eq!(json["shim"], "health");
        assert!(json["timestamp"].is_string());
    }
}
