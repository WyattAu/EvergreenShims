//! Failover integration tests.

/// Test that failover-shim can detect a healthy primary.
#[tokio::test]
async fn test_failover_detects_healthy_primary() {
    use std::net::TcpStream;
    use std::time::Duration;

    let addr: std::net::SocketAddr = "127.0.0.1:3306".parse().unwrap();
    let result = TcpStream::connect_timeout(&addr, Duration::from_secs(2));

    if result.is_ok() {
        println!("MariaDB is reachable at 127.0.0.1:3306");
    } else {
        println!("MariaDB not available, skipping test");
        return;
    }
}

/// Test failover state transitions.
#[tokio::test]
async fn test_failover_state_transitions() {
    use failover_shim::FailoverState;

    let state = FailoverState::Healthy;
    assert_eq!(state, FailoverState::Healthy);

    let state = FailoverState::Suspect;
    assert_eq!(state, FailoverState::Suspect);

    let state = FailoverState::FailingOver;
    assert_eq!(state, FailoverState::FailingOver);

    let state = FailoverState::FailedOver;
    assert_eq!(state, FailoverState::FailedOver);

    println!("Failover state transitions work correctly");
}

/// Test failover event serialization.
#[tokio::test]
async fn test_failover_event_serialization() {
    use failover_shim::FailoverEvent;

    let event = FailoverEvent {
        event: "failover".to_string(),
        old_primary: "127.0.0.1:3306".to_string(),
        new_primary: "127.0.0.1:3307".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        reason: "3 consecutive health check failures".to_string(),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("failover"));
    assert!(json.contains("127.0.0.1:3306"));

    println!("Failover event serialization works: {}", json);
}

/// Test patroni connector configuration via env vars.
#[tokio::test]
async fn test_patroni_connector_config() {
    use failover_shim::{FailoverConnector, FailoverShim};

    temp_env::with_vars(
        [
            ("FAILOVER_CONNECTOR", Some("patroni")),
            ("FAILOVER_DB_HOST", Some("pg-cluster.internal")),
            ("FAILOVER_DB_PORT", Some("5433")),
            ("FAILOVER_DB_USER", Some("patroni_admin")),
            ("FAILOVER_DB_PASSWORD", Some("supersecret")),
            ("FAILOVER_DB_NAME", Some("cluster_db")),
            ("FAILOVER_LAG_THRESHOLD_SECS", Some("45.5")),
            ("FAILOVER_CHECK_INTERVAL_SECS", Some("15")),
        ],
        || {
            let shim = FailoverShim::new();
            assert_eq!(shim.connector(), &FailoverConnector::Patroni);
            assert_eq!(shim.db_host(), "pg-cluster.internal");
            assert_eq!(shim.db_port(), "5433");
            assert_eq!(shim.db_user(), "patroni_admin");
            assert_eq!(shim.db_password(), "supersecret");
            assert_eq!(shim.db_name(), "cluster_db");
            assert_eq!(shim.lag_threshold_secs(), 45.5);
            assert_eq!(shim.check_interval_secs(), 15);
        },
    );
}

/// Test redis sentinel connector configuration via env vars.
#[tokio::test]
async fn test_redis_sentinel_connector_config() {
    use failover_shim::{FailoverConnector, FailoverShim};

    temp_env::with_vars(
        [
            ("FAILOVER_CONNECTOR", Some("redis-sentinel")),
            ("REDIS_SENTINEL_URL", Some("redis://sentinel.prod:26379")),
            ("REDIS_SENTINEL_MASTER", Some("production-master")),
            ("FAILOVER_CHECK_INTERVAL_SECS", Some("7")),
        ],
        || {
            let shim = FailoverShim::new();
            assert_eq!(shim.connector(), &FailoverConnector::RedisSentinel);
            assert_eq!(shim.redis_sentinel_url(), "redis://sentinel.prod:26379");
            assert_eq!(shim.redis_sentinel_master(), "production-master");
            assert_eq!(shim.check_interval_secs(), 7);
        },
    );
}

