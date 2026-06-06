//! Cassandra shim — health checks and cluster monitoring.
//!
//! Uses nodetool for health checks and cluster topology queries.
//!
//! ## Environment Variables
//!
//! ```text
//! CASSANDRA_HOST        Cassandra host (default: localhost)
//! CASSANDRA_PORT        CQL port (default: 9042)
//! CASSANDRA_JMX_PORT    JMX port for nodetool (default: 7199)
//! CASSANDRA_CLUSTER     Cluster name (default: local)
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Cassandra cluster node info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CassandraNode {
    pub address: String,
    pub datacenter: String,
    pub rack: String,
    pub status: String,
    pub state: String,
    pub load: String,
    pub tokens: u32,
}

/// Cassandra cluster health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CassandraHealth {
    pub ok: bool,
    pub cluster_name: String,
    pub node_count: u32,
    pub live_nodes: u32,
    pub datacenter_count: u32,
    pub version: String,
}

/// Cassandra shim.
pub struct CassandraShim {
    host: String,
    port: u16,
    jmx_port: u16,
    cluster_name: String,
    health_checks: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CassandraShim {
    pub fn new() -> Self {
        Self {
            host: std::env::var("CASSANDRA_HOST").unwrap_or_else(|_| "localhost".to_string()),
            port: std::env::var("CASSANDRA_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(9042),
            jmx_port: std::env::var("CASSANDRA_JMX_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(7199),
            cluster_name: std::env::var("CASSANDRA_CLUSTER")
                .unwrap_or_else(|_| "local".to_string()),
            health_checks: 0,
            shutdown_tx: None,
        }
    }

    /// Check cluster health via nodetool status.
    pub async fn check_health(&mut self) -> anyhow::Result<CassandraHealth> {
        self.health_checks += 1;

        let output = tokio::process::Command::new("nodetool")
            .args(["-h", &self.host, "-p", &self.jmx_port.to_string(), "status"])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("nodetool status failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut node_count = 0u32;
        let mut live_nodes = 0u32;
        let mut datacenters = std::collections::HashSet::new();

        for line in stdout.lines() {
            if line.starts_with("UN")
                || line.starts_with("DN")
                || line.starts_with("UL")
                || line.starts_with("UM")
                || line.starts_with("UJ")
            {
                node_count += 1;
                if line.starts_with("UN") {
                    live_nodes += 1;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    datacenters.insert(parts[3].to_string());
                }
            }
        }

        let datacenter_count = datacenters.len() as u32;

        // Extract cluster name and version from first line
        let cluster_name = stdout
            .lines()
            .find(|l| l.contains("Datacenter:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or(&self.cluster_name)
            .to_string();

        let version = stdout
            .lines()
            .find(|l| l.contains("Release version:"))
            .and_then(|l| l.split(':').nth(1))
            .unwrap_or("unknown")
            .trim()
            .to_string();

        Ok(CassandraHealth {
            ok: live_nodes > 0,
            cluster_name,
            node_count,
            live_nodes,
            datacenter_count,
            version,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn cluster_name(&self) -> &str {
        &self.cluster_name
    }
}

impl Default for CassandraShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for CassandraShim {
    fn name(&self) -> &str {
        "cassandra"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "CassandraShim initialized (host={}:{})",
            self.host,
            self.port
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("CassandraShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("CassandraShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![Metric::new(
            "cassandra_health_checks_total",
            self.health_checks as f64,
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cassandra_defaults() {
        temp_env::with_vars(
            [
                ("CASSANDRA_HOST", None::<&str>),
                ("CASSANDRA_PORT", None::<&str>),
                ("CASSANDRA_CLUSTER", None::<&str>),
            ],
            || {
                let shim = CassandraShim::new();
                assert_eq!(shim.host(), "localhost");
                assert_eq!(shim.port(), 9042);
                assert_eq!(shim.cluster_name(), "local");
            },
        );
    }

    #[test]
    fn test_cassandra_env_overrides() {
        temp_env::with_vars(
            [
                ("CASSANDRA_HOST", Some("cassandra.prod")),
                ("CASSANDRA_PORT", Some("9043")),
                ("CASSANDRA_JMX_PORT", Some("7200")),
                ("CASSANDRA_CLUSTER", Some("prod-cluster")),
            ],
            || {
                let shim = CassandraShim::new();
                assert_eq!(shim.host(), "cassandra.prod");
                assert_eq!(shim.port(), 9043);
                assert_eq!(shim.cluster_name(), "prod-cluster");
            },
        );
    }

    #[test]
    fn test_cassandra_metrics() {
        let shim = CassandraShim::new();
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 1);
    }

    #[test]
    fn test_cassandra_capability() {
        let shim = CassandraShim::new();
        assert_eq!(shim.name(), "cassandra");
    }

    #[test]
    fn test_cassandra_default_trait() {
        let shim = CassandraShim::default();
        assert_eq!(shim.name(), "cassandra");
        assert_eq!(shim.host(), "localhost");
        assert_eq!(shim.port(), 9042);
    }

    #[test]
    fn test_cassandra_jmx_port_default() {
        temp_env::with_vars([("CASSANDRA_JMX_PORT", None::<&str>)], || {
            let shim = CassandraShim::new();
            assert_eq!(shim.jmx_port, 7199);
        });
    }

    #[test]
    fn test_cassandra_jmx_port_override() {
        temp_env::with_vars([("CASSANDRA_JMX_PORT", Some("7200"))], || {
            let shim = CassandraShim::new();
            assert_eq!(shim.jmx_port, 7200);
        });
    }

    #[test]
    fn test_cassandra_invalid_port_falls_back() {
        temp_env::with_vars([("CASSANDRA_PORT", Some("not_a_port"))], || {
            let shim = CassandraShim::new();
            assert_eq!(shim.port(), 9042);
        });
    }

    #[test]
    fn test_cassandra_invalid_jmx_port_falls_back() {
        temp_env::with_vars([("CASSANDRA_JMX_PORT", Some("invalid"))], || {
            let shim = CassandraShim::new();
            assert_eq!(shim.jmx_port, 7199);
        });
    }

    #[test]
    fn test_cassandra_init_and_start_stop() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = CassandraShim::new();
            let config = Config::default();

            let init_result = shim.init(&config).await;
            assert!(init_result.is_ok());

            let start_result = shim.start().await;
            assert!(start_result.is_ok());
            assert!(shim.shutdown_tx.is_some());

            let stop_result = shim.stop().await;
            assert!(stop_result.is_ok());
            assert!(shim.shutdown_tx.is_none());
        });
    }

    #[test]
    fn test_cassandra_stop_without_start() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = CassandraShim::new();
            let result = shim.stop().await;
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_cassandra_metrics_after_init() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = CassandraShim::new();
            let config = Config::default();
            let _ = shim.init(&config).await;

            let metrics = shim.metrics();
            assert_eq!(metrics.len(), 1);
            assert_eq!(metrics[0].name, "cassandra_health_checks_total");
            assert_eq!(metrics[0].value, 0.0);
        });
    }

    #[test]
    fn test_cassandra_health_serialize() {
        let health = CassandraHealth {
            ok: true,
            cluster_name: "prod-cluster".to_string(),
            node_count: 6,
            live_nodes: 6,
            datacenter_count: 2,
            version: "4.1.0".to_string(),
        };
        let json = serde_json::to_string(&health).unwrap();
        let parsed: CassandraHealth = serde_json::from_str(&json).unwrap();
        assert!(parsed.ok);
        assert_eq!(parsed.node_count, 6);
        assert_eq!(parsed.datacenter_count, 2);
    }

    #[test]
    fn test_cassandra_node_serialize() {
        let node = CassandraNode {
            address: "10.0.0.1".to_string(),
            datacenter: "us-east-1".to_string(),
            rack: "rack-1".to_string(),
            status: "UN".to_string(),
            state: "Normal".to_string(),
            load: "256.00 KiB".to_string(),
            tokens: 256,
        };
        let json = serde_json::to_string(&node).unwrap();
        let parsed: CassandraNode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.address, "10.0.0.1");
        assert_eq!(parsed.tokens, 256);
    }
}
