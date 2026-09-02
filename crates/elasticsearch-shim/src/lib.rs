//! Elasticsearch shim — health checks and snapshot management.
//!
//! ## Environment Variables
//!
//! ```text
//! ES_URL              Elasticsearch URL (default: http://localhost:9200)
//! ES_INDEX            Index to monitor
//! ES_SNAPSHOT_REPO    Snapshot repository name
//! ES_SNAPSHOT_REPO_URL Repository location URL
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

use loop_retry::{with_backoff, IsRetryable, RetryConfig};

/// Elasticsearch cluster health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EsHealth {
    pub status: String,
    pub cluster_name: String,
    pub node_count: u32,
    pub active_shards: u32,
    pub relocating_shards: u32,
    pub initializing_shards: u32,
    pub unassigned_shards: u32,
}

/// Elasticsearch shim.
pub struct ElasticsearchShim {
    url: String,
    index: String,
    snapshot_repo: String,
    health_checks: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ElasticsearchShim {
    pub fn new() -> Self {
        Self {
            url: std::env::var("ES_URL").unwrap_or_else(|_| "http://localhost:9200".to_string()),
            index: std::env::var("ES_INDEX").unwrap_or_default(),
            snapshot_repo: std::env::var("ES_SNAPSHOT_REPO").unwrap_or_default(),
            health_checks: 0,
            shutdown_tx: None,
        }
    }

    /// Check Elasticsearch cluster health with retry.
    pub async fn check_health(&mut self) -> anyhow::Result<EsHealth> {
        self.health_checks += 1;

        #[derive(Debug)]
        struct AnyError(anyhow::Error);
        impl std::fmt::Display for AnyError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
        impl IsRetryable for AnyError {
            fn is_retryable(&self) -> bool {
                true
            }
        }
        impl From<anyhow::Error> for AnyError {
            fn from(e: anyhow::Error) -> Self {
                Self(e)
            }
        }

        let config = RetryConfig {
            max_retries: 3,
            initial_delay: std::time::Duration::from_secs(1),
            max_delay: std::time::Duration::from_secs(4),
            backoff_multiplier: 2.0,
            jitter: false,
        };

        let url = format!("{}/_cluster/health", self.url);

        with_backoff(&config, || {
            let url = url.clone();
            async move {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .map_err(|e| AnyError(anyhow::Error::from(e)))?;

                let resp = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| AnyError(anyhow::Error::from(e)))?;

                let val: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| AnyError(anyhow::Error::from(e)))?;

                Ok::<EsHealth, AnyError>(EsHealth {
                    status: val["status"].as_str().unwrap_or("unknown").to_string(),
                    cluster_name: val["cluster_name"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string(),
                    node_count: val["number_of_nodes"].as_u64().unwrap_or(0) as u32,
                    active_shards: val["active_shards"].as_u64().unwrap_or(0) as u32,
                    relocating_shards: val["relocating_shards"].as_u64().unwrap_or(0) as u32,
                    initializing_shards: val["initializing_shards"].as_u64().unwrap_or(0) as u32,
                    unassigned_shards: val["unassigned_shards"].as_u64().unwrap_or(0) as u32,
                })
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!(e.into_inner().0))
    }

    /// Create a snapshot repository.
    pub async fn create_snapshot_repo(
        &self,
        repo_name: &str,
        location: &str,
    ) -> anyhow::Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        let url = format!("{}/_snapshot/{}", self.url, repo_name);
        let body = serde_json::json!({
            "type": "fs",
            "settings": {
                "location": location
            }
        });

        client.put(&url).json(&body).send().await?;
        tracing::info!("Snapshot repository created: {}", repo_name);
        Ok(())
    }

    /// Create a snapshot.
    pub async fn create_snapshot(
        &self,
        repo_name: &str,
        snapshot_name: &str,
    ) -> anyhow::Result<()> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?;

        let url = format!("{}/_snapshot/{}/{}", self.url, repo_name, snapshot_name);
        client.put(&url).send().await?;
        tracing::info!("Snapshot created: {}/{}", repo_name, snapshot_name);
        Ok(())
    }

    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn index(&self) -> &str {
        &self.index
    }
    pub fn snapshot_repo(&self) -> &str {
        &self.snapshot_repo
    }
}

impl Default for ElasticsearchShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ElasticsearchShim {
    fn name(&self) -> &str {
        "elasticsearch"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("ElasticsearchShim initialized (url={})", self.url);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ElasticsearchShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("ElasticsearchShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![Metric::new(
            "es_health_checks_total",
            self.health_checks as f64,
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_es_defaults() {
        temp_env::with_vars(
            [("ES_URL", None::<&str>), ("ES_INDEX", None::<&str>)],
            || {
                let shim = ElasticsearchShim::new();
                assert_eq!(shim.url(), "http://localhost:9200");
                assert_eq!(shim.index(), "");
            },
        );
    }

    #[test]
    fn test_es_env_overrides() {
        temp_env::with_vars(
            [
                ("ES_URL", Some("https://es.prod:9200")),
                ("ES_INDEX", Some("logs-*")),
            ],
            || {
                let shim = ElasticsearchShim::new();
                assert_eq!(shim.url(), "https://es.prod:9200");
                assert_eq!(shim.index(), "logs-*");
            },
        );
    }

    #[test]
    fn test_es_metrics() {
        let shim = ElasticsearchShim::new();
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 1);
    }

    #[test]
    fn test_es_capability() {
        let shim = ElasticsearchShim::new();
        assert_eq!(shim.name(), "elasticsearch");
    }

    #[test]
    fn test_es_default_trait() {
        let shim = ElasticsearchShim::default();
        assert_eq!(shim.name(), "elasticsearch");
    }

    #[test]
    fn test_es_snapshot_repo_default() {
        temp_env::with_vars([("ES_SNAPSHOT_REPO", None::<&str>)], || {
            let shim = ElasticsearchShim::new();
            assert_eq!(shim.snapshot_repo(), "");
        });
    }

    #[test]
    fn test_es_snapshot_repo_override() {
        temp_env::with_vars([("ES_SNAPSHOT_REPO", Some("my-repo"))], || {
            let shim = ElasticsearchShim::new();
            assert_eq!(shim.snapshot_repo(), "my-repo");
        });
    }

    #[test]
    fn test_es_init_and_start_stop() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = ElasticsearchShim::new();
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
    fn test_es_stop_without_start() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = ElasticsearchShim::new();
            let result = shim.stop().await;
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_es_metrics_after_init() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = ElasticsearchShim::new();
            let config = Config::default();
            let _ = shim.init(&config).await;

            let metrics = shim.metrics();
            assert_eq!(metrics.len(), 1);
            assert_eq!(metrics[0].name, "es_health_checks_total");
            assert_eq!(metrics[0].value, 0.0);
        });
    }

    #[test]
    fn test_es_health_serialize() {
        let health = EsHealth {
            status: "green".to_string(),
            cluster_name: "prod-cluster".to_string(),
            node_count: 5,
            active_shards: 15,
            relocating_shards: 0,
            initializing_shards: 0,
            unassigned_shards: 0,
        };
        let json = serde_json::to_string(&health).unwrap();
        let parsed: EsHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, "green");
        assert_eq!(parsed.node_count, 5);
        assert_eq!(parsed.active_shards, 15);
    }
}
