//! Sharding shim — automatic sharding for distributed databases.
//!
//! Routes queries to the correct shard based on a shard key.
//!
//! ## Environment Variables
//!
//! ```text
//! SHARDING_STRATEGY     Strategy: hash, range, directory (default: hash)
//! SHARDING_KEY          Shard key column (required)
//! SHARDING_COUNT        Number of shards (default: 4)
//! SHARDING_ADDRESSES    Comma-separated shard addresses
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Sharding shim.
pub struct ShardingShim {
    strategy: String,
    shard_key: String,
    shard_count: u32,
    addresses: Vec<String>,
    queries_routed: u64,
    queries_missed: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ShardingShim {
    pub fn new() -> Self {
        Self {
            strategy: std::env::var("SHARDING_STRATEGY").unwrap_or_else(|_| "hash".to_string()),
            shard_key: std::env::var("SHARDING_KEY").unwrap_or_default(),
            shard_count: std::env::var("SHARDING_COUNT").ok().and_then(|s| s.parse().ok()).unwrap_or(4),
            addresses: std::env::var("SHARDING_ADDRESSES").unwrap_or_default()
                .split(',').filter(|s| !s.is_empty()).map(|s| s.trim().to_string()).collect(),
            queries_routed: 0, queries_missed: 0, shutdown_tx: None,
        }
    }
}

impl Default for ShardingShim { fn default() -> Self { Self::new() } }

#[async_trait::async_trait]
impl Capability for ShardingShim {
    fn name(&self) -> &str { "sharding" }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("ShardingShim initialized (strategy={}, key={}, shards={})",
            self.strategy, self.shard_key, self.shard_count);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ShardingShim started ({} shards)", self.shard_count);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(true); }
        tracing::info!("ShardingShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("sharding_queries_routed", self.queries_routed as f64),
            Metric::new("sharding_queries_missed", self.queries_missed as f64),
        ]
    }
}
