//! Health server implementation.
//!
//! Provides:
//! - `/livez` liveness endpoint
//! - `/readyz` readiness endpoint
//! - `/startupz` startup probe endpoint
//! - `/healthz` detailed health status with per-capability breakdown
//! - `/metrics` metrics endpoint

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::checker::HealthChecker;
use crate::CapabilityHealthStatus;
use shim_core::health::HealthStatus;

/// Shared state for the health server.
#[derive(Clone)]
struct HealthState {
    checker: Arc<Mutex<HealthChecker>>,
    capabilities: HashMap<String, CapabilityHealthStatus>,
    start_time: Arc<Instant>,
    initialized: Arc<AtomicBool>,
    liveness_failures: Arc<AtomicU64>,
    readiness_failures: Arc<AtomicU64>,
}

/// Health server.
pub struct HealthServer {
    state: HealthState,
    listen: String,
}

impl HealthServer {
    /// Create a new health server.
    pub fn new(
        checker: HealthChecker,
        listen: &str,
        capabilities: HashMap<String, CapabilityHealthStatus>,
    ) -> Self {
        Self {
            state: HealthState {
                checker: Arc::new(Mutex::new(checker)),
                capabilities,
                start_time: Arc::new(Instant::now()),
                initialized: Arc::new(AtomicBool::new(true)),
                liveness_failures: Arc::new(AtomicU64::new(0)),
                readiness_failures: Arc::new(AtomicU64::new(0)),
            },
            listen: listen.to_string(),
        }
    }

    /// Create a new health server with initialization tracking.
    pub fn with_initialization(
        checker: HealthChecker,
        listen: &str,
        capabilities: HashMap<String, CapabilityHealthStatus>,
        initialized: Arc<AtomicBool>,
    ) -> Self {
        Self {
            state: HealthState {
                checker: Arc::new(Mutex::new(checker)),
                capabilities,
                start_time: Arc::new(Instant::now()),
                initialized,
                liveness_failures: Arc::new(AtomicU64::new(0)),
                readiness_failures: Arc::new(AtomicU64::new(0)),
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
            .route("/startupz", get(startup_handler))
            .route("/healthz", get(health_handler))
            .route("/metrics", get(metrics_handler))
            .with_state(state);

        let listener = TcpListener::bind(&self.listen).await?;
        tracing::info!("Health server listening on {}", self.listen);

        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Liveness endpoint - checks if the process is responsive.
async fn liveness_handler(State(state): State<HealthState>) -> StatusCode {
    let mut checker = state.checker.lock().await;
    match checker.check_liveness().await {
        HealthStatus::Healthy => StatusCode::OK,
        HealthStatus::Unhealthy => {
            state.liveness_failures.fetch_add(1, Ordering::Relaxed);
            StatusCode::SERVICE_UNAVAILABLE
        }
        HealthStatus::Unknown => StatusCode::OK,
    }
}

/// Readiness endpoint - checks if the process can serve traffic.
async fn readiness_handler(State(state): State<HealthState>) -> StatusCode {
    // Check if initialized
    if !state.initialized.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    // Check if all capabilities are healthy
    let all_healthy = state.capabilities.values().all(|c| c.healthy);
    if !all_healthy {
        state.readiness_failures.fetch_add(1, Ordering::Relaxed);
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    let mut checker = state.checker.lock().await;
    match checker.check_readiness().await {
        HealthStatus::Healthy => StatusCode::OK,
        HealthStatus::Unhealthy => {
            state.readiness_failures.fetch_add(1, Ordering::Relaxed);
            StatusCode::SERVICE_UNAVAILABLE
        }
        HealthStatus::Unknown => StatusCode::OK,
    }
}

/// Startup probe endpoint - checks if initialization is complete.
async fn startup_handler(State(state): State<HealthState>) -> StatusCode {
    if state.initialized.load(Ordering::Relaxed) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Detailed health endpoint with per-capability breakdown.
async fn health_handler(State(state): State<HealthState>) -> Json<Value> {
    let uptime_secs = state.start_time.elapsed().as_secs();
    let initialized = state.initialized.load(Ordering::Relaxed);

    let all_healthy = state.capabilities.values().all(|c| c.healthy);
    let liveness = if all_healthy && initialized {
        "healthy"
    } else if initialized {
        "degraded"
    } else {
        "starting"
    };

    let readiness = if all_healthy && initialized {
        "ready"
    } else {
        "not_ready"
    };

    let startup = if initialized {
        "complete"
    } else {
        "in_progress"
    };

    let capabilities: Vec<Value> = state
        .capabilities
        .values()
        .map(|c| {
            json!({
                "name": c.name,
                "healthy": c.healthy,
                "message": c.message,
            })
        })
        .collect();

    Json(json!({
        "liveness": liveness,
        "readiness": readiness,
        "startup": startup,
        "capabilities": capabilities,
        "uptime_secs": uptime_secs,
        "initialized": initialized,
        "liveness_failures": state.liveness_failures.load(Ordering::Relaxed),
        "readiness_failures": state.readiness_failures.load(Ordering::Relaxed),
    }))
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
