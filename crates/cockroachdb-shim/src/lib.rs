#![allow(dead_code)]
//! CockroachDB shim — health checks, topology awareness, and CDC.
//!
//! CockroachDB uses the PostgreSQL wire protocol, so health checks
//! use pg_isready and topology queries use SQL.
//!
//! ## Environment Variables
//!
//! ```text
//! CRDB_HOST              CockroachDB host (default: localhost)
//! CRDB_PORT              CockroachDB port (default: 26257)
//! CRDB_USER              Database user (default: root)
//! CRDB_PASSWORD          Database password
//! CRDB_DATABASE          Database name (default: defaultdb)
//! CRDB_URL               Full connection URL (overrides host/port/user/password)
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// CockroachDB cluster node info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrdbNode {
    pub node_id: i64,
    pub address: String,
    pub locality: String,
    pub is_live: bool,
    pub ranges: i64,
    pub leases: i64,
}

/// CockroachDB cluster health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrdbHealth {
    pub ok: bool,
    pub node_count: u32,
    pub live_nodes: u32,
    pub version: String,
    pub uptime_secs: u64,
}

/// CockroachDB shim.
pub struct CrdbShim {
    host: String,
    port: u16,
    user: String,
    password: String,
    database: String,
    url: Option<String>,
    health_checks: u64,
    topology_queries: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CrdbShim {
    pub fn new() -> Self {
        Self {
            host: std::env::var("CRDB_HOST").unwrap_or_else(|_| "localhost".to_string()),
            port: std::env::var("CRDB_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(26257),
            user: std::env::var("CRDB_USER").unwrap_or_else(|_| "root".to_string()),
            password: std::env::var("CRDB_PASSWORD").unwrap_or_default(),
            database: std::env::var("CRDB_DATABASE").unwrap_or_else(|_| "defaultdb".to_string()),
            url: std::env::var("CRDB_URL").ok(),
            health_checks: 0,
            topology_queries: 0,
            shutdown_tx: None,
        }
    }

    fn connection_string(&self) -> String {
        if let Some(ref url) = self.url {
            return url.clone();
        }
        if self.password.is_empty() {
            format!(
                "postgresql://{}@{}:{}/{}",
                self.user, self.host, self.port, self.database
            )
        } else {
            format!(
                "postgresql://{}:{}@{}:{}/{}",
                self.user, self.password, self.host, self.port, self.database
            )
        }
    }

    /// Check cluster health.
    pub async fn check_health(&mut self) -> anyhow::Result<CrdbHealth> {
        self.health_checks += 1;

        // Use pg_isready for basic connectivity
        let output = tokio::process::Command::new("pg_isready")
            .args([
                "-h",
                &self.host,
                "-p",
                &self.port.to_string(),
                "-U",
                &self.user,
            ])
            .output()
            .await?;

        let ok = output.status.success();

        // Try to get node count from cluster
        let mut node_count = 1u32;
        let mut live_nodes = if ok { 1 } else { 0 };
        let mut version = "unknown".to_string();

        if ok {
            if let Ok(pool) = sqlx::PgPool::connect(&self.connection_string()).await {
                if let Ok(row) = sqlx::query_as::<_, (i64,)>(
                    "SELECT count(*) FROM crdb_internal.gossip_nodes WHERE is_live = true",
                )
                .fetch_one(&pool)
                .await
                {
                    live_nodes = row.0 as u32;
                }
                if let Ok(row) =
                    sqlx::query_as::<_, (i64,)>("SELECT count(*) FROM crdb_internal.gossip_nodes")
                        .fetch_one(&pool)
                        .await
                {
                    node_count = row.0 as u32;
                }
                if let Ok(row) = sqlx::query_as::<_, (String,)>("SHOW cluster_setting('version')")
                    .fetch_one(&pool)
                    .await
                {
                    version = row.0;
                }
                self.topology_queries += 1;
            }
        }

        Ok(CrdbHealth {
            ok,
            node_count,
            live_nodes,
            version,
            uptime_secs: 0,
        })
    }

    /// Get cluster topology.
    pub async fn get_topology(&mut self) -> anyhow::Result<Vec<CrdbNode>> {
        self.topology_queries += 1;

        let pool = sqlx::PgPool::connect(&self.connection_string()).await?;

        let rows = sqlx::query_as::<_, (i64, String, String, bool, i64, i64)>(
            "SELECT node_id, address, locality, is_live, ranges, leases FROM crdb_internal.gossip_nodes ORDER BY node_id",
        )
        .fetch_all(&pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(node_id, address, locality, is_live, ranges, leases)| CrdbNode {
                    node_id,
                    address,
                    locality,
                    is_live,
                    ranges,
                    leases,
                },
            )
            .collect())
    }

    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub fn user(&self) -> &str {
        &self.user
    }
    pub fn database(&self) -> &str {
        &self.database
    }
}

impl Default for CrdbShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for CrdbShim {
    fn name(&self) -> &str {
        "cockroachdb"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("CrdbShim initialized (host={}:{})", self.host, self.port);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("CrdbShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("CrdbShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("crdb_health_checks_total", self.health_checks as f64),
            Metric::new("crdb_topology_queries_total", self.topology_queries as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crdb_defaults() {
        temp_env::with_vars(
            [
                ("CRDB_HOST", None::<&str>),
                ("CRDB_PORT", None::<&str>),
                ("CRDB_USER", None::<&str>),
            ],
            || {
                let shim = CrdbShim::new();
                assert_eq!(shim.host(), "localhost");
                assert_eq!(shim.port(), 26257);
                assert_eq!(shim.user(), "root");
                assert_eq!(shim.database(), "defaultdb");
            },
        );
    }

    #[test]
    fn test_crdb_env_overrides() {
        temp_env::with_vars(
            [
                ("CRDB_HOST", Some("crdb.prod")),
                ("CRDB_PORT", Some("26258")),
            ],
            || {
                let shim = CrdbShim::new();
                assert_eq!(shim.host(), "crdb.prod");
                assert_eq!(shim.port(), 26258);
            },
        );
    }

    #[test]
    fn test_crdb_connection_string() {
        temp_env::with_vars(
            [("CRDB_HOST", Some("myhost")), ("CRDB_PORT", Some("9999"))],
            || {
                let shim = CrdbShim::new();
                let cs = shim.connection_string();
                assert!(cs.contains("myhost"));
                assert!(cs.contains("9999"));
            },
        );
    }

    #[test]
    fn test_crdb_url_override() {
        temp_env::with_vars(
            [("CRDB_URL", Some("postgresql://admin:pass@remote:9000/mydb"))],
            || {
                let shim = CrdbShim::new();
                assert_eq!(
                    shim.connection_string(),
                    "postgresql://admin:pass@remote:9000/mydb"
                );
            },
        );
    }

    #[test]
    fn test_crdb_metrics() {
        let shim = CrdbShim::new();
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].name, "crdb_health_checks_total");
    }

    #[test]
    fn test_crdb_capability() {
        let shim = CrdbShim::new();
        assert_eq!(shim.name(), "cockroachdb");
    }
}