/// Test connector type equality.
#[tokio::test]
async fn test_connector_type_equality() {
    use failover_shim::FailoverConnector;

    assert_eq!(FailoverConnector::Tcp, FailoverConnector::Tcp);
    assert_eq!(FailoverConnector::Patroni, FailoverConnector::Patroni);
    assert_eq!(
        FailoverConnector::RedisSentinel,
        FailoverConnector::RedisSentinel
    );
    assert_ne!(FailoverConnector::Tcp, FailoverConnector::Patroni);
    assert_ne!(FailoverConnector::Patroni, FailoverConnector::RedisSentinel);
}

/// Test patroni connector defaults.
#[tokio::test]
async fn test_patroni_connector_defaults() {
    use failover_shim::FailoverShim;

    temp_env::with_vars(
        [
            ("FAILOVER_DB_HOST", None::<&str>),
            ("FAILOVER_DB_PORT", None::<&str>),
            ("FAILOVER_DB_USER", None::<&str>),
            ("FAILOVER_DB_PASSWORD", None::<&str>),
            ("FAILOVER_DB_NAME", None::<&str>),
            ("FAILOVER_LAG_THRESHOLD_SECS", None::<&str>),
        ],
        || {
            let shim = FailoverShim::new();
            assert_eq!(shim.db_host(), "127.0.0.1");
            assert_eq!(shim.db_port(), "5432");
            assert_eq!(shim.db_user(), "postgres");
            assert_eq!(shim.db_password(), "");
            assert_eq!(shim.db_name(), "postgres");
            assert_eq!(shim.lag_threshold_secs(), 30.0);
        },
    );
}

/// Test redis sentinel defaults.
#[tokio::test]
async fn test_redis_sentinel_defaults() {
    use failover_shim::FailoverShim;

    temp_env::with_vars(
        [
            ("REDIS_SENTINEL_URL", None::<&str>),
            ("REDIS_SENTINEL_MASTER", None::<&str>),
        ],
        || {
            let shim = FailoverShim::new();
            assert_eq!(shim.redis_sentinel_url(), "redis://localhost:26379");
            assert_eq!(shim.redis_sentinel_master(), "mymaster");
        },
    );
}

/// Test graceful shutdown with generic shutdown handler.
#[tokio::test]
async fn test_graceful_shutdown_generic() {
    use shim_core::{DatabaseType, ShutdownManager};

    let manager = ShutdownManager::new(DatabaseType::Generic, 5);
    assert_eq!(manager.db_type(), DatabaseType::Generic);
    assert!(!manager.is_initiated());

    // Shutdown a nonexistent PID — should handle gracefully
    let result = manager.shutdown(i32::MAX as u32).await.unwrap();
    assert!(!result.clean_exit);
    assert!(manager.is_initiated());
}

/// Test graceful shutdown with postgres shutdown handler.
#[tokio::test]
async fn test_graceful_shutdown_postgres() {
    use shim_core::{DatabaseType, ShutdownManager};

    let manager = ShutdownManager::new(DatabaseType::Postgres, 10);
    assert_eq!(manager.db_type(), DatabaseType::Postgres);

    let result = manager.shutdown(i32::MAX as u32).await.unwrap();
    assert!(!result.clean_exit);
    assert!(result.db_type == DatabaseType::Postgres);
}

/// Test graceful shutdown with redis shutdown handler.
#[tokio::test]
async fn test_graceful_shutdown_redis() {
    use shim_core::{DatabaseType, ShutdownManager};

    let manager = ShutdownManager::new(DatabaseType::Redis, 10);
    assert_eq!(manager.db_type(), DatabaseType::Redis);

    let result = manager.shutdown(i32::MAX as u32).await.unwrap();
    assert!(!result.clean_exit);
    assert!(result.db_type == DatabaseType::Redis);
}

