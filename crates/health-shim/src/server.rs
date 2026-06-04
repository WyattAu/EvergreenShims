//! Health server implementation.

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::checker::HealthChecker;
use shim_core::health::HealthStatus;

/// Shared state for the health server.
#[derive(Clone)]
struct HealthState {
    checker: Arc<Mutex<HealthChecker>>,
}

/// Health server.
pub struct HealthServer {
    state: HealthState,
    listen: String,
}

impl HealthServer {
    /// Create a new health server.
    pub fn new(checker: HealthChecker, listen: &str) -> Self {
        Self {
            state: HealthState {
                checker: Arc::new(Mutex::new(checker)),
            },
            listen: listen.to_string(),
        }
    }

    /// Start the health server.
    pub async fn start(&self) -> anyhow::Result<()> {
        let state = self.state.clone();

        let app = Router::new()
            .route("/livez", get(liveness_handler))
            .route("/readyz", get(readiness_handler))
            .route("/metrics", get(metrics_handler))
            .with_state(state);

        let listener = TcpListener::bind(&self.listen).await?;
        tracing::info!("Health server listening on {}", self.listen);

        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Liveness endpoint.
async fn liveness_handler(State(state): State<HealthState>) -> StatusCode {
    let mut checker = state.checker.lock().await;
    match checker.check_liveness().await {
        HealthStatus::Healthy => StatusCode::OK,
        HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
        HealthStatus::Unknown => StatusCode::OK,
    }
}

/// Readiness endpoint.
async fn readiness_handler(State(state): State<HealthState>) -> StatusCode {
    let mut checker = state.checker.lock().await;
    match checker.check_readiness().await {
        HealthStatus::Healthy => StatusCode::OK,
        HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
        HealthStatus::Unknown => StatusCode::OK,
    }
}

/// Metrics endpoint.
async fn metrics_handler(State(state): State<HealthState>) -> Json<Value> {
    let checker = state.checker.lock().await;
    let metrics = checker.metrics();

    let metrics_json: Vec<Value> = metrics
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "value": m.value,
                "labels": m.labels,
            })
        })
        .collect();

    Json(json!({
        "metrics": metrics_json,
    }))
}
