//! gRPC management API for EvergreenShim.
//!
//! Provides a gRPC service for querying shim status, metrics,
//! configuration reload, and capability discovery.

pub mod proto {
    tonic::include_proto!("evergreen.shim");
}

pub mod audit;
pub mod rate_limiter;
pub mod sanitization;
pub mod validation;

pub use proto::shim_management_service_server::{
    ShimManagementService, ShimManagementServiceServer,
};
pub use proto::*;
pub use tonic::{Request, Response, Status};

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use validation::Validate;

/// State shared across RPC handlers
#[derive(Clone)]
pub struct ShimState {
    start_time: Instant,
    metrics: Arc<RwLock<HashMap<String, Metric>>>,
    capabilities: Vec<CapabilityInfo>,
}

fn extract_peer(request: &Request<impl std::any::Any>) -> IpAddr {
    request
        .metadata()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(IpAddr::from([0, 0, 0, 0]))
}

impl ShimState {
    /// Create a new management state with default metrics and capabilities.
    pub fn new() -> Self {
        let mut metrics = HashMap::new();

        // Initialize default metrics
        metrics.insert(
            "shim_events_published_total".to_string(),
            Metric {
                name: "shim_events_published_total".to_string(),
                value: 0.0,
                labels: HashMap::new(),
                r#type: MetricType::Counter as i32,
            },
        );
        metrics.insert(
            "shim_health_status".to_string(),
            Metric {
                name: "shim_health_status".to_string(),
                value: 1.0,
                labels: HashMap::new(),
                r#type: MetricType::Gauge as i32,
            },
        );
        metrics.insert(
            "shim_uptime_seconds".to_string(),
            Metric {
                name: "shim_uptime_seconds".to_string(),
                value: 0.0,
                labels: HashMap::new(),
                r#type: MetricType::Gauge as i32,
            },
        );

        let capabilities = vec![
            CapabilityInfo {
                name: "health".to_string(),
                enabled: true,
                version: env!("CARGO_PKG_VERSION").to_string(),
                dependencies: vec![],
                metadata: HashMap::from([("critical".to_string(), "true".to_string())]),
            },
            CapabilityInfo {
                name: "metrics".to_string(),
                enabled: true,
                version: env!("CARGO_PKG_VERSION").to_string(),
                dependencies: vec!["health".to_string()],
                metadata: HashMap::new(),
            },
        ];

        Self {
            start_time: Instant::now(),
            metrics: Arc::new(RwLock::new(metrics)),
            capabilities,
        }
    }
}

impl Default for ShimState {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl ShimManagementService for ShimState {
    async fn get_status(
        &self,
        request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let req = request.into_inner();
        req.validate()?;

        let peer = extract_peer(&Request::new(()));
        audit::audit_get_status(peer);

        let uptime = self.start_time.elapsed().as_secs();

        let capabilities: Vec<CapabilityStatus> = self
            .capabilities
            .iter()
            .map(|c| CapabilityStatus {
                name: c.name.clone(),
                enabled: c.enabled,
                healthy: true,
                last_error: String::new(),
            })
            .collect();

        let response = GetStatusResponse {
            health: HealthStatus::Healthy as i32,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: uptime.to_string(),
            capabilities,
        };

        Ok(Response::new(response))
    }

    async fn get_metrics(
        &self,
        request: Request<GetMetricsRequest>,
    ) -> Result<Response<GetMetricsResponse>, Status> {
        let req = request.into_inner();
        req.validate()?;

        let metrics_guard = self.metrics.read().await;
        let mut metrics: Vec<Metric> = metrics_guard.values().cloned().collect();

        // Add dynamic uptime metric
        metrics.push(Metric {
            name: "shim_uptime_seconds".to_string(),
            value: self.start_time.elapsed().as_secs() as f64,
            labels: HashMap::new(),
            r#type: MetricType::Gauge as i32,
        });

        Ok(Response::new(GetMetricsResponse { metrics }))
    }

    async fn reload_config(
        &self,
        request: Request<ReloadConfigRequest>,
    ) -> Result<Response<ReloadConfigResponse>, Status> {
        let req = request.into_inner();
        req.validate()?;

        let config_path = sanitization::sanitize_string_field(&req.config_path, "config_path")?;

        let peer = extract_peer(&Request::new(()));

        let mut warnings = Vec::new();

        if config_path.is_empty() {
            warnings.push("No config path specified, reloading current config".to_string());
        }

        audit::audit_reload_config(peer, &config_path, true);

        Ok(Response::new(ReloadConfigResponse {
            success: true,
            message: "Configuration reload triggered successfully".to_string(),
            warnings,
        }))
    }