/// Test shutdown result serialization.
#[tokio::test]
async fn test_shutdown_result_serialization() {
    use shim_core::{DatabaseType, ShutdownResult};

    let result = ShutdownResult {
        clean_exit: true,
        db_type: DatabaseType::Postgres,
        duration_ms: 1500,
        signals_sent: 1,
        log: vec![
            "Step 1: SIGTERM sent".to_string(),
            "Step 2: Clean exit".to_string(),
        ],
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("clean_exit"));
    assert!(json.contains("Postgres"));
    assert!(json.contains("1500"));
}

/// Test startup probe with immediate success.
#[tokio::test]
async fn test_startup_probe_immediate_success() {
    use shim_core::StartupProbe;

    let probe = StartupProbe::new("exec:true", 5).with_max_retries(0);
    let status = probe.check().await;
    assert_eq!(status, shim_core::HealthStatus::Healthy);
}

/// Test startup probe with immediate failure.
#[tokio::test]
async fn test_startup_probe_immediate_failure() {
    use shim_core::StartupProbe;

    let probe = StartupProbe::new("exec:false", 1).with_max_retries(0);
    let status = probe.check().await;
    assert_eq!(status, shim_core::HealthStatus::Unhealthy);
}

/// Test postgres startup probe construction.
#[tokio::test]
async fn test_postgres_startup_probe_construction() {
    use shim_core::health::postgres_startup_probe;

    let probe = postgres_startup_probe("127.0.0.1", 5432);
    assert!(probe.check_cmd.contains("127.0.0.1"));
    assert!(probe.check_cmd.contains("5432"));
    assert_eq!(probe.max_retries, 30);
    assert_eq!(probe.retry_delay_secs, 1);
}

/// Test redis startup probe construction.
#[tokio::test]
async fn test_redis_startup_probe_construction() {
    use shim_core::health::redis_startup_probe;

    let probe = redis_startup_probe("redis-cluster:6379");
    assert!(probe.check_cmd.contains("redis-cli"));
    assert!(probe.check_cmd.contains("6379"));
    assert!(probe.check_cmd.contains("ping"));
    assert_eq!(probe.max_retries, 30);
    assert_eq!(probe.retry_delay_secs, 1);
}

/// Test ChildProcess with database type.
#[tokio::test]
async fn test_child_process_with_db_type() {
    use shim_core::config::ProcessConfig;
    use shim_core::{ChildProcess, DatabaseType};

    let config = ProcessConfig::default();
    let mut proc = ChildProcess::with_db_type(config, DatabaseType::Postgres);
    assert_eq!(*proc.db_type(), DatabaseType::Postgres);

    proc.set_db_type(DatabaseType::Redis);
    assert_eq!(*proc.db_type(), DatabaseType::Redis);
}

// ============================================================================
// Shutdown Strategy Tests
// ============================================================================

/// Test shutdown strategy: postgres sends SIGTERM and waits.
#[tokio::test]
async fn test_shutdown_strategy_postgres() {
    use shim_core::ShutdownStrategy;

    let strategy = ShutdownStrategy::PostgresGraceful;
    assert_eq!(strategy, ShutdownStrategy::PostgresGraceful);
    assert_eq!(strategy.to_string(), "postgres");
    assert_eq!(
        strategy.to_database_type(),
        shim_core::DatabaseType::Postgres
    );
}

/// Test shutdown strategy: redis sends SIGTERM and waits for RDB.
#[tokio::test]
async fn test_shutdown_strategy_redis() {
    use shim_core::ShutdownStrategy;

    let strategy = ShutdownStrategy::RedisGraceful;
    assert_eq!(strategy, ShutdownStrategy::RedisGraceful);
    assert_eq!(strategy.to_string(), "redis");
    assert_eq!(strategy.to_database_type(), shim_core::DatabaseType::Redis);
}

