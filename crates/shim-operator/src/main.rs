//! Shim Operator entry point.
//!
//! A Kubernetes operator that reconciles `ShimConfig` custom resources,
//! generating ConfigMaps and updating Deployments with shim sidecar containers.

mod crd;
mod error;
mod reconciler;

use std::sync::Arc;

use futures::StreamExt;
use kube::api::Api;
use kube::runtime::controller::Controller;
use kube::runtime::watcher;
use kube::Client;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crd::ShimConfig;

/// Application entry point.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,shim_operator=debug"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .init();

    info!("shim-operator starting up");

    // Load kube config
    let client = Client::try_default().await?;
    info!("kubernetes client initialized");

    // Set up the controller
    let shim_configs: Api<ShimConfig> = Api::default_namespaced(client.clone());

    info!("starting ShimConfig controller");

    let controller = Controller::new(shim_configs, watcher::Config::default()).run(
        reconciler::reconcile_shim_config,
        reconciler::error_backoff,
        Arc::new(client.clone()),
    );

    // Run the controller until shutdown signal
    tokio::select! {
        _ = controller.for_each(|res| async move {
            match res {
                Ok((_obj_ref, action)) => {
                    info!(
                        action = ?action,
                        "reconciliation complete"
                    );
                }
                Err(e) => {
                    tracing::error!(error = %e, "reconciliation failed");
                }
            }
        }) => {
            info!("controller finished");
        }
        _ = shutdown_signal() => {
            info!("shutdown signal received, stopping controller");
        }
    }

    info!("shim-operator stopped");
    Ok(())
}

/// Wait for a shutdown signal (SIGTERM or SIGINT).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_main_exists() {
        // Verify the binary compiles with main
    }
}
