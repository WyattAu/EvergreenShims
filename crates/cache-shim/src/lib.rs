//! Cache shim — query result caching with in-process LRU/LFU/FIFO eviction.
//!
//! Intercepts database queries and caches results for faster repeated access.
//! Runs in-process (single-node deployments); not backed by Redis/Memcached.
//!
//! ## Environment Variables
//!
//! ```text
//! CACHE_TTL             Time-to-live in seconds (default: 300)
//! CACHE_MAX_ENTRIES     Max cache entries (default: 10000)
//! CACHE_MAX_SIZE        Max cache size in bytes (default: 1GB)
//! CACHE_STRATEGY        Eviction: lru, lfu, fifo (default: lru)
//! CACHE_PREFIX          Key prefix (default: "shim:")
//! CACHE_SWEEP_INTERVAL  Background sweep interval in seconds (default: 60)
//! ```

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// A cache entry with value, TTL, and access metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub key: String,
    pub value: Vec<u8>,
    pub size_bytes: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub last_accessed: chrono::DateTime<chrono::Utc>,
    pub access_count: u64,
}

/// Eviction strategy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EvictionStrategy {
    Lru,
    Lfu,
    Fifo,
}

impl std::fmt::Display for EvictionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lru => write!(f, "lru"),
            Self::Lfu => write!(f, "lfu"),
            Self::Fifo => write!(f, "fifo"),
        }
    }
}

impl std::str::FromStr for EvictionStrategy {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "lru" => Ok(Self::Lru),
            "lfu" => Ok(Self::Lfu),
            "fifo" => Ok(Self::Fifo),
            _ => Err(format!("Unknown eviction strategy: {}", s)),
        }
    }
}

/// Inner cache state protected by RwLock.
struct CacheInner {
    entries: HashMap<String, CacheEntry>,
    /// Access order for LRU: keys in order of most-recently-used (back) to least (front).
    access_order: VecDeque<String>,
    /// Insertion order for FIFO.
    insert_order: VecDeque<String>,
    hits: u64,
    misses: u64,
    evictions: u64,
    size_bytes: u64,
}

impl CacheInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            access_order: VecDeque::new(),
            insert_order: VecDeque::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
            size_bytes: 0,
        }
    }
}