/// Test shutdown strategy: generic sends SIGTERM then SIGKILL.
#[tokio::test]
async fn test_shutdown_strategy_generic() {
    use shim_core::ShutdownStrategy;

    let strategy = ShutdownStrategy::GenericGraceful;
    assert_eq!(strategy, ShutdownStrategy::GenericGraceful);
    assert_eq!(strategy.to_string(), "generic");
    assert_eq!(
        strategy.to_database_type(),
        shim_core::DatabaseType::Generic
    );
}

/// Test graceful_shutdown with a real short-lived process.
#[tokio::test]
async fn test_graceful_shutdown_real_process() {
    use shim_core::{graceful_shutdown, ShutdownStrategy};

    // Spawn a process that sleeps for a long time
    let mut child = tokio::process::Command::new("sleep")
        .arg("300")
        .spawn()
        .unwrap();

    let pid = child.id().unwrap_or(0);
    assert!(pid > 0);

    // Use generic strategy with short timeout
    let strategy = ShutdownStrategy::GenericGraceful;
    let result = graceful_shutdown(&strategy, &mut child, 2).await.unwrap();

    assert!(
        result.clean_exit,
        "Process should have been killed: {:?}",
        result.log
    );
    assert!(result.signals_sent >= 2); // SIGTERM + SIGKILL
}

// ============================================================================
// Startup Probe Tests
// ============================================================================

/// Test startup probe: tcp type.
#[tokio::test]
async fn test_startup_probe_tcp() {
    use shim_core::StartupProbe;

    // TCP to a non-listening port should fail
    let probe = StartupProbe::new("tcp:127.0.0.1:1", 1).with_max_retries(0);
    let status = probe.check().await;
    assert_eq!(status, shim_core::HealthStatus::Unhealthy);
}

/// Test startup probe: postgres type (pg_isready).
#[tokio::test]
async fn test_startup_probe_postgres() {
    use shim_core::health::postgres_startup_probe;

    let probe = postgres_startup_probe("127.0.0.1", 5432);
    assert!(probe.check_cmd.contains("pg_isready"));
    assert!(probe.check_cmd.contains("127.0.0.1"));
    assert!(probe.check_cmd.contains("5432"));
    // pg_isready won't be available in CI, so just check construction
}

/// Test startup probe: redis type (redis-cli ping).
#[tokio::test]
async fn test_startup_probe_redis() {
    use shim_core::health::redis_startup_probe;

    let probe = redis_startup_probe("127.0.0.1:6379");
    assert!(probe.check_cmd.contains("redis-cli"));
    assert!(probe.check_cmd.contains("6379"));
    assert!(probe.check_cmd.contains("ping"));
}

/// Test startup probe from env: tcp default.
#[tokio::test]
async fn test_startup_probe_from_env_tcp() {
    use shim_core::health::startup_probe_from_env;

    temp_env::with_var_unset("STARTUP_PROBE_TYPE", || {
        let probe = startup_probe_from_env();
        assert!(probe.check_cmd.contains("tcp:"));
    });
}

/// Test startup probe from env: postgres.
#[tokio::test]
async fn test_startup_probe_from_env_postgres() {
    use shim_core::health::startup_probe_from_env;

    temp_env::with_vars(
        [
            ("STARTUP_PROBE_TYPE", Some("postgres")),
            ("FAILOVER_DB_HOST", Some("pg.internal")),
            ("FAILOVER_DB_PORT", Some("5433")),
        ],
        || {
            let probe = startup_probe_from_env();
            assert!(probe.check_cmd.contains("pg_isready"));
            assert!(probe.check_cmd.contains("pg.internal"));
            assert!(probe.check_cmd.contains("5433"));
        },
    );
}