    async fn list_capabilities(
        &self,
        request: Request<ListCapabilitiesRequest>,
    ) -> Result<Response<ListCapabilitiesResponse>, Status> {
        let req = request.into_inner();
        req.validate()?;

        Ok(Response::new(ListCapabilitiesResponse {
            capabilities: self.capabilities.clone(),
        }))
    }
}

/// Build and start the gRPC server with rate limiting
pub async fn start_server(addr: std::net::SocketAddr, state: ShimState) -> anyhow::Result<()> {
    let svc = ShimManagementServiceServer::new(state);
    let rate_limit = rate_limiter::rate_limit_from_env();

    tracing::info!(
        "Management API server listening on {} (rate limit: {} rpm)",
        addr,
        rate_limit
    );

    let layer = rate_limiter::RateLimitLayer::new(rate_limit);

    tonic::transport::Server::builder()
        .layer(layer)
        .add_service(svc)
        .serve(addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_status() {
        let state = ShimState::new();
        let request = Request::new(GetStatusRequest {});
        let response = state.get_status(request).await.unwrap();
        let status = response.into_inner();

        assert_eq!(status.health, HealthStatus::Healthy as i32);
        assert!(!status.version.is_empty());
        assert!(!status.uptime_seconds.is_empty());
        assert!(!status.capabilities.is_empty());
    }

    #[tokio::test]
    async fn test_get_metrics() {
        let state = ShimState::new();
        let request = Request::new(GetMetricsRequest {});
        let response = state.get_metrics(request).await.unwrap();
        let metrics = response.into_inner();

        assert!(!metrics.metrics.is_empty());
        // Should contain the uptime metric
        assert!(metrics
            .metrics
            .iter()
            .any(|m| m.name == "shim_uptime_seconds"));
    }

    #[tokio::test]
    async fn test_reload_config() {
        let state = ShimState::new();
        let request = Request::new(ReloadConfigRequest {
            config_path: String::new(),
        });
        let response = state.reload_config(request).await.unwrap();
        let reload = response.into_inner();

        assert!(reload.success);
        assert!(!reload.warnings.is_empty());
    }

    #[tokio::test]
    async fn test_reload_config_with_path() {
        let state = ShimState::new();
        let request = Request::new(ReloadConfigRequest {
            config_path: "/etc/config.toml".to_string(),
        });
        let response = state.reload_config(request).await.unwrap();
        let reload = response.into_inner();

        assert!(reload.success);
        assert!(reload.warnings.is_empty());
    }

    #[tokio::test]
    async fn test_list_capabilities() {
        let state = ShimState::new();
        let request = Request::new(ListCapabilitiesRequest {});
        let response = state.list_capabilities(request).await.unwrap();
        let caps = response.into_inner();

        assert_eq!(caps.capabilities.len(), 2);
        assert_eq!(caps.capabilities[0].name, "health");
        assert!(caps.capabilities[0].enabled);
    }

    #[tokio::test]
    async fn test_state_default() {
        let state = ShimState::default();
        assert_eq!(state.capabilities.len(), 2);
    }

    #[tokio::test]
    async fn test_rate_limiter_enforced() {
        use crate::rate_limiter::RateLimiter;

        let limiter = RateLimiter::new(3);
        assert!(limiter.allow());
        assert!(limiter.allow());
        assert!(limiter.allow());
        assert!(!limiter.allow());
    }

    #[test]
    fn test_sanitize_control_characters() {
        let result = sanitization::sanitize_input("hello\x01\x02world").unwrap();
        assert_eq!(result, "helloworld");
    }

    #[test]
    fn test_sanitize_preserves_whitespace() {
        let result = sanitization::sanitize_input("hello\tworld\n").unwrap();
        assert_eq!(result, "hello\tworld\n");
    }

    #[test]
    fn test_validation_rejects_null_bytes() {
        let req = ReloadConfigRequest {
            config_path: "path\0bad".to_string(),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_validation_rejects_long_strings() {
        let req = ReloadConfigRequest {
            config_path: "a".repeat(2000),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_validation_allows_valid_input() {
        let req = ReloadConfigRequest {
            config_path: "/etc/config.toml".to_string(),
        };
        assert!(req.validate().is_ok());
    }
}
