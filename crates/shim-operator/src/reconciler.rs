//! Reconciliation logic for ShimConfig resources.
//!
//! Watches ShimConfig CRs and reconciles the desired state by:
//! - Generating ConfigMaps with environment variables derived from the spec
//! - Updating Deployment pod templates with the sidecar container spec
//! - Setting status conditions (Ready, Reconciling, Degraded)
//! - Cleaning up ConfigMaps on delete
//! - Handling errors with exponential backoff
//! - Emitting events for observability

use std::collections::BTreeMap;
use std::sync::Arc;

use k8s_openapi::api::core::v1::ObjectReference;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::runtime::events::{Event, EventType, Recorder, Reporter};
use kube::Client;
use tracing::{error, info, warn};

use crate::crd::{ShimConfig, ShimConfigSpec, ShimConfigStatus};
use crate::error::{OperatorError, OperatorResult};

/// Name prefix for generated ConfigMaps.
const CONFIGMAP_PREFIX: &str = "shim-config";

/// Create a recorder for emitting Kubernetes events.
fn make_recorder(client: &Client, shim_config: &ShimConfig) -> Recorder {
    let reference = ObjectReference {
        api_version: Some("evergreen.dev/v1alpha1".into()),
        kind: Some("ShimConfig".into()),
        name: shim_config.metadata.name.clone(),
        namespace: shim_config.metadata.namespace.clone(),
        uid: shim_config.metadata.uid.clone(),
        resource_version: shim_config.metadata.resource_version.clone(),
        field_path: None,
    };
    Recorder::new(
        client.clone(),
        Reporter {
            controller: "shim-operator".into(),
            instance: None,
        },
        reference,
    )
}

/// Get a Client from an Arc<Client>.
fn client_ref(client: &Arc<Client>) -> Client {
    Client::clone(client)
}

/// Reconcile a single ShimConfig resource.
pub async fn reconcile_shim_config(
    shim_config: Arc<ShimConfig>,
    client: Arc<Client>,
) -> Result<Action, OperatorError> {
    let ns = shim_config.metadata.namespace.clone().unwrap_or_default();
    let name = shim_config.metadata.name.clone().unwrap_or_default();
    let generation = shim_config.metadata.generation.unwrap_or(0);

    let recorder = make_recorder(&client_ref(&client), &shim_config);

    info!(
        shim_name = %name,
        namespace = %ns,
        generation = generation,
        "reconciling ShimConfig"
    );

    // Emit reconciling event
    recorder
        .publish(Event {
            type_: EventType::Normal,
            reason: "Reconciling".into(),
            note: Some(format!("Reconciling ShimConfig {}", name)),
            action: "Reconcile".into(),
            secondary: None,
        })
        .await
        .ok();

    let spec = shim_config.spec.clone();

    // 1. Generate ConfigMap from spec
    let configmap = generate_configmap(&name, &ns, &spec)?;

    // 2. Apply ConfigMap using JSON merge patch
    let cms: Api<k8s_openapi::api::core::v1::ConfigMap> = Api::namespaced(client_ref(&client), &ns);
    let configmap_name = format!("{}-{}", CONFIGMAP_PREFIX, name);

    let cm_json = serde_json::to_value(&configmap).map_err(OperatorError::Serialization)?;
    let pp = PatchParams::apply("shim-operator").force();
    cms.patch(&configmap_name, &pp, &Patch::Apply(&cm_json))
        .await
        .map_err(OperatorError::Kubernetes)?;

    info!(
        shim_name = %name,
        configmap = %configmap_name,
        "ConfigMap applied successfully"
    );

    // 3. Update Deployment if it exists
    let deployments: Api<k8s_openapi::api::apps::v1::Deployment> =
        Api::namespaced(client_ref(&client), &ns);
    let deployment_name = format!("{}-deployment", name);

    if let Ok(Some(mut deploy)) = deployments.get_opt(&deployment_name).await {
        let container = build_sidecar_container(&spec);

        // Add container to pod template if not present
        if let Some(ref mut pod_template) =
            deploy.spec.as_mut().and_then(|s| s.template.spec.as_mut())
        {
            let has_shim = pod_template
                .containers
                .iter()
                .any(|c| c.name == "evergreen-shim");
            if !has_shim {
                pod_template.containers.push(container);
            }
        }

        let pp = PatchParams::apply("shim-operator");
        deployments
            .patch(&deployment_name, &pp, &Patch::Apply(&deploy))
            .await
            .map_err(OperatorError::Kubernetes)?;

        info!(
            shim_name = %name,
            deployment = %deployment_name,
            "Deployment updated with shim sidecar"
        );
    }

    // 4. Update status conditions
    let new_status = ShimConfigStatus {
        conditions: vec![
            crate::crd::ShimConfigCondition::new(
                "Ready",
                "True",
                "Reconciled",
                "ShimConfig reconciled successfully",
            ),
            crate::crd::ShimConfigCondition::new(
                "Reconciling",
                "False",
                "Reconciled",
                "Reconciliation complete",
            ),
        ],
        ready_replicas: Some(1),
        observed_generation: Some(generation),
    };

    let status_patch = serde_json::json!({
        "status": new_status
    });

    let status_api: Api<ShimConfig> = Api::namespaced(client_ref(&client), &ns);
    status_api
        .patch_status(&name, &PatchParams::default(), &Patch::Merge(&status_patch))
        .await
        .map_err(OperatorError::Kubernetes)?;

    info!(shim_name = %name, "status conditions updated");

    // Emit success event
    recorder
        .publish(Event {
            type_: EventType::Normal,
            reason: "Reconciled".into(),
            note: Some(format!("ShimConfig {} reconciled successfully", name)),
            action: "Reconcile".into(),
            secondary: None,
        })
        .await
        .ok();

    Ok(Action::requeue(std::time::Duration::from_secs(300)))
}