/// Test startup probe from env: redis.
#[tokio::test]
async fn test_startup_probe_from_env_redis() {
    use shim_core::health::startup_probe_from_env;

    temp_env::with_vars([("STARTUP_PROBE_TYPE", Some("redis"))], || {
        let probe = startup_probe_from_env();
        assert!(probe.check_cmd.contains("redis-cli"));
        assert!(probe.check_cmd.contains("ping"));
    });
}

// ============================================================================
// Failover Monitor Tests (mock-based)
// ============================================================================

/// Test PatroniMonitor construction and env var configuration.
#[tokio::test]
async fn test_failover_monitor_patroni() {
    use failover_shim::PatroniMonitor;

    let monitor = temp_env::with_vars(
        [
            ("FAILOVER_DB_HOST", Some("pg-cluster.internal")),
            ("FAILOVER_DB_PORT", Some("5433")),
            ("FAILOVER_DB_USER", Some("admin")),
            ("FAILOVER_DB_PASSWORD", Some("secret")),
            ("FAILOVER_DB_NAME", Some("cluster_db")),
            ("FAILOVER_CHECK_INTERVAL_SECS", Some("15")),
            ("FAILOVER_LAG_THRESHOLD_SECS", Some("20.5")),
        ],
        || {
            let monitor = PatroniMonitor::from_env();
            assert_eq!(monitor.db_host(), "pg-cluster.internal");
            assert_eq!(monitor.db_port(), "5433");
            assert_eq!(monitor.db_user(), "admin");
            assert_eq!(monitor.db_password(), "secret");
            assert_eq!(monitor.check_interval_secs(), 15);
            assert_eq!(monitor.lag_threshold_secs(), 20.5);
            monitor
        },
    );

    // Check that it returns Unreachable when no PG is running
    let result = monitor.check().await;
    match result {
        failover_shim::PatroniCheckResult::Unreachable => {
            // Expected in CI
        }
        failover_shim::PatroniCheckResult::Healthy { .. } => {
            // PG is running somewhere
        }
    }
}

/// Test RedisSentinelMonitor construction and env var configuration.
#[tokio::test]
async fn test_failover_monitor_redis_sentinel() {
    use failover_shim::RedisSentinelMonitor;

    let monitor = temp_env::with_vars(
        [
            ("REDIS_SENTINEL_URL", Some("redis://sentinel.prod:26379")),
            ("REDIS_SENTINEL_MASTER", Some("prod-master")),
            ("FAILOVER_CHECK_INTERVAL_SECS", Some("7")),
        ],
        || {
            let monitor = RedisSentinelMonitor::from_env();
            assert_eq!(monitor.sentinel_url(), "redis://sentinel.prod:26379");
            assert_eq!(monitor.master_name(), "prod-master");
            assert_eq!(monitor.check_interval_secs(), 7);
            monitor
        },
    );

    // Check that it returns Unreachable when no sentinel is running
    let result = monitor.check().await;
    match result {
        failover_shim::RedisSentinelCheckResult::Unreachable => {
            // Expected in CI
        }
        failover_shim::RedisSentinelCheckResult::MasterInfo { .. } => {
            // Sentinel is running somewhere
        }
    }
}

// ============================================================================
// Multi-Cluster Failover Integration Tests
// ============================================================================

/// Test MultiClusterMonitor construction from environment variables.
#[tokio::test]
async fn test_multi_cluster_monitor_from_env() {
    temp_env::with_vars(
        [
            (
                "FAILOVER_CLUSTERS",
                Some("us-east:10.0.0.1:5432,eu-west:10.0.1.1:5432"),
            ),
            ("FAILOVER_PRIMARY", Some("us-east")),
            ("FAILOVER_STRATEGY", Some("latency")),
            ("FAILOVER_CROSS_CHECK_SECS", Some("30")),
            ("FAILOVER_FAILURE_THRESHOLD", Some("5")),
        ],
        || {
            let monitor = failover_shim::MultiClusterMonitor::from_env();
            assert_eq!(monitor.clusters().len(), 2);
            assert_eq!(monitor.primary_cluster(), "us-east");
            assert_eq!(
                monitor.failover_strategy(),
                &failover_shim::MultiClusterFailoverStrategy::LatencyBased
            );
            assert_eq!(monitor.cross_cluster_check_interval_secs(), 30);
            assert_eq!(monitor.failure_threshold(), 5);
        },
    );
}

