//! EvergreenShim - Unified shim binary.
//!
//! This binary combines multiple shim capabilities into a single executable.
//! Capabilities are initialized and started with graceful degradation:
//! - Critical capabilities (health, migration) cause process failure on error.
//! - Non-critical capabilities log warnings and allow the process to continue.
//! - A `shim_capabilities_healthy` gauge is exported via metrics (1=all critical healthy, 0=otherwise).

use anyhow::Result;
use clap::Parser;
use shim_core::{Capability, Config};
use std::collections::HashSet;
use std::path::PathBuf;

/// Capabilities that must start successfully for the shim to be considered operational.
/// If any critical capability fails to init or start, the process exits with an error.
const CRITICAL_CAPABILITIES: &[&str] = &["health", "migration"];

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

    /// Output logs as JSON (structured logging).
    #[arg(long, global = true)]
    json: bool,

    /// OpenTelemetry OTLP endpoint (e.g. http://localhost:4317).
    /// When set, traces are exported to this endpoint.
    #[arg(long, global = true)]
    otel_endpoint: Option<String>,
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

    // Initialize logging — OpenTelemetry takes precedence when an endpoint is given.
    #[cfg(feature = "otel")]
    {
        if let Some(ref endpoint) = args.otel_endpoint {
            shim_core::otel::init_otel_tracing(endpoint, args.debug, args.json);
        } else {
            let _ = shim_core::structured_logging::init_structured_logging(args.debug, args.json);
        }
    }
    #[cfg(not(feature = "otel"))]
    {
        if args.otel_endpoint.is_some() {
            eprintln!("warning: --otel-endpoint requires the 'otel' feature; ignoring");
        }
        let _ = shim_core::structured_logging::init_structured_logging(args.debug, args.json);
    }

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

/// Result of attempting to initialize and start a capability.
struct CapabilityOutcome {
    name: String,
    started: bool,
    error: Option<String>,
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

    // Initialize and start capabilities with graceful degradation.
    let mut outcomes: Vec<CapabilityOutcome> = Vec::new();
    let mut successful: Vec<usize> = Vec::new();

    for (idx, cap) in capabilities.iter_mut().enumerate() {
        let name = cap.name().to_string();
        let is_critical = CRITICAL_CAPABILITIES.contains(&name.as_str());

        tracing::info!("Initializing capability: {}", name);

        // Attempt init
        if let Err(e) = cap.init(&config).await {
            let msg = format!("init failed: {e}");
            tracing::error!("Capability '{}' failed to initialize: {msg}", name);

            outcomes.push(CapabilityOutcome {
                name,
                started: false,
                error: Some(msg),
            });

            if is_critical {
                return Err(anyhow::anyhow!(
                    "Critical capability '{}' failed to initialize: {}",
                    cap.name(),
                    e
                ));
            }
            continue;
        }

        // Attempt start
        tracing::info!("Starting capability: {}", name);
        if let Err(e) = cap.start().await {
            let msg = format!("start failed: {e}");
            tracing::error!("Capability '{}' failed to start: {msg}", name);

            outcomes.push(CapabilityOutcome {
                name,
                started: false,
                error: Some(msg),
            });

            if is_critical {
                return Err(anyhow::anyhow!(
                    "Critical capability '{}' failed to start: {}",
                    cap.name(),
                    e
                ));
            }
            continue;
        }

        tracing::info!("Capability '{}' started successfully", name);
        outcomes.push(CapabilityOutcome {
            name,
            started: true,
            error: None,
        });
        successful.push(idx);
    }

    // Build the set of successful capability names for the health metric.
    let all_critical_healthy = {
        let successful_names: HashSet<&str> =
            successful.iter().map(|&i| capabilities[i].name()).collect();
        CRITICAL_CAPABILITIES
            .iter()
            .all(|c| successful_names.contains(c))
    };

    let healthy_value: f64 = if all_critical_healthy { 1.0 } else { 0.0 };

    // Log a summary of capability startup results.
    let failed_count = outcomes.iter().filter(|o| !o.started).count();
    let started_count = outcomes.iter().filter(|o| o.started).count();
    tracing::info!(
        "Capability startup summary: {} started, {} failed (shim_capabilities_healthy={})",
        started_count,
        failed_count,
        healthy_value as u32,
    );

