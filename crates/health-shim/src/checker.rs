//! Health checker implementation.

use shim_core::health::{HealthCheck, HealthStatus};
use shim_core::metrics::Metric;

/// Health checker for the managed application.
pub struct HealthChecker {
    /// Health check implementation.
    health_check: Box<dyn HealthCheck>,

    /// Last liveness status.
    last_liveness: HealthStatus,

    /// Last readiness status.
    last_readiness: HealthStatus,
}

impl HealthChecker {
    /// Create a new health checker.
    pub fn new(health_check: Box<dyn HealthCheck>) -> Self {
        Self {
            health_check,
            last_liveness: HealthStatus::Unknown,
            last_readiness: HealthStatus::Unknown,
        }
    }

    /// Check liveness.
    pub async fn check_liveness(&mut self) -> HealthStatus {
        let status = self.health_check.liveness().await;
        self.last_liveness = status.clone();
        status
    }

    /// Check readiness.
    pub async fn check_readiness(&mut self) -> HealthStatus {
        let status = self.health_check.readiness().await;
        self.last_readiness = status.clone();
        status
    }

    /// Get last liveness status.
    pub fn last_liveness(&self) -> &HealthStatus {
        &self.last_liveness
    }

    /// Get last readiness status.
    pub fn last_readiness(&self) -> &HealthStatus {
        &self.last_readiness
    }

    /// Get health metrics.
    pub fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::with_labels(
                "health_liveness",
                match &self.last_liveness {
                    HealthStatus::Healthy => 1.0,
                    _ => 0.0,
                },
                std::collections::HashMap::new(),
            ),
            Metric::with_labels(
                "health_readiness",
                match &self.last_readiness {
                    HealthStatus::Healthy => 1.0,
                    _ => 0.0,
                },
                std::collections::HashMap::new(),
            ),
        ]
    }
}