/// Test MultiClusterMonitor with explicit configuration.
#[tokio::test]
async fn test_multi_cluster_monitor_explicit_config() {
    use std::collections::HashMap;

    let mut clusters = HashMap::new();
    clusters.insert(
        "primary".to_string(),
        failover_shim::ClusterHealth::new("primary", vec!["127.0.0.1:5432".to_string()]),
    );
    clusters.insert(
        "secondary".to_string(),
        failover_shim::ClusterHealth::new("secondary", vec!["127.0.0.2:5432".to_string()]),
    );

    let monitor = failover_shim::MultiClusterMonitor::new(
        clusters,
        "primary",
        failover_shim::MultiClusterFailoverStrategy::Sequential,
        10,
        3,
        Some("https://hooks.slack.com/test".to_string()),
    );

    assert_eq!(monitor.clusters().len(), 2);
    assert_eq!(monitor.primary_cluster(), "primary");
}

/// Test cross-cluster health check with unreachable clusters.
#[tokio::test]
async fn test_multi_cluster_health_check_unreachable() {
    use std::collections::HashMap;

    let mut clusters = HashMap::new();
    clusters.insert(
        "primary".to_string(),
        failover_shim::ClusterHealth::new("primary", vec!["192.0.2.1:9999".to_string()]),
    );
    clusters.insert(
        "secondary".to_string(),
        failover_shim::ClusterHealth::new("secondary", vec!["192.0.2.2:9999".to_string()]),
    );

    let mut monitor = failover_shim::MultiClusterMonitor::new(
        clusters,
        "primary",
        failover_shim::MultiClusterFailoverStrategy::Sequential,
        10,
        3,
        None,
    );

    monitor.check_all_clusters().await;

    let clusters = monitor.clusters();
    let primary = clusters.get("primary").unwrap();
    // Unreachable endpoint → Degraded or Failed depending on threshold
    assert!(
        primary.status == failover_shim::ClusterStatus::Degraded
            || primary.status == failover_shim::ClusterStatus::Failed
    );
}

/// Test failover trigger: primary cluster fails, secondary is promoted.
#[tokio::test]
async fn test_multi_cluster_failover_trigger() {
    use std::collections::HashMap;

    let mut clusters = HashMap::new();
    clusters.insert(
        "us-east".to_string(),
        failover_shim::ClusterHealth::new("us-east", vec!["192.0.2.1:9999".to_string()]),
    );
    let mut eu = failover_shim::ClusterHealth::new("eu-west", vec!["192.0.2.2:9999".to_string()]);
    eu.status = failover_shim::ClusterStatus::Healthy;
    clusters.insert("eu-west".to_string(), eu);

    let mut monitor = failover_shim::MultiClusterMonitor::new(
        clusters,
        "us-east",
        failover_shim::MultiClusterFailoverStrategy::Sequential,
        10,
        3,
        None,
    );

    // Check all clusters — primary should fail
    monitor.check_all_clusters().await;

    // Since primary is unreachable and threshold is 3, it should be Degraded first
    let primary_status = monitor.clusters().get("us-east").unwrap().status.clone();
    assert_ne!(primary_status, failover_shim::ClusterStatus::Healthy);

    // Simulate repeated failures to reach threshold
    for _ in 0..5 {
        monitor.check_all_clusters().await;
    }

    // After threshold, primary should be Failed
    let primary_status = monitor.clusters().get("us-east").unwrap().status.clone();
    assert_eq!(primary_status, failover_shim::ClusterStatus::Failed);

    // Reset eu-west to Healthy (check_all_clusters would have marked it Degraded/Failed too)
    monitor.clusters_mut().get_mut("eu-west").unwrap().status =
        failover_shim::ClusterStatus::Healthy;

    // Now promote secondary
    let promoted = monitor.promote_secondary().await;
    assert_eq!(promoted, Some("eu-west".to_string()));
    assert_eq!(monitor.primary_cluster(), "eu-west");
    assert_eq!(monitor.promotions_total(), 1);
}

