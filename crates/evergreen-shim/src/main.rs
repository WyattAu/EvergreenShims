//! EvergreenShim - Unified shim binary.
//!
//! This binary combines multiple shim capabilities into a single executable.

use anyhow::Result;
use clap::Parser;
use shim_core::{Capability, Config};
use std::path::PathBuf;

/// CLI arguments.
#[derive(Parser)]
#[command(name = "shim")]
#[command(about = "EvergreenShim - Self-managing container shim")]
#[command(version)]
struct Args {
    /// Path to configuration file.
    #[arg(short, long, default_value = "/etc/shim/config.toml")]
    config: PathBuf,

    /// Command to run as child process.
    #[arg(short, long)]
    command: Option<String>,

    /// Arguments for the child process.
    #[arg(short, long, num_args = 0)]
    args: Vec<String>,

    /// Enable debug logging.
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.debug { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    tracing::info!("EvergreenShim starting");

    // Load configuration
    let mut config = if args.config.exists() {
        Config::from_file(args.config.to_str().unwrap())?
    } else {
        tracing::info!("No config file found, using defaults");
        Config::default()
    };

    // Override with env vars
    let env_config = Config::from_env();
    config.merge(env_config);

    // Override with CLI args
    if let Some(cmd) = args.command {
        config.process.command = cmd;
    }
    if !args.args.is_empty() {
        config.process.args = args.args;
    }

    // Create capabilities
    let mut capabilities: Vec<Box<dyn Capability>> = Vec::new();

    #[cfg(feature = "health")]
    {
        tracing::info!("Enabling health shim");
        capabilities.push(Box::new(health_shim::HealthShim::new()));
    }

    #[cfg(feature = "vault")]
    {
        tracing::info!("Enabling vault shim");
        capabilities.push(Box::new(vault_shim::VaultShim::new()));
    }

    #[cfg(feature = "backup")]
    {
        tracing::info!("Enabling backup shim");
        capabilities.push(Box::new(backup_shim::BackupShim::new()));
    }

    #[cfg(feature = "migration")]
    {
        tracing::info!("Enabling migration shim");
        capabilities.push(Box::new(migration_shim::MigrationShim::new()));
    }

    #[cfg(feature = "audit")]
    {
        tracing::info!("Enabling audit shim");
        capabilities.push(Box::new(audit_shim::AuditShim::new()));
    }

    #[cfg(feature = "tls")]
    {
        tracing::info!("Enabling TLS shim");
        capabilities.push(Box::new(tls_shim::TlsShim::new()));
    }

    #[cfg(feature = "config")]
    {
        tracing::info!("Enabling config shim");
        capabilities.push(Box::new(config_shim::ConfigShim::new()));
    }

    #[cfg(feature = "failover")]
    {
        tracing::info!("Enabling failover shim");
        capabilities.push(Box::new(failover_shim::FailoverShim::new()));
    }

    #[cfg(feature = "replication")]
    {
        tracing::info!("Enabling replication shim");
        capabilities.push(Box::new(replication_shim::ReplicationShim::new()));
    }

    #[cfg(feature = "cache")]
    {
        tracing::info!("Enabling cache shim");
        capabilities.push(Box::new(cache_shim::CacheShim::new()));
    }

    #[cfg(feature = "cdc")]
    {
        tracing::info!("Enabling CDC shim");
        capabilities.push(Box::new(cdc_shim::CdcShim::new()));
    }

    #[cfg(feature = "sharding")]
    {
        tracing::info!("Enabling sharding shim");
        capabilities.push(Box::new(sharding_shim::ShardingShim::new()));
    }

    #[cfg(feature = "archival")]
    {
        tracing::info!("Enabling archival shim");
        capabilities.push(Box::new(archival_shim::ArchivalShim::new()));
    }

    #[cfg(feature = "auth")]
    {
        tracing::info!("Enabling auth shim");
        capabilities.push(Box::new(auth_shim::AuthShim::new()));
    }

    #[cfg(feature = "encryption")]
    {
        tracing::info!("Enabling encryption shim");
        capabilities.push(Box::new(encryption_shim::EncryptionShim::new()));
    }

    #[cfg(feature = "compliance")]
    {
        tracing::info!("Enabling compliance shim");
        capabilities.push(Box::new(compliance_shim::ComplianceShim::new()));
    }

    #[cfg(feature = "scheduler")]
    {
        tracing::info!("Enabling scheduler shim");
        capabilities.push(Box::new(scheduler_shim::SchedulerShim::new()));
    }

    #[cfg(feature = "queue")]
    {
        tracing::info!("Enabling queue shim");
        capabilities.push(Box::new(queue_shim::QueueShim::new()));
    }

    #[cfg(feature = "alerting")]
    {
        tracing::info!("Enabling alerting shim");
        capabilities.push(Box::new(alerting_shim::AlertingShim::new()));
    }

    #[cfg(feature = "chaos")]
    {
        tracing::info!("Enabling chaos shim");
        capabilities.push(Box::new(chaos_shim::ChaosShim::new()));
    }

    #[cfg(feature = "cost")]
    {
        tracing::info!("Enabling cost shim");
        capabilities.push(Box::new(cost_shim::CostShim::new()));
    }

    // Initialize capabilities
    for cap in &mut capabilities {
        cap.init(&config).await?;
    }

    // Start capabilities
    for cap in &mut capabilities {
        cap.start().await?;
    }

    tracing::info!("All capabilities started, waiting for shutdown signal");

    // Wait for shutdown signal
    let signal_handler = shim_core::SignalHandler::new();
    let mut shutdown_rx = signal_handler.subscribe();

    // Wait for SIGTERM/SIGINT
    loop {
        if signal_handler.is_shutdown() {
            break;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received Ctrl+C");
                break;
            }
            result = shutdown_rx.recv() => {
                if let Ok(signal) = result {
                    tracing::info!("Received signal: {:?}", signal);
                    break;
                }
            }
        }
    }

    // Stop capabilities in reverse order
    for cap in capabilities.iter_mut().rev() {
        cap.stop().await?;
    }

    tracing::info!("EvergreenShim stopped");
    Ok(())
}
