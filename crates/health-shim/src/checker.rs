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

#[cfg(test)]
mod tests {
    use super::*;
    use shim_core::CommandHealthCheck;

    /// A mock health check that always returns a configurable status.
    struct MockHealthCheck {
        liveness_status: HealthStatus,
        readiness_status: HealthStatus,
    }

    #[async_trait::async_trait]
    impl HealthCheck for MockHealthCheck {
        async fn liveness(&self) -> HealthStatus {
            self.liveness_status.clone()
        }
        async fn readiness(&self) -> HealthStatus {
            self.readiness_status.clone()
        }
    }

    #[test]
    fn test_checker_new_defaults_to_unknown() {
        let check = Box::new(MockHealthCheck {
            liveness_status: HealthStatus::Healthy,
            readiness_status: HealthStatus::Healthy,
        });
        let checker = HealthChecker::new(check);
        assert_eq!(*checker.last_liveness(), HealthStatus::Unknown);
        assert_eq!(*checker.last_readiness(), HealthStatus::Unknown);
    }

    #[tokio::test]
    async fn test_checker_liveness_updates_status() {
        let check = Box::new(MockHealthCheck {
            liveness_status: HealthStatus::Healthy,
            readiness_status: HealthStatus::Unknown,
        });
        let mut checker = HealthChecker::new(check);
        let status = checker.check_liveness().await;
        assert_eq!(status, HealthStatus::Healthy);
        assert_eq!(*checker.last_liveness(), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_checker_readiness_updates_status() {
        let check = Box::new(MockHealthCheck {
            liveness_status: HealthStatus::Unknown,
            readiness_status: HealthStatus::Unhealthy,
        });
        let mut checker = HealthChecker::new(check);
        let status = checker.check_readiness().await;
        assert_eq!(status, HealthStatus::Unhealthy);
        assert_eq!(*checker.last_readiness(), HealthStatus::Unhealthy);
    }

    #[test]
    fn test_checker_metrics_unknown_values() {
        let check = Box::new(MockHealthCheck {
            liveness_status: HealthStatus::Healthy,
            readiness_status: HealthStatus::Unhealthy,
        });
        let checker = HealthChecker::new(check);

        // Before any checks, both are Unknown -> 0.0
        let metrics = checker.metrics();
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].name, "health_liveness");
        assert_eq!(metrics[0].value, 0.0);
        assert_eq!(metrics[1].name, "health_readiness");
        assert_eq!(metrics[1].value, 0.0);
    }

    #[tokio::test]
    async fn test_checker_metrics_after_checks() {
        let check = Box::new(MockHealthCheck {
            liveness_status: HealthStatus::Healthy,
            readiness_status: HealthStatus::Healthy,
        });
        let mut checker = HealthChecker::new(check);

        checker.check_liveness().await;
        checker.check_readiness().await;

        let metrics = checker.metrics();
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].value, 1.0); // liveness healthy
        assert_eq!(metrics[1].value, 1.0); // readiness healthy
    }

    #[tokio::test]
    async fn test_checker_metrics_unhealthy() {
        let check = Box::new(MockHealthCheck {
            liveness_status: HealthStatus::Unhealthy,
            readiness_status: HealthStatus::Unhealthy,
        });
        let mut checker = HealthChecker::new(check);

        checker.check_liveness().await;
        checker.check_readiness().await;

        let metrics = checker.metrics();
        assert_eq!(metrics[0].value, 0.0); // liveness unhealthy
        assert_eq!(metrics[1].value, 0.0); // readiness unhealthy
    }

    #[test]
    fn test_command_health_check_construction() {
        let check = CommandHealthCheck {
            liveness_cmd: "echo ok".to_string(),
            readiness_cmd: "echo ready".to_string(),
            timeout_secs: 5,
        };
        assert_eq!(check.liveness_cmd, "echo ok");
        assert_eq!(check.readiness_cmd, "echo ready");
        assert_eq!(check.timeout_secs, 5);
    }
}
