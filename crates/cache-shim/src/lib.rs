//! Cache shim — query result caching with Redis/Memcached.
//!
//! Intercepts database queries and caches results for faster repeated access.
//!
//! ## Environment Variables
//!
//! ```text
//! CACHE_BACKEND         Backend: redis, memcached (required)
//! CACHE_URL             Backend URL (default: redis://127.0.0.1:6379)
//! CACHE_TTL             Time-to-live in seconds (default: 300)
//! CACHE_MAX_SIZE        Max cache size in bytes (default: 1GB)
//! CACHE_STRATEGY        Eviction: lru, lfu, fifo (default: lru)
//! CACHE_PREFIX          Key prefix (default: "shim:")
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Cache shim.
pub struct CacheShim {
    backend: String,
    url: String,
    ttl_secs: u64,
    max_size: u64,
    strategy: String,
    prefix: String,
    hits: u64,
    misses: u64,
    evictions: u64,
    size_bytes: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CacheShim {
    pub fn new() -> Self {
        Self {
            backend: std::env::var("CACHE_BACKEND").unwrap_or_else(|_| "redis".to_string()),
            url: std::env::var("CACHE_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            ttl_secs: std::env::var("CACHE_TTL").ok().and_then(|s| s.parse().ok()).unwrap_or(300),
            max_size: std::env::var("CACHE_MAX_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(1_073_741_824),
            strategy: std::env::var("CACHE_STRATEGY").unwrap_or_else(|_| "lru".to_string()),
            prefix: std::env::var("CACHE_PREFIX").unwrap_or_else(|_| "shim:".to_string()),
            hits: 0, misses: 0, evictions: 0, size_bytes: 0,
            shutdown_tx: None,
        }
    }
}

impl Default for CacheShim { fn default() -> Self { Self::new() } }

#[async_trait::async_trait]
impl Capability for CacheShim {
    fn name(&self) -> &str { "cache" }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("CacheShim initialized (backend={}, url={}, ttl={}s)", self.backend, self.url, self.ttl_secs);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("CacheShim started (strategy={})", self.strategy);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(true); }
        tracing::info!("CacheShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("cache_hits_total", self.hits as f64),
            Metric::new("cache_misses_total", self.misses as f64),
            Metric::new("cache_evictions_total", self.evictions as f64),
            Metric::new("cache_size_bytes", self.size_bytes as f64),
        ]
    }
}