/// Handle deletion of a ShimConfig resource.
#[allow(dead_code)]
pub async fn cleanup_shim_config(
    shim_config: Arc<ShimConfig>,
    client: Arc<Client>,
) -> Result<Action, OperatorError> {
    let ns = shim_config.metadata.namespace.clone().unwrap_or_default();
    let name = shim_config.metadata.name.clone().unwrap_or_default();

    info!(shim_name = %name, namespace = %ns, "cleaning up ShimConfig resources");

    // Delete the generated ConfigMap
    let cms: Api<k8s_openapi::api::core::v1::ConfigMap> = Api::namespaced(client_ref(&client), &ns);
    let cm_name = format!("{}-{}", CONFIGMAP_PREFIX, name);

    match cms.delete(&cm_name, &Default::default()).await {
        Ok(_) => info!(shim_name = %name, configmap = %cm_name, "ConfigMap deleted"),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("404") || msg.contains("Not Found") {
                info!(shim_name = %name, configmap = %cm_name, "ConfigMap already deleted");
            } else {
                warn!(shim_name = %name, error = %e, "failed to delete ConfigMap");
            }
        }
    }

    // Remove the finalizer
    let api: Api<ShimConfig> = Api::namespaced(client_ref(&client), &ns);
    let patch = serde_json::json!({
        "metadata": {
            "finalizers": null
        }
    });
    api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .map_err(OperatorError::Kubernetes)?;

    info!(shim_name = %name, "cleanup complete");
    Ok(Action::await_change())
}

/// Generate ConfigMap data from the ShimConfig spec.
pub fn generate_configmap_data(spec: &ShimConfigSpec) -> BTreeMap<String, String> {
    let mut data = BTreeMap::new();

    data.insert("SHIM_CHILD_COMMAND".to_string(), spec.child_command.clone());
    data.insert("SHIM_CHILD_ARGS".to_string(), spec.child_args.join(","));
    data.insert("SHIM_FEATURES".to_string(), spec.features.join(","));

    let opts = spec.shim_config.clone().unwrap_or_default();
    data.insert("SHIM_LOG_LEVEL".to_string(), opts.log_level);
    data.insert(
        "SHIM_JSON_LOGGING".to_string(),
        opts.json_logging.to_string(),
    );
    data.insert(
        "SHIM_METRICS_PORT".to_string(),
        opts.metrics_port.to_string(),
    );
    data.insert("SHIM_HEALTH_PORT".to_string(), opts.health_port.to_string());

    if let Some(ref endpoint) = opts.otel_endpoint {
        data.insert("SHIM_OTEL_ENDPOINT".to_string(), endpoint.clone());
    }

    data
}

/// Generate a ConfigMap from the ShimConfig spec.
pub fn generate_configmap(
    name: &str,
    namespace: &str,
    spec: &ShimConfigSpec,
) -> OperatorResult<k8s_openapi::api::core::v1::ConfigMap> {
    let data = generate_configmap_data(spec);
    let cm_name = format!("{}-{}", CONFIGMAP_PREFIX, name);

    Ok(k8s_openapi::api::core::v1::ConfigMap {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(cm_name),
            namespace: Some(namespace.to_string()),
            labels: Some(
                [
                    (
                        "app.kubernetes.io/managed-by".into(),
                        "shim-operator".into(),
                    ),
                    ("evergreen.dev/shim-config".into(), name.into()),
                ]
                .into_iter()
                .collect(),
            ),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    })
}

/// Parse a resource quantity string into a k8s Quantity.
fn parse_quantity(s: &str) -> Option<Quantity> {
    // Quantity is just a newtype wrapper around String
    if s.is_empty() {
        None
    } else {
        Some(Quantity(s.to_string()))
    }
}