/// Cache shim with in-process LRU/LFU/FIFO eviction.
pub struct CacheShim {
    ttl_secs: u64,
    max_size: u64,
    max_entries: usize,
    strategy: EvictionStrategy,
    prefix: String,
    sweep_interval_secs: u64,
    inner: Arc<RwLock<CacheInner>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CacheShim {
    pub fn new() -> Self {
        let strategy_str = std::env::var("CACHE_STRATEGY").unwrap_or_else(|_| "lru".to_string());
        let strategy = strategy_str.parse().unwrap_or(EvictionStrategy::Lru);

        Self {
            ttl_secs: std::env::var("CACHE_TTL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            max_entries: std::env::var("CACHE_MAX_ENTRIES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10_000),
            max_size: std::env::var("CACHE_MAX_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1_073_741_824),
            strategy,
            prefix: std::env::var("CACHE_PREFIX").unwrap_or_else(|_| "shim:".to_string()),
            sweep_interval_secs: std::env::var("CACHE_SWEEP_INTERVAL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60),
            inner: Arc::new(RwLock::new(CacheInner::new())),
            shutdown_tx: None,
        }
    }

    /// Build a full cache key with prefix.
    pub fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// Get a value from cache. Returns None on miss or expired.
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let full_key = self.full_key(key);
        let mut inner = self.inner.write();

        // Check existence and expiry first
        let is_expired = match inner.entries.get(&full_key) {
            Some(e) => chrono::Utc::now() > e.expires_at,
            None => {
                inner.misses += 1;
                return None;
            }
        };

        if is_expired {
            if let Some(entry) = inner.entries.remove(&full_key) {
                inner.size_bytes -= entry.size_bytes as u64;
            }
            inner.access_order.retain(|k| k != &full_key);
            inner.insert_order.retain(|k| k != &full_key);
            inner.misses += 1;
            return None;
        }

        // Extract the value, then update metadata
        let value = inner.entries.get(&full_key).map(|e| e.value.clone())?;

        // Update entry metadata
        if let Some(entry) = inner.entries.get_mut(&full_key) {
            entry.last_accessed = chrono::Utc::now();
            entry.access_count += 1;
        }

        // Update access order: move to back (most recently used)
        inner.access_order.retain(|k| k != &full_key);
        inner.access_order.push_back(full_key);
        inner.hits += 1;
        Some(value)
    }

    /// Set a value in the cache. Evicts if over max_size or max_entries.
    pub fn set(&self, key: &str, value: &[u8]) -> bool {
        let full_key = self.full_key(key);
        let value_size = value.len() as u64;

        if value_size > self.max_size {
            return false;
        }

        let mut inner = self.inner.write();

        if let Some(existing) = inner.entries.remove(&full_key) {
            inner.size_bytes -= existing.size_bytes as u64;
            inner.access_order.retain(|k| k != &full_key);
            inner.insert_order.retain(|k| k != &full_key);
        }

        inner.size_bytes += value_size;
        // Evict by size
        while inner.size_bytes > self.max_size {
            if !self.evict_one_inner(&mut inner) {
                break;
            }
        }
        // Evict by count
        while inner.entries.len() >= self.max_entries {
            if !self.evict_one_inner(&mut inner) {
                break;
            }
        }

        let now = chrono::Utc::now();
        inner.entries.insert(
            full_key.clone(),
            CacheEntry {
                key: key.to_string(),
                value: value.to_vec(),
                size_bytes: value.len(),
                created_at: now,
                expires_at: now + chrono::Duration::seconds(self.ttl_secs as i64),
                last_accessed: now,
                access_count: 0,
            },
        );
        inner.access_order.push_back(full_key.clone());
        inner.insert_order.push_back(full_key);

        true
    }

    /// Invalidate a specific key.
    pub fn invalidate(&self, key: &str) -> bool {
        let full_key = self.full_key(key);
        let mut inner = self.inner.write();
        if let Some(entry) = inner.entries.remove(&full_key) {
            inner.size_bytes -= entry.size_bytes as u64;
            inner.access_order.retain(|k| k != &full_key);
            inner.insert_order.retain(|k| k != &full_key);
            true
        } else {
            false
        }
    }

    /// Invalidate all entries matching a prefix pattern.
    pub fn invalidate_prefix(&self, prefix: &str) -> u64 {
        let full_prefix = format!("{}{}", self.prefix, prefix);
        let mut inner = self.inner.write();
        let before = inner.entries.len();
        let keys_to_remove: Vec<String> = inner
            .entries
            .keys()
            .filter(|k| k.starts_with(&full_prefix))
            .cloned()
            .collect();

        let count = keys_to_remove.len() as u64;
        for key in keys_to_remove {
            if let Some(entry) = inner.entries.remove(&key) {
                inner.size_bytes -= entry.size_bytes as u64;
                inner.access_order.retain(|k| k != &key);
                inner.insert_order.retain(|k| k != &key);
            }
        }

        let removed = (before - inner.entries.len()) as u64;
        debug_assert_eq!(removed, count);
        count
    }

    /// Invalidate all expired entries.
    pub fn purge_expired(&self) -> u64 {
        let now = chrono::Utc::now();
        let mut inner = self.inner.write();
        let before = inner.entries.len();

        let expired_keys: Vec<String> = inner
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at < now)
            .map(|(k, _)| k.clone())
            .collect();

        for key in expired_keys {
            if let Some(entry) = inner.entries.remove(&key) {
                inner.size_bytes -= entry.size_bytes as u64;
                inner.access_order.retain(|k| k != &key);
                inner.insert_order.retain(|k| k != &key);
            }
        }

        (before - inner.entries.len()) as u64
    }

    /// Clear all entries.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.entries.clear();
        inner.access_order.clear();
        inner.insert_order.clear();
        inner.size_bytes = 0;
    }

    /// Get hit/miss ratio (0.0 to 1.0).
    pub fn hit_rate(&self) -> f64 {
        let inner = self.inner.read();
        let total = inner.hits + inner.misses;
        if total == 0 {
            0.0
        } else {
            inner.hits as f64 / total as f64
        }
    }

    /// Get number of entries.
    pub fn entry_count(&self) -> usize {
        self.inner.read().entries.len()
    }

