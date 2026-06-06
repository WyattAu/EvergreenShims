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
#[command(
    after_help = "EXAMPLES:\n  shim -c postgres -- postgres -D /var/lib/postgresql/data\n  shim --command redis-server -- --bind 0.0.0.0\n  shim -f /etc/shim/config.toml\n  shim healthcheck --tcp 127.0.0.1:5432\n  shim healthcheck --http 127.0.0.1:9101/livez"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Subcommand>,

    /// Enable debug logging.
    #[arg(long, global = true)]
    debug: bool,
}

#[derive(clap::Subcommand)]
enum Subcommand {
    /// Run as PID 1 shim (manage child process + health probes)
    Run {
        /// Path to configuration file.
        #[arg(short = 'f', long, default_value = "/etc/shim/config.toml")]
        config: PathBuf,

        /// Command to run as child process.
        #[arg(short, long)]
        command: Option<String>,

        /// Arguments for the child process (after --).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// One-shot health check (exits 0=healthy, 1=unhealthy). For Docker HEALTHCHECK.
    Healthcheck {
        /// TCP host:port to check (e.g., 127.0.0.1:5432)
        #[arg(long)]
        tcp: Option<String>,

        /// HTTP URL to check (e.g., http://127.0.0.1:9101/livez)
        #[arg(long)]
        http: Option<String>,

        /// Timeout in seconds (default: 3)
        #[arg(short, long, default_value = "3")]
        timeout: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.debug { "debug" } else { "error" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    match args.command {
        Some(Subcommand::Run {
            config,
            command,
            args,
        }) => run_shim(config, command, args).await,
        Some(Subcommand::Healthcheck { tcp, http, timeout }) => {
            run_healthcheck(tcp, http, timeout).await
        }
        None => {
            // Default: run mode with no args (backward compat)
            run_shim(PathBuf::from("/etc/shim/config.toml"), None, vec![]).await
        }
    }
}

async fn run_healthcheck(
    tcp: Option<String>,
    http: Option<String>,
    timeout_secs: u64,
) -> Result<()> {
    use std::net::TcpStream;
    use std::time::Duration;

    let timeout = Duration::from_secs(timeout_secs);

    if let Some(addr) = tcp {
        match TcpStream::connect_timeout(&addr.parse()?, timeout) {
            Ok(_) => std::process::exit(0),
            Err(_) => std::process::exit(1),
        }
    }

    if let Some(url) = http {
        // Parse host:port from URL
        let url_parts: Vec<&str> = url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .split('/')
            .collect();
        if let Some(host_port) = url_parts.first() {
            match TcpStream::connect_timeout(&host_port.parse()?, timeout) {
                Ok(_) => std::process::exit(0),
                Err(_) => std::process::exit(1),
            }
        }
    }

    // Nothing to check
    eprintln!("healthcheck: specify --tcp or --http");
    std::process::exit(2)
}

async fn run_shim(config_path: PathBuf, command: Option<String>, args: Vec<String>) -> Result<()> {
    tracing::info!("EvergreenShim starting");

    // Load configuration
    let mut config = if config_path.exists() {
        Config::from_file(config_path.to_str().unwrap())?
    } else {
        tracing::info!("No config file found, using defaults");
        Config::default()
    };

    // Override with env vars
    let env_config = Config::from_env();
    config.merge(env_config);

    // Override with CLI args
    if let Some(cmd) = command {
        config.process.command = cmd;
    }
    if !args.is_empty() {
        config.process.args = args;
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

    #[cfg(feature = "mongodb")]
    {
        tracing::info!("Enabling MongoDB shim");
        capabilities.push(Box::new(mongodb_shim::MongoShim::new()));
    }

    #[cfg(feature = "cockroachdb")]
    {
        tracing::info!("Enabling CockroachDB shim");
        capabilities.push(Box::new(cockroachdb_shim::CrdbShim::new()));
    }

    #[cfg(feature = "dynamodb")]
    {
        tracing::info!("Enabling DynamoDB shim");
        capabilities.push(Box::new(dynamodb_shim::DynamoShim::new()));
    }

    #[cfg(feature = "elasticsearch")]
    {
        tracing::info!("Enabling Elasticsearch shim");
        capabilities.push(Box::new(elasticsearch_shim::ElasticsearchShim::new()));
    }

    #[cfg(feature = "cassandra")]
    {
        tracing::info!("Enabling Cassandra shim");
        capabilities.push(Box::new(cassandra_shim::CassandraShim::new()));
    }

    // Initialize capabilities
    for cap in &mut capabilities {
        cap.init(&config).await?;
    }

    // Start capabilities
    for cap in &mut capabilities {
        cap.start().await?;
    }

    // Spawn child process
    let mut child = shim_core::ChildProcess::new(config.process.clone());
    child.start().await?;

    tracing::info!("All capabilities started, child process running, waiting for shutdown signal");

    // Wait for shutdown signal or child exit
    let signal_handler = shim_core::SignalHandler::new();
    let mut shutdown_rx = signal_handler.subscribe();

    // Wait for SIGTERM/SIGINT or child exit
    loop {
        if signal_handler.is_shutdown() {
            break;
        }
        if !child.is_running() {
            tracing::info!("Child process exited, initiating shutdown");
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
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                // Periodically check child process
            }
        }
    }

    // Stop child process
    child.stop().await?;

    // Stop capabilities in reverse order
    for cap in capabilities.iter_mut().rev() {
        cap.stop().await?;
    }

    tracing::info!("EvergreenShim stopped");
    Ok(())
}
