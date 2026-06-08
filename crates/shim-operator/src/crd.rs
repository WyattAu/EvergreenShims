//! CRD struct definitions mirroring the ShimConfig CRD YAML.
//!
//! The `CustomResource` derive on `ShimConfigSpec` generates the `ShimConfig` wrapper type.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// ShimConfigSpec defines the desired state of ShimConfig.
///
/// The `#[derive(CustomResource)]` generates a `ShimConfig` wrapper type
/// with `metadata`, `spec`, and `status` fields.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "evergreen.dev",
    version = "v1alpha1",
    kind = "ShimConfig",
    plural = "shimconfigs",
    shortname = "sc",
    shortname = "shimcfg",
    namespaced,
    status = "ShimConfigStatus",
    printcolumn = r#"{"name":"Child Command","type":"string","jsonPath":".spec.childCommand"}"#,
    printcolumn = r#"{"name":"Features","type":"string","jsonPath":".spec.features"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct ShimConfigSpec {
    /// Command to run as the child process.
    #[serde(rename = "childCommand")]
    pub child_command: String,

    /// Arguments for the child process.
    #[serde(default)]
    pub child_args: Vec<String>,

    /// List of shim features to enable (e.g., health, vault, backup).
    #[serde(default)]
    pub features: Vec<String>,

    /// Resource requirements for the shim sidecar.
    #[serde(default)]
    pub resources: Option<ResourceRequirements>,

    /// General shim configuration options.
    #[serde(default)]
    pub shim_config: Option<ShimConfigOptions>,
}

/// ResourceRequirements describes the compute resource requirements.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResourceRequirements {
    /// Minimum resources required.
    #[serde(default)]
    pub requests: Option<ResourceSpec>,
    /// Maximum resources allowed.
    #[serde(default)]
    pub limits: Option<ResourceSpec>,
}

/// ResourceSpec defines the amount of a particular resource.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResourceSpec {
    /// CPU amount (e.g., "100m", "0.5").
    #[serde(default)]
    pub cpu: Option<String>,
    /// Memory amount (e.g., "128Mi", "1Gi").
    #[serde(default)]
    pub memory: Option<String>,
}

/// ShimConfigOptions contains general shim configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShimConfigOptions {
    /// Logging level for the shim.
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Enable JSON structured logging.
    #[serde(default)]
    pub json_logging: bool,
    /// Port for Prometheus metrics endpoint.
    #[serde(default = "default_metrics_port")]
    pub metrics_port: u16,
    /// Port for health check endpoint.
    #[serde(default = "default_health_port")]
    pub health_port: u16,
    /// OpenTelemetry OTLP endpoint.
    #[serde(default)]
    pub otel_endpoint: Option<String>,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_metrics_port() -> u16 {
    9090
}

fn default_health_port() -> u16 {
    9091
}

impl Default for ShimConfigOptions {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            json_logging: false,
            metrics_port: default_metrics_port(),
            health_port: default_health_port(),
            otel_endpoint: None,
        }
    }
}

/// ShimConfigStatus defines the observed state of ShimConfig.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ShimConfigStatus {
    /// Current conditions of the ShimConfig.
    #[serde(default)]
    pub conditions: Vec<ShimConfigCondition>,
    /// Number of ready shim replicas.
    #[serde(default)]
    pub ready_replicas: Option<i32>,
    /// Latest observed generation.
    #[serde(default)]
    pub observed_generation: Option<i64>,
}

/// Condition for ShimConfig status.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ShimConfigCondition {
    /// Type of the condition (Ready, Reconciling, Degraded).
    pub condition_type: String,
    /// Status of the condition (True, False, Unknown).
    pub status: String,
    /// Time of the last transition.
    #[serde(default)]
    pub last_transition_time: Option<String>,
    /// Reason for the condition.
    #[serde(default)]
    pub reason: Option<String>,
    /// Human-readable message.
    #[serde(default)]
    pub message: Option<String>,
}

impl ShimConfigCondition {
    /// Create a new condition.
    pub fn new(
        condition_type: impl Into<String>,
        status: impl Into<String>,
        reason: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            condition_type: condition_type.into(),
            status: status.into(),
            last_transition_time: Some(chrono::Utc::now().to_rfc3339()),
            reason: Some(reason.into()),
            message: Some(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shim_config_spec_serialization() {
        let spec = ShimConfigSpec {
            child_command: "/app/start".into(),
            child_args: vec!["--verbose".into()],
            features: vec!["health".into(), "vault".into()],
            resources: Some(ResourceRequirements {
                requests: Some(ResourceSpec {
                    cpu: Some("100m".into()),
                    memory: Some("128Mi".into()),
                }),
                limits: Some(ResourceSpec {
                    cpu: Some("500m".into()),
                    memory: Some("256Mi".into()),
                }),
            }),
            shim_config: Some(ShimConfigOptions::default()),
        };

        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("childCommand"));
        assert!(json.contains("/app/start"));

        let deserialized: ShimConfigSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.child_command, "/app/start");
        assert_eq!(deserialized.features.len(), 2);
    }

    #[test]
    fn test_shim_config_options_defaults() {
        let opts = ShimConfigOptions::default();
        assert_eq!(opts.log_level, "info");
        assert!(!opts.json_logging);
        assert_eq!(opts.metrics_port, 9090);
        assert_eq!(opts.health_port, 9091);
        assert!(opts.otel_endpoint.is_none());
    }

    #[test]
    fn test_shim_config_condition_new() {
        let condition = ShimConfigCondition::new("Ready", "True", "Reconciled", "all good");
        assert_eq!(condition.condition_type, "Ready");
        assert_eq!(condition.status, "True");
        assert_eq!(condition.reason, Some("Reconciled".to_string()));
        assert_eq!(condition.message, Some("all good".to_string()));
        assert!(condition.last_transition_time.is_some());
    }

    #[test]
    fn test_shim_config_status_default() {
        let status = ShimConfigStatus::default();
        assert!(status.conditions.is_empty());
        assert!(status.ready_replicas.is_none());
        assert!(status.observed_generation.is_none());
    }

    #[test]
    fn test_resource_requirements_serialization() {
        let req = ResourceRequirements {
            requests: Some(ResourceSpec {
                cpu: Some("250m".into()),
                memory: Some("512Mi".into()),
            }),
            limits: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("250m"));
        let deser: ResourceRequirements = serde_json::from_str(&json).unwrap();
        assert!(deser.limits.is_none());
    }

    #[test]
    fn test_shim_config_wrapper_creation() {
        let spec = ShimConfigSpec {
            child_command: "/app/start".into(),
            child_args: vec![],
            features: vec![],
            resources: None,
            shim_config: None,
        };
        let shim_config = ShimConfig::new("my-shim", spec);
        assert_eq!(shim_config.metadata.name.as_deref(), Some("my-shim"));
        assert_eq!(shim_config.spec.child_command, "/app/start");
    }
}