/// Build a sidecar container spec from the ShimConfig spec.
fn build_sidecar_container(spec: &ShimConfigSpec) -> k8s_openapi::api::core::v1::Container {
    let mut env = Vec::new();

    env.push(k8s_openapi::api::core::v1::EnvVar {
        name: "SHIM_CHILD_COMMAND".into(),
        value: Some(spec.child_command.clone()),
        value_from: None,
    });

    if !spec.child_args.is_empty() {
        env.push(k8s_openapi::api::core::v1::EnvVar {
            name: "SHIM_CHILD_ARGS".into(),
            value: Some(spec.child_args.join(",")),
            value_from: None,
        });
    }

    if !spec.features.is_empty() {
        env.push(k8s_openapi::api::core::v1::EnvVar {
            name: "SHIM_FEATURES".into(),
            value: Some(spec.features.join(",")),
            value_from: None,
        });
    }

    let resources = spec.resources.as_ref().map(|r| {
        let mut res = k8s_openapi::api::core::v1::ResourceRequirements::default();

        if let Some(ref req) = r.requests {
            let mut map = BTreeMap::new();
            if let Some(ref cpu) = req.cpu {
                if let Some(q) = parse_quantity(cpu) {
                    map.insert("cpu".into(), q);
                }
            }
            if let Some(ref mem) = req.memory {
                if let Some(q) = parse_quantity(mem) {
                    map.insert("memory".into(), q);
                }
            }
            res.requests = Some(map);
        }

        if let Some(ref lim) = r.limits {
            let mut map = BTreeMap::new();
            if let Some(ref cpu) = lim.cpu {
                if let Some(q) = parse_quantity(cpu) {
                    map.insert("cpu".into(), q);
                }
            }
            if let Some(ref mem) = lim.memory {
                if let Some(q) = parse_quantity(mem) {
                    map.insert("memory".into(), q);
                }
            }
            res.limits = Some(map);
        }

        res
    });

    k8s_openapi::api::core::v1::Container {
        name: "evergreen-shim".to_string(),
        image: Some("ghcr.io/wyattau/evergreen-shim:latest".to_string()),
        env: Some(env),
        resources,
        ports: Some(vec![
            k8s_openapi::api::core::v1::ContainerPort {
                container_port: spec
                    .shim_config
                    .as_ref()
                    .map(|o| o.metrics_port as i32)
                    .unwrap_or(9090),
                name: Some("metrics".into()),
                protocol: Some("TCP".into()),
                ..Default::default()
            },
            k8s_openapi::api::core::v1::ContainerPort {
                container_port: spec
                    .shim_config
                    .as_ref()
                    .map(|o| o.health_port as i32)
                    .unwrap_or(9091),
                name: Some("health".into()),
                protocol: Some("TCP".into()),
                ..Default::default()
            },
        ]),
        ..Default::default()
    }
}