    /// Check if a key exists and is not expired.
    pub fn exists(&self, key: &str) -> bool {
        let full_key = self.full_key(key);
        let inner = self.inner.read();
        if let Some(entry) = inner.entries.get(&full_key) {
            chrono::Utc::now() <= entry.expires_at
        } else {
            false
        }
    }

    fn evict_one_inner(&self, inner: &mut CacheInner) -> bool {
        if inner.entries.is_empty() {
            return false;
        }

        let victim_key = match self.strategy {
            EvictionStrategy::Lru => inner.access_order.front().cloned(),
            EvictionStrategy::Fifo => inner.insert_order.front().cloned(),
            EvictionStrategy::Lfu => inner
                .entries
                .iter()
                .min_by_key(|(_, e)| e.access_count)
                .map(|(k, _)| k.clone()),
        };

        if let Some(key) = victim_key {
            if let Some(entry) = inner.entries.remove(&key) {
                inner.size_bytes -= entry.size_bytes as u64;
                inner.evictions += 1;
                inner.access_order.retain(|k| k != &key);
                inner.insert_order.retain(|k| k != &key);
            }
            true
        } else {
            false
        }
    }
}

impl Default for CacheShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for CacheShim {
    fn name(&self) -> &str {
        "cache"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "CacheShim initialized (ttl={}s, max_entries={}, max_size={}, strategy={})",
            self.ttl_secs,
            self.max_entries,
            self.max_size,
            self.strategy
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Spawn background sweep task
        let inner = Arc::clone(&self.inner);
        let interval = std::time::Duration::from_secs(self.sweep_interval_secs);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {
                        Self::sweep_expired(&inner);
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::debug!("CacheShim sweep task shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!(
            "CacheShim started (strategy={}, sweep_interval={}s)",
            self.strategy,
            self.sweep_interval_secs
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("CacheShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let inner = self.inner.read();
        vec![
            Metric::new("cache_hits_total", inner.hits as f64),
            Metric::new("cache_misses_total", inner.misses as f64),
            Metric::new("cache_evictions_total", inner.evictions as f64),
            Metric::new("cache_size_bytes", inner.size_bytes as f64),
            Metric::new("cache_entries", inner.entries.len() as f64),
            Metric::new("cache_hit_rate", self.hit_rate()),
        ]
    }
}

impl CacheShim {
    /// Background sweep: remove expired entries.
    fn sweep_expired(inner: &RwLock<CacheInner>) {
        let now = chrono::Utc::now();
        let mut inner = inner.write();
        let expired_keys: Vec<String> = inner
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at < now)
            .map(|(k, _)| k.clone())
            .collect();

        let count = expired_keys.len();
        for key in expired_keys {
            if let Some(entry) = inner.entries.remove(&key) {
                inner.size_bytes -= entry.size_bytes as u64;
                inner.access_order.retain(|k| k != &key);
                inner.insert_order.retain(|k| k != &key);
            }
        }

        if count > 0 {
            tracing::debug!("Background sweep removed {} expired entries", count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eviction_strategy_parse() {
        assert_eq!(
            "lru".parse::<EvictionStrategy>().unwrap(),
            EvictionStrategy::Lru
        );
        assert_eq!(
            "lfu".parse::<EvictionStrategy>().unwrap(),
            EvictionStrategy::Lfu
        );
        assert_eq!(
            "fifo".parse::<EvictionStrategy>().unwrap(),
            EvictionStrategy::Fifo
        );
        assert!("random".parse::<EvictionStrategy>().is_err());
    }

    #[test]
    fn test_eviction_strategy_display() {
        assert_eq!(EvictionStrategy::Lru.to_string(), "lru");
        assert_eq!(EvictionStrategy::Lfu.to_string(), "lfu");
    }

    #[test]
    fn test_full_key_with_prefix() {
        let shim = CacheShim {
            prefix: "shim:".to_string(),
            ..CacheShim::new()
        };
        assert_eq!(shim.full_key("users:1"), "shim:users:1");
    }

    #[test]
    fn test_set_and_get() {
        let shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            ..CacheShim::new()
        };
        shim.set("key1", b"value1");
        assert_eq!(shim.get("key1"), Some(b"value1".to_vec()));
        {
            let inner = shim.inner.read();
            assert_eq!(inner.hits, 1);
            assert_eq!(inner.misses, 0);
        }
    }

    #[test]
    fn test_get_miss() {
        let shim = CacheShim::new();
        assert_eq!(shim.get("nonexistent"), None);
        let inner = shim.inner.read();
        assert_eq!(inner.misses, 1);
    }

    #[test]
    fn test_set_overwrite() {
        let shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            ..CacheShim::new()
        };
        shim.set("key1", b"old");
        shim.set("key1", b"new");
        assert_eq!(shim.get("key1"), Some(b"new".to_vec()));
        assert_eq!(shim.entry_count(), 1);
    }

    #[test]
    fn test_set_too_large() {
        let shim = CacheShim {
            max_size: 10,
            ..CacheShim::new()
        };
        assert!(!shim.set("key1", &[0u8; 100]));
    }

    #[test]
    fn test_invalidate() {
        let shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            ..CacheShim::new()
        };
        shim.set("key1", b"value1");
        assert!(shim.invalidate("key1"));
        assert_eq!(shim.get("key1"), None);
    }

    #[test]
    fn test_invalidate_miss() {
        let shim = CacheShim::new();
        assert!(!shim.invalidate("nonexistent"));
    }

    #[test]
    fn test_invalidate_prefix() {
        let shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            ..CacheShim::new()
        };
        shim.set("user:1", b"a");
        shim.set("user:2", b"b");
        shim.set("order:1", b"c");

        let removed = shim.invalidate_prefix("user:");
        assert_eq!(removed, 2);
        assert_eq!(shim.entry_count(), 1);
        assert!(shim.exists("order:1"));
    }

    #[test]
    fn test_clear() {
        let shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            ..CacheShim::new()
        };
        shim.set("a", b"1");
        shim.set("b", b"2");
        shim.set("c", b"3");

        shim.clear();
        assert_eq!(shim.entry_count(), 0);
        let inner = shim.inner.read();
        assert_eq!(inner.size_bytes, 0);
    }

