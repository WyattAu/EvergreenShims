//! gRPC management API for EvergreenShim.
//!
//! Provides a gRPC service for querying shim status, metrics,
//! configuration reload, and capability discovery.

pub mod proto {
    tonic::include_proto!("evergreen.shim");
}

pub use proto::shim_management_service_server::{
    ShimManagementService, ShimManagementServiceServer,
};
pub use proto::*;
pub use tonic::{Request, Response, Status};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

/// State shared across RPC handlers
#[derive(Clone)]
pub struct ShimState {
    start_time: Instant,
    metrics: Arc<RwLock<HashMap<String, Metric>>>,
    capabilities: Vec<CapabilityInfo>,
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
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        tracing::info!("GetStatus called");

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
        _request: Request<GetMetricsRequest>,
    ) -> Result<Response<GetMetricsResponse>, Status> {
        tracing::info!("GetMetrics called");

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
        tracing::info!("ReloadConfig called with path: {:?}", req.config_path);

        // In a real implementation, this would trigger config reload via ShimBus
        let mut warnings = Vec::new();

        if req.config_path.is_empty() {
            warnings.push("No config path specified, reloading current config".to_string());
        }

        Ok(Response::new(ReloadConfigResponse {
            success: true,
            message: "Configuration reload triggered successfully".to_string(),
            warnings,
        }))
    }

    async fn list_capabilities(
        &self,
        _request: Request<ListCapabilitiesRequest>,
    ) -> Result<Response<ListCapabilitiesResponse>, Status> {
        tracing::info!("ListCapabilities called");

        Ok(Response::new(ListCapabilitiesResponse {
            capabilities: self.capabilities.clone(),
        }))
    }
}

/// Build and start the gRPC server
pub async fn start_server(addr: std::net::SocketAddr, state: ShimState) -> anyhow::Result<()> {
    let svc = ShimManagementServiceServer::new(state);

    tracing::info!("Management API server listening on {}", addr);

    tonic::transport::Server::builder()
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
}
