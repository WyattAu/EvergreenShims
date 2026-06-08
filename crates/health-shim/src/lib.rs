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

use std::collections::HashMap;

use shim_core::{Capability, Config, Metric, Result};

use checker::HealthChecker;
use server::HealthServer;
use shim_core::CommandHealthCheck;

/// Health shim that provides liveness/readiness probes and metrics.
pub struct HealthShim {
    checker: Option<HealthChecker>,
    listen: String,
    /// Registered capability health checkers for per-capability breakdown.
    capabilities: HashMap<String, CapabilityHealthStatus>,
    /// Whether initialization is complete.
    initialized: bool,
}

/// Health status for a specific capability.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CapabilityHealthStatus {
    /// Name of the capability.
    pub name: String,
    /// Whether the capability is healthy.
    pub healthy: bool,
    /// Optional status message.
    pub message: Option<String>,
}

/// Detailed health status response with per-capability breakdown.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DetailedHealthStatus {
    /// Overall liveness status.
    pub liveness: String,
    /// Overall readiness status.
    pub readiness: String,
    /// Startup probe status.
    pub startup: String,
    /// Per-capability health breakdown.
    pub capabilities: Vec<CapabilityHealthStatus>,
    /// Process uptime in seconds.
    pub uptime_secs: u64,
    /// Whether the shim is initialized.
    pub initialized: bool,
}

impl HealthShim {
    /// Create a new health shim.
    pub fn new() -> Self {
        Self {
            checker: None,
            listen: "0.0.0.0:9101".to_string(),
            capabilities: HashMap::new(),
            initialized: false,
        }
    }

    /// Register a capability for health monitoring.
    pub fn register_capability(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.capabilities.insert(
            name.clone(),
            CapabilityHealthStatus {
                name,
                healthy: true,
                message: Some("registered".into()),
            },
        );
    }

    /// Update a capability's health status.
    pub fn set_capability_health(&mut self, name: &str, healthy: bool, message: Option<String>) {
        if let Some(cap) = self.capabilities.get_mut(name) {
            cap.healthy = healthy;
            cap.message = message;
        }
    }

    /// Get the detailed health status with per-capability breakdown.
    pub fn detailed_status(&self, uptime_secs: u64) -> DetailedHealthStatus {
        let all_healthy = self.capabilities.values().all(|c| c.healthy);
        let liveness = if all_healthy && self.initialized {
            "healthy".to_string()
        } else if self.initialized {
            "degraded".to_string()
        } else {
            "starting".to_string()
        };

        let readiness = if all_healthy && self.initialized {
            "ready".to_string()
        } else {
            "not_ready".to_string()
        };

        let startup = if self.initialized {
            "complete".to_string()
        } else {
            "in_progress".to_string()
        };

        DetailedHealthStatus {
            liveness,
            readiness,
            startup,
            capabilities: self.capabilities.values().cloned().collect(),
            uptime_secs,
            initialized: self.initialized,
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
        self.initialized = true;
        tracing::info!("HealthShim initialized (listen={})", self.listen);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        if let Some(checker) = self.checker.take() {
            let server = HealthServer::new(checker, &self.listen, self.capabilities.clone());
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
        assert!(shim.initialized);
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
    fn test_register_capability() {
        let mut shim = HealthShim::new();
        shim.register_capability("vault");
        assert!(shim.capabilities.contains_key("vault"));
        assert!(shim.capabilities.get("vault").unwrap().healthy);
    }

    #[test]
    fn test_set_capability_health() {
        let mut shim = HealthShim::new();
        shim.register_capability("backup");
        shim.set_capability_health("backup", false, Some("disk full".into()));

        let cap = shim.capabilities.get("backup").unwrap();
        assert!(!cap.healthy);
        assert_eq!(cap.message.as_deref(), Some("disk full"));
    }

    #[test]
    fn test_detailed_status_healthy() {
        let mut shim = HealthShim::new();
        shim.register_capability("health");
        shim.initialized = true;

        let status = shim.detailed_status(120);
        assert_eq!(status.liveness, "healthy");
        assert_eq!(status.readiness, "ready");
        assert_eq!(status.startup, "complete");
        assert!(status.initialized);
        assert_eq!(status.uptime_secs, 120);
        assert_eq!(status.capabilities.len(), 1);
    }

    #[test]
    fn test_detailed_status_degraded() {
        let mut shim = HealthShim::new();
        shim.register_capability("vault");
        shim.set_capability_health("vault", false, None);
        shim.initialized = true;

        let status = shim.detailed_status(60);
        assert_eq!(status.liveness, "degraded");
        assert_eq!(status.readiness, "not_ready");
    }

    #[test]
    fn test_detailed_status_starting() {
        let shim = HealthShim::new();
        let status = shim.detailed_status(0);
        assert_eq!(status.liveness, "starting");
        assert_eq!(status.startup, "in_progress");
        assert!(!status.initialized);
    }

    #[test]
    fn test_detailed_status_serialization() {
        let mut shim = HealthShim::new();
        shim.register_capability("test");
        shim.initialized = true;

        let status = shim.detailed_status(100);
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["liveness"], "healthy");
        assert_eq!(json["startup"], "complete");
        assert!(json["capabilities"].is_array());
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