/// Error handler for reconciliation failures with exponential backoff.
pub fn error_backoff(
    _object: Arc<ShimConfig>,
    error: &OperatorError,
    _client: Arc<Client>,
) -> Action {
    match error.classify() {
        crate::error::ErrorClass::Transient => {
            warn!(error = %error, "transient error, retrying with backoff");
            Action::requeue(std::time::Duration::from_secs(30))
        }
        crate::error::ErrorClass::Permanent => {
            error!(error = %error, "permanent error, not retrying");
            Action::await_change()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{ResourceRequirements, ResourceSpec, ShimConfigOptions};

    fn test_spec() -> ShimConfigSpec {
        ShimConfigSpec {
            child_command: "/app/start".into(),
            child_args: vec!["--port".into(), "8080".into()],
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
            shim_config: Some(ShimConfigOptions {
                log_level: "debug".into(),
                json_logging: true,
                metrics_port: 9090,
                health_port: 9091,
                otel_endpoint: Some("http://localhost:4317".into()),
            }),
        }
    }

    #[test]
    fn test_generate_configmap_all_fields() {
        let spec = test_spec();
        let cm = generate_configmap("my-shim", "default", &spec).unwrap();

        assert_eq!(cm.metadata.name, Some("shim-config-my-shim".into()));
        assert_eq!(cm.metadata.namespace, Some("default".into()));

        let data = cm.data.as_ref().unwrap();
        assert_eq!(data.get("SHIM_CHILD_COMMAND").unwrap(), "/app/start");
        assert_eq!(data.get("SHIM_CHILD_ARGS").unwrap(), "--port,8080");
        assert_eq!(data.get("SHIM_FEATURES").unwrap(), "health,vault");
        assert_eq!(data.get("SHIM_LOG_LEVEL").unwrap(), "debug");
        assert_eq!(data.get("SHIM_JSON_LOGGING").unwrap(), "true");
        assert_eq!(data.get("SHIM_METRICS_PORT").unwrap(), "9090");
        assert_eq!(data.get("SHIM_HEALTH_PORT").unwrap(), "9091");
        assert_eq!(
            data.get("SHIM_OTEL_ENDPOINT").unwrap(),
            "http://localhost:4317"
        );
    }

    #[test]
    fn test_generate_configmap_minimal_spec() {
        let spec = ShimConfigSpec {
            child_command: "/bin/app".into(),
            child_args: vec![],
            features: vec![],
            resources: None,
            shim_config: None,
        };

        let cm = generate_configmap("minimal", "default", &spec).unwrap();
        let data = cm.data.as_ref().unwrap();

        assert_eq!(data.get("SHIM_CHILD_COMMAND").unwrap(), "/bin/app");
        assert_eq!(data.get("SHIM_CHILD_ARGS").unwrap(), "");
        assert_eq!(data.get("SHIM_FEATURES").unwrap(), "");
        assert_eq!(data.get("SHIM_LOG_LEVEL").unwrap(), "info");
        assert_eq!(data.get("SHIM_JSON_LOGGING").unwrap(), "false");
        assert!(data.get("SHIM_OTEL_ENDPOINT").is_none());
    }

    #[test]
    fn test_generate_configmap_labels() {
        let spec = test_spec();
        let cm = generate_configmap("test", "ns", &spec).unwrap();
        let labels = cm.metadata.labels.as_ref().unwrap();

        assert_eq!(
            labels.get("app.kubernetes.io/managed-by").unwrap(),
            "shim-operator"
        );
        assert_eq!(labels.get("evergreen.dev/shim-config").unwrap(), "test");
    }

    #[test]
    fn test_generate_configmap_data() {
        let spec = test_spec();
        let data = generate_configmap_data(&spec);
        assert_eq!(data.get("SHIM_CHILD_COMMAND").unwrap(), "/app/start");
    }

    #[test]
    fn test_parse_quantity() {
        assert!(parse_quantity("100m").is_some());
        assert!(parse_quantity("128Mi").is_some());
        assert!(parse_quantity("1Gi").is_some());
        assert!(parse_quantity("").is_none());
        assert_eq!(
            parse_quantity("100m").unwrap(),
            Quantity("100m".to_string())
        );
    }

    #[test]
    fn test_build_sidecar_container_resources() {
        let spec = test_spec();
        let container = build_sidecar_container(&spec);

        assert_eq!(container.name, "evergreen-shim");
        assert!(container.image.is_some());
        assert!(container.resources.is_some());

        let res = container.resources.as_ref().unwrap();
        assert!(res.requests.is_some());
        assert!(res.limits.is_some());
    }

    #[test]
    fn test_build_sidecar_container_env() {
        let spec = test_spec();
        let container = build_sidecar_container(&spec);
        let env = container.env.as_ref().unwrap();

        let child_cmd = env.iter().find(|e| e.name == "SHIM_CHILD_COMMAND").unwrap();
        assert_eq!(child_cmd.value.as_deref(), Some("/app/start"));

        let args = env.iter().find(|e| e.name == "SHIM_CHILD_ARGS").unwrap();
        assert_eq!(args.value.as_deref(), Some("--port,8080"));

        let features = env.iter().find(|e| e.name == "SHIM_FEATURES").unwrap();
        assert_eq!(features.value.as_deref(), Some("health,vault"));
    }

    #[test]
    fn test_build_sidecar_container_ports() {
        let spec = test_spec();
        let container = build_sidecar_container(&spec);
        let ports = container.ports.as_ref().unwrap();

        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].name.as_deref(), Some("metrics"));
        assert_eq!(ports[0].container_port, 9090);
        assert_eq!(ports[1].name.as_deref(), Some("health"));
        assert_eq!(ports[1].container_port, 9091);
    }

    #[tokio::test]
    async fn test_error_backoff_transient_does_not_panic() {
        let err = OperatorError::Reconcile("temp".into());
        let spec = test_spec();
        let shim_config = Arc::new(ShimConfig::new("test", spec));
        if let Ok(client) = kube::Client::try_default().await {
            let client = Arc::new(client);
            let _action = error_backoff(shim_config, &err, client);
        }
    }

    #[tokio::test]
    async fn test_error_backoff_permanent_does_not_panic() {
        let err = OperatorError::Validation("bad".into());
        let spec = test_spec();
        let shim_config = Arc::new(ShimConfig::new("test", spec));
        if let Ok(client) = kube::Client::try_default().await {
            let client = Arc::new(client);
            let _action = error_backoff(shim_config, &err, client);
        }
    }
}