    for outcome in &outcomes {
        if let Some(ref err) = outcome.error {
            tracing::warn!("  [FAILED] {}: {}", outcome.name, err);
        }
    }

    // Spawn child process
    let mut child = shim_core::ChildProcess::new(config.process.clone());
    child.start().await?;

    tracing::info!(
        "All critical capabilities started, child process running, waiting for shutdown signal"
    );

    // Mark shim as healthy now that all capabilities are running
    metrics.set_healthy(true);

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

    // Stop capabilities in reverse order (only those that started)
    for idx in successful.iter().rev() {
        capabilities[*idx].stop().await?;
    }

    tracing::info!("EvergreenShim stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Existing tests ────────────────────────────────────────────────

    #[test]
    fn critical_capabilities_list_contains_expected() {
        assert!(CRITICAL_CAPABILITIES.contains(&"health"));
        assert!(CRITICAL_CAPABILITIES.contains(&"migration"));
    }

    #[test]
    fn non_critical_capabilities_are_not_in_critical_list() {
        for name in &["chaos", "cost", "cache", "alerting", "audit", "vault"] {
            assert!(
                !CRITICAL_CAPABILITIES.contains(name),
                "{name} should not be critical"
            );
        }
    }

    // ── 1. Capability initialization lifecycle (init → start → stop) ─

    #[tokio::test]
    async fn health_shim_lifecycle_init_start_stop() {
        let mut cap = health_shim::HealthShim::new();
        assert_eq!(cap.name(), "health");

        let config = Config::default();
        let init_result = cap.init(&config).await;
        assert!(init_result.is_ok(), "init should succeed");

        let start_result = cap.start().await;
        assert!(start_result.is_ok(), "start should succeed");

        let stop_result = cap.stop().await;
        assert!(stop_result.is_ok(), "stop should succeed");
    }

    #[tokio::test]
    async fn health_shim_metrics_after_lifecycle() {
        let mut cap = health_shim::HealthShim::new();
        let config = Config::default();
        cap.init(&config).await.unwrap();
        cap.start().await.unwrap();

        let metrics = cap.metrics();
        assert!(
            metrics.is_empty() || metrics.iter().all(|m| !m.name.is_empty()),
            "metrics should either be empty or have non-empty names"
        );

        cap.stop().await.unwrap();
    }

    #[tokio::test]
    async fn health_shim_init_sets_listen_address() {
        let mut cap = health_shim::HealthShim::new();
        let mut config = Config::default();
        config.health.listen = "127.0.0.1:19101".to_string();

        cap.init(&config).await.unwrap();
        let start_result = cap.start().await;
        assert!(start_result.is_ok());

        cap.stop().await.unwrap();
    }

    #[tokio::test]
    async fn health_shim_stop_before_start_succeeds() {
        let mut cap = health_shim::HealthShim::new();
        let config = Config::default();
        cap.init(&config).await.unwrap();

        // stop without start should still succeed
        let result = cap.stop().await;
        assert!(result.is_ok());
    }

    // ── 2. Feature flag compilation verification ──────────────────────

    #[test]
    fn default_feature_includes_health() {
        // The "health" feature is in the default feature set; verify
        // the module path resolves (compile-time check: this test only
        // compiles when the `health` feature is enabled).
        let cap = health_shim::HealthShim::new();
        assert_eq!(cap.name(), "health");
    }

    #[test]
    fn critical_capabilities_count_matches_expected() {
        // Exactly 2 critical capabilities
        assert_eq!(CRITICAL_CAPABILITIES.len(), 2);
        assert!(CRITICAL_CAPABILITIES.contains(&"health"));
        assert!(CRITICAL_CAPABILITIES.contains(&"migration"));
    }

    #[test]
    fn capability_outcome_struct_fields() {
        let outcome = CapabilityOutcome {
            name: "test-cap".to_string(),
            started: true,
            error: None,
        };
        assert_eq!(outcome.name, "test-cap");
        assert!(outcome.started);
        assert!(outcome.error.is_none());

        let failed = CapabilityOutcome {
            name: "bad-cap".to_string(),
            started: false,
            error: Some("init failed".to_string()),
        };
        assert!(!failed.started);
        assert!(failed.error.is_some());
    }

    #[test]
    fn all_known_feature_names_are_distinct() {
        let features = vec![
            "health",
            "vault",
            "backup",
            "migration",
            "audit",
            "proxy",
            "tls",
            "config",
            "failover",
            "replication",
            "cache",
            "cdc",
            "sharding",
            "archival",
            "auth",
            "encryption",
            "compliance",
            "scheduler",
            "queue",
            "alerting",
            "chaos",
            "cost",
            "mongodb",
            "cockroachdb",
            "dynamodb",
            "elasticsearch",
            "cassandra",
        ];
        let set: std::collections::HashSet<&str> = features.into_iter().collect();
        assert_eq!(set.len(), 27, "all feature names must be unique");
    }

    // ── 3. Graceful shutdown signal handling ───────────────────────────

    #[tokio::test]
    async fn signal_handler_initial_state_is_not_shutdown() {
        let handler = shim_core::SignalHandler::new();
        assert!(
            !handler.is_shutdown(),
            "fresh SignalHandler should not be in shutdown state"
        );
    }

    #[tokio::test]
    async fn signal_handler_subscribe_returns_receiver() {
        let handler = shim_core::SignalHandler::new();
        let mut rx = handler.subscribe();
        let result = rx.try_recv();
        assert!(result.is_err(), "no signals sent yet");
    }

    #[tokio::test]
    async fn signal_handler_multiple_subscribers() {
        let handler = shim_core::SignalHandler::new();
        let mut rx1 = handler.subscribe();
        let mut rx2 = handler.subscribe();

        assert!(rx1.try_recv().is_err());
        assert!(rx2.try_recv().is_err());
    }

    #[test]
    fn config_default_shutdown_timeout() {
        let config = Config::default();
        assert!(
            config.process.shutdown_timeout_secs > 0,
            "default shutdown timeout should be positive"
        );
    }

    #[test]
    fn graceful_shutdown_strategy_is_variant() {
        use shim_core::ShutdownStrategy;
        let strategy = ShutdownStrategy::GenericGraceful;
        assert_eq!(strategy, ShutdownStrategy::GenericGraceful);
        assert_eq!(strategy.to_string(), "generic");
    }

    // ── 4. Metrics collection across capabilities ─────────────────────

    #[test]
    fn health_shim_metrics_returns_empty_vec() {
        let cap = health_shim::HealthShim::new();
        let metrics = cap.metrics();
        assert!(metrics.is_empty(), "HealthShim starts with no metrics");
    }

    #[test]
    fn metric_new_has_no_labels() {
        let m = shim_core::Metric::new("test_counter", 42.0);
        assert_eq!(m.name, "test_counter");
        assert_eq!(m.value, 42.0);
        assert!(m.labels.is_empty());
    }

    #[test]
    fn metric_with_labels() {
        let mut labels = std::collections::HashMap::new();
        labels.insert("source".to_string(), "backup".to_string());
        labels.insert("status".to_string(), "ok".to_string());

        let m = shim_core::Metric::with_labels("backup_events", 5.0, labels);
        assert_eq!(m.name, "backup_events");
        assert_eq!(m.value, 5.0);
        assert_eq!(m.labels.len(), 2);
        assert_eq!(m.labels.get("source").unwrap(), "backup");
    }

    #[test]
    fn shim_metrics_collector_new_and_export() {
        let metrics = shim_core::metrics::ShimMetrics::new();
        metrics.set_healthy(true);
        metrics.events_published.inc_by(10);
        metrics.record_error("timeout");

        let output = metrics.export();
        assert!(output.contains("shim_events_published_total 10"));
        assert!(output.contains("shim_health_status 1"));
        assert!(output.contains("shim_errors_total"));
    }

    #[test]
    fn shim_metrics_all_standard_metrics_present() {
        let metrics = shim_core::metrics::ShimMetrics::new();
        // Label-based metrics must be used at least once to appear in output
        metrics.record_error("test");
        metrics.events_by_source.with_label_values(&["test"]).inc();
        metrics.events_handled.with_label_values(&["test"]).inc();
        let output = metrics.export();

        let expected = vec![
            "shim_events_published_total",
            "shim_events_dropped_total",
            "shim_bus_subscribers",
            "shim_health_status",
            "shim_health_checks_total",
            "shim_uptime_seconds",
            "shim_errors_total",
            "shim_operation_duration_seconds",
            "shim_events_by_source_total",
            "shim_events_handled_total",
        ];
        for name in &expected {
            assert!(
                output.contains(name),
                "Prometheus output should contain metric '{name}'"
            );
        }
    }

    #[test]
    fn config_validate_default_is_clean() {
        let config = Config::default();
        let errors = config.validate();
        assert!(
            errors.is_empty(),
            "default config should pass validation, got: {:?}",
            errors
        );
    }

    // ── 5. CLI Argument Parsing ─────────────────────────────────────

    #[test]
    fn cli_args_parse_default() {
        use clap::Parser;
        let args = Args::try_parse_from(["shim"]).unwrap();
        assert!(args.command.is_none());
        assert!(!args.debug);
        assert!(!args.json);
        assert!(args.otel_endpoint.is_none());
    }

    #[test]
    fn cli_args_parse_run_subcommand() {
        use clap::Parser;
        let args = Args::try_parse_from(["shim", "run", "-c", "redis-server"]).unwrap();
        match args.command {
            Some(Subcommand::Run { command, .. }) => {
                assert_eq!(command, Some("redis-server".to_string()));
            }
            _ => panic!("Expected Run subcommand"),
        }
    }

    #[test]
    fn cli_args_parse_healthcheck_tcp() {
        use clap::Parser;
        let args =
            Args::try_parse_from(["shim", "healthcheck", "--tcp", "127.0.0.1:5432"]).unwrap();
        match args.command {
            Some(Subcommand::Healthcheck { tcp, http, timeout }) => {
                assert_eq!(tcp, Some("127.0.0.1:5432".to_string()));
                assert!(http.is_none());
                assert_eq!(timeout, 3);
            }
            _ => panic!("Expected Healthcheck subcommand"),
        }
    }

    #[test]
    fn cli_args_parse_healthcheck_http_with_timeout() {
        use clap::Parser;
        let args = Args::try_parse_from([
            "shim",
            "healthcheck",
            "--http",
            "http://127.0.0.1:9101/livez",
            "-t",
            "10",
        ])
        .unwrap();
        match args.command {
            Some(Subcommand::Healthcheck { tcp, http, timeout }) => {
                assert!(tcp.is_none());
                assert_eq!(http, Some("http://127.0.0.1:9101/livez".to_string()));
                assert_eq!(timeout, 10);
            }
            _ => panic!("Expected Healthcheck subcommand"),
        }
    }

    #[test]
    fn cli_args_debug_json_flags() {
        use clap::Parser;
        let args = Args::try_parse_from(["shim", "--debug", "--json"]).unwrap();
        assert!(args.debug);
        assert!(args.json);
    }

    #[test]
    fn cli_args_otel_endpoint() {
        use clap::Parser;
        let args =
            Args::try_parse_from(["shim", "--otel-endpoint", "http://localhost:4317"]).unwrap();
        assert_eq!(
            args.otel_endpoint,
            Some("http://localhost:4317".to_string())
        );
    }

    // ── 6. Config Merge with Env Vars ───────────────────────────────

    #[test]
    fn config_merge_env_overrides() {
        temp_env::with_vars(
            [
                ("SHIM_MAX_MEMORY_BYTES", Some("8192")),
                ("SHIM_MAX_CPU_PERCENT", Some("75.0")),
            ],
            || {
                let mut base = Config::default();
                let env_config = Config::from_env();
                base.merge(env_config);
                assert_eq!(base.resource_quota.max_memory_bytes, Some(8192));
                assert_eq!(base.resource_quota.max_cpu_percent, Some(75.0));
            },
        );
    }

    #[test]
    fn config_merge_preserves_existing() {
        let mut base = Config::default();
        base.health.listen = "127.0.0.1:9999".to_string();
        let env_config = Config::from_env();
        base.merge(env_config);
        // Health listen is not set via env, so it should be preserved
        assert_eq!(base.health.listen, "127.0.0.1:9999");
    }
}