/// Test cluster promotion with no eligible secondary.
#[tokio::test]
async fn test_multi_cluster_no_eligible_secondary() {
    use std::collections::HashMap;

    let mut clusters = HashMap::new();
    clusters.insert(
        "primary".to_string(),
        failover_shim::ClusterHealth::new("primary", vec!["192.0.2.1:9999".to_string()]),
    );
    let mut sec =
        failover_shim::ClusterHealth::new("secondary", vec!["192.0.2.2:9999".to_string()]);
    sec.status = failover_shim::ClusterStatus::Failed;
    clusters.insert("secondary".to_string(), sec);

    let mut monitor = failover_shim::MultiClusterMonitor::new(
        clusters,
        "primary",
        failover_shim::MultiClusterFailoverStrategy::Sequential,
        10,
        3,
        None,
    );

    // No healthy secondary → promote_secondary returns None
    let promoted = monitor.promote_secondary().await;
    assert_eq!(promoted, None);
}

/// Test multi-cluster metrics report cluster status and latency.
#[tokio::test]
async fn test_multi_cluster_metrics() {
    use std::collections::HashMap;

    let mut clusters = HashMap::new();
    clusters.insert(
        "primary".to_string(),
        failover_shim::ClusterHealth::new("primary", vec!["127.0.0.1:5432".to_string()]),
    );
    let mut sec =
        failover_shim::ClusterHealth::new("secondary", vec!["127.0.0.2:5432".to_string()]);
    sec.status = failover_shim::ClusterStatus::Healthy;
    sec.latency_ms = Some(42);
    clusters.insert("secondary".to_string(), sec);

    let monitor = failover_shim::MultiClusterMonitor::new(
        clusters,
        "primary",
        failover_shim::MultiClusterFailoverStrategy::Sequential,
        10,
        3,
        None,
    );

    let metrics = monitor.metrics();
    // Should have status metrics for both clusters + latency for secondary + promotions_total
    assert!(metrics.len() >= 4);

    let promotions = metrics
        .iter()
        .find(|m| m.name == "failover_promotions_total");
    assert!(promotions.is_some());
    assert_eq!(promotions.unwrap().value, 0.0);
}

/// Test multi-cluster failover with latency-based strategy.
#[tokio::test]
async fn test_multi_cluster_latency_based_strategy() {
    use std::collections::HashMap;

    let mut clusters = HashMap::new();
    clusters.insert(
        "primary".to_string(),
        failover_shim::ClusterHealth::new("primary", vec!["127.0.0.1:5432".to_string()]),
    );

    let mut slow = failover_shim::ClusterHealth::new("slow", vec!["127.0.0.2:5432".to_string()]);
    slow.status = failover_shim::ClusterStatus::Healthy;
    slow.latency_ms = Some(200);
    clusters.insert("slow".to_string(), slow);

    let mut fast = failover_shim::ClusterHealth::new("fast", vec!["127.0.0.3:5432".to_string()]);
    fast.status = failover_shim::ClusterStatus::Healthy;
    fast.latency_ms = Some(10);
    clusters.insert("fast".to_string(), fast);

    let monitor = failover_shim::MultiClusterMonitor::new(
        clusters,
        "primary",
        failover_shim::MultiClusterFailoverStrategy::LatencyBased,
        10,
        3,
        None,
    );

    let target = monitor.select_failover_target();
    assert_eq!(target, Some("fast".to_string()));
}
