//! Metrics types for shims.

use prometheus::{Encoder, Registry, TextEncoder};
use serde::{Deserialize, Serialize};

/// A single metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    /// Metric name.
    pub name: String,

    /// Metric value.
    pub value: f64,

    /// Metric labels.
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

impl Metric {
    /// Create a new metric.
    pub fn new(name: &str, value: f64) -> Self {
        Self {
            name: name.to_string(),
            value,
            labels: std::collections::HashMap::new(),
        }
    }

    /// Create a metric with labels.
    pub fn with_labels(name: &str, value: f64, labels: std::collections::HashMap<String, String>) -> Self {
        Self {
            name: name.to_string(),
            value,
            labels,
        }
    }
}

/// Metrics collector.
pub struct MetricsCollector {
    registry: Registry,
}

impl MetricsCollector {
    /// Create a new metrics collector.
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
        }
    }

    /// Register a metric.
    pub fn register(&self, metric: Metric) {
        // In a real implementation, this would register with Prometheus
        tracing::debug!("Registered metric: {} = {}", metric.name, metric.value);
    }

    /// Collect all metrics.
    pub fn collect(&self) -> Vec<Metric> {
        // In a real implementation, this would collect from Prometheus
        vec![]
    }

    /// Export metrics as Prometheus text format.
    pub fn export(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}