    #[test]
    fn test_exists() {
        let shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            ..CacheShim::new()
        };
        shim.set("key1", b"value1");
        assert!(shim.exists("key1"));
        assert!(!shim.exists("nonexistent"));
    }

    #[test]
    fn test_hit_rate() {
        let shim = CacheShim::new();
        assert_eq!(shim.hit_rate(), 0.0);

        {
            let mut inner = shim.inner.write();
            inner.hits = 3;
            inner.misses = 1;
        }
        assert!((shim.hit_rate() - 0.75).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = CacheShim::new();
        {
            let mut inner = shim.inner.write();
            inner.hits = 10;
            inner.misses = 2;
            inner.evictions = 1;
            inner.size_bytes = 5000;
        }
        shim.set("a", b"1");

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 6);
        assert_eq!(metrics[0].name, "cache_hits_total");
        assert_eq!(metrics[0].value, 10.0);
        assert_eq!(metrics[5].name, "cache_hit_rate");
    }

    #[test]
    fn test_lru_eviction_order() {
        let shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            max_entries: 3,
            strategy: EvictionStrategy::Lru,
            ..CacheShim::new()
        };
        shim.set("a", b"1");
        shim.set("b", b"2");
        shim.set("c", b"3");
        // Access "a" to make it recently used
        shim.get("a");
        // Insert "d" should evict "b" (least recently used)
        shim.set("d", b"4");

        assert!(!shim.exists("b"));
        assert!(shim.exists("a"));
        assert!(shim.exists("c"));
        assert!(shim.exists("d"));
    }

    #[test]
    fn test_fifo_eviction_order() {
        let shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            max_entries: 3,
            strategy: EvictionStrategy::Fifo,
            ..CacheShim::new()
        };
        shim.set("a", b"1");
        shim.set("b", b"2");
        shim.set("c", b"3");
        // Access "a" but FIFO ignores access recency
        shim.get("a");
        // Insert "d" should evict "a" (first in)
        shim.set("d", b"4");

        assert!(!shim.exists("a"));
        assert!(shim.exists("b"));
        assert!(shim.exists("c"));
        assert!(shim.exists("d"));
    }
}
