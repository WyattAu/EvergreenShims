#![allow(dead_code)]
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

use std::collections::HashMap;

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

/// Cache shim with in-process LRU/LFU/FIFO eviction.
pub struct CacheShim {
    backend: String,
    url: String,
    ttl_secs: u64,
    max_size: u64,
    strategy: EvictionStrategy,
    prefix: String,
    entries: HashMap<String, CacheEntry>,
    hits: u64,
    misses: u64,
    evictions: u64,
    size_bytes: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CacheShim {
    pub fn new() -> Self {
        let strategy_str = std::env::var("CACHE_STRATEGY").unwrap_or_else(|_| "lru".to_string());
        let strategy = strategy_str.parse().unwrap_or(EvictionStrategy::Lru);

        Self {
            backend: std::env::var("CACHE_BACKEND").unwrap_or_else(|_| "redis".to_string()),
            url: std::env::var("CACHE_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            ttl_secs: std::env::var("CACHE_TTL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            max_size: std::env::var("CACHE_MAX_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1_073_741_824),
            strategy,
            prefix: std::env::var("CACHE_PREFIX").unwrap_or_else(|_| "shim:".to_string()),
            entries: HashMap::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
            size_bytes: 0,
            shutdown_tx: None,
        }
    }

    /// Build a full cache key with prefix.
    pub fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// Get a value from cache. Returns None on miss or expired.
    pub fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        let full_key = self.full_key(key);
        let entry = match self.entries.get_mut(&full_key) {
            Some(e) => e,
            None => {
                self.misses += 1;
                return None;
            }
        };

        if chrono::Utc::now() > entry.expires_at {
            let entry_size = entry.size_bytes as u64;
            self.entries.remove(&full_key);
            self.size_bytes -= entry_size;
            self.misses += 1;
            return None;
        }

        entry.last_accessed = chrono::Utc::now();
        entry.access_count += 1;
        self.hits += 1;
        Some(entry.value.clone())
    }

    /// Set a value in the cache. Evicts if over max_size.
    pub fn set(&mut self, key: &str, value: &[u8]) -> bool {
        let full_key = self.full_key(key);
        let value_size = value.len() as u64;

        if value_size > self.max_size {
            return false;
        }

        if let Some(existing) = self.entries.remove(&full_key) {
            self.size_bytes -= existing.size_bytes as u64;
        }

        self.size_bytes += value_size;
        while self.size_bytes > self.max_size {
            if !self.evict_one() {
                break;
            }
        }

        let now = chrono::Utc::now();
        self.entries.insert(
            full_key,
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

        true
    }

    /// Invalidate a specific key.
    pub fn invalidate(&mut self, key: &str) -> bool {
        let full_key = self.full_key(key);
        if let Some(entry) = self.entries.remove(&full_key) {
            self.size_bytes -= entry.size_bytes as u64;
            true
        } else {
            false
        }
    }

    /// Invalidate all entries matching a prefix pattern.
    pub fn invalidate_prefix(&mut self, prefix: &str) -> u64 {
        let full_prefix = format!("{}{}", self.prefix, prefix);
        let before = self.entries.len();
        let keys_to_remove: Vec<String> = self
            .entries
            .keys()
            .filter(|k| k.starts_with(&full_prefix))
            .cloned()
            .collect();

        for key in keys_to_remove {
            if let Some(entry) = self.entries.remove(&key) {
                self.size_bytes -= entry.size_bytes as u64;
            }
        }

        (before - self.entries.len()) as u64
    }

    /// Invalidate all expired entries.
    pub fn purge_expired(&mut self) -> u64 {
        let now = chrono::Utc::now();
        let before = self.entries.len();

        let expired_keys: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at < now)
            .map(|(k, _)| k.clone())
            .collect();

        for key in expired_keys {
            if let Some(entry) = self.entries.remove(&key) {
                self.size_bytes -= entry.size_bytes as u64;
            }
        }

        (before - self.entries.len()) as u64
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.size_bytes = 0;
    }

    /// Get hit/miss ratio (0.0 to 1.0).
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Get number of entries.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Check if a key exists and is not expired.
    pub fn exists(&self, key: &str) -> bool {
        let full_key = self.full_key(key);
        if let Some(entry) = self.entries.get(&full_key) {
            chrono::Utc::now() <= entry.expires_at
        } else {
            false
        }
    }

    fn evict_one(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }

        let victim_key = match self.strategy {
            EvictionStrategy::Lru => self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_accessed)
                .map(|(k, _)| k.clone()),
            EvictionStrategy::Lfu => self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.access_count)
                .map(|(k, _)| k.clone()),
            EvictionStrategy::Fifo => self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.created_at)
                .map(|(k, _)| k.clone()),
        };

        if let Some(key) = victim_key {
            if let Some(entry) = self.entries.remove(&key) {
                self.size_bytes -= entry.size_bytes as u64;
                self.evictions += 1;
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
            "CacheShim initialized (backend={}, url={}, ttl={}s)",
            self.backend,
            self.url,
            self.ttl_secs
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("CacheShim started (strategy={})", self.strategy);
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
        vec![
            Metric::new("cache_hits_total", self.hits as f64),
            Metric::new("cache_misses_total", self.misses as f64),
            Metric::new("cache_evictions_total", self.evictions as f64),
            Metric::new("cache_size_bytes", self.size_bytes as f64),
            Metric::new("cache_entries", self.entries.len() as f64),
            Metric::new("cache_hit_rate", self.hit_rate()),
        ]
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
        let mut shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            ..CacheShim::new()
        };
        shim.set("key1", b"value1");
        assert_eq!(shim.get("key1"), Some(b"value1".to_vec()));
        assert_eq!(shim.hits, 1);
        assert_eq!(shim.misses, 0);
    }

    #[test]
    fn test_get_miss() {
        let mut shim = CacheShim::new();
        assert_eq!(shim.get("nonexistent"), None);
        assert_eq!(shim.misses, 1);
    }

    #[test]
    fn test_set_overwrite() {
        let mut shim = CacheShim {
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
        let mut shim = CacheShim {
            max_size: 10,
            ..CacheShim::new()
        };
        assert!(!shim.set("key1", &[0u8; 100]));
    }

    #[test]
    fn test_invalidate() {
        let mut shim = CacheShim {
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
        let mut shim = CacheShim::new();
        assert!(!shim.invalidate("nonexistent"));
    }

    #[test]
    fn test_invalidate_prefix() {
        let mut shim = CacheShim {
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
        let mut shim = CacheShim {
            ttl_secs: 3600,
            max_size: 1_000_000,
            ..CacheShim::new()
        };
        shim.set("a", b"1");
        shim.set("b", b"2");
        shim.set("c", b"3");

        shim.clear();
        assert_eq!(shim.entry_count(), 0);
        assert_eq!(shim.size_bytes, 0);
    }

    #[test]
    fn test_exists() {
        let mut shim = CacheShim {
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
        let mut shim = CacheShim::new();
        assert_eq!(shim.hit_rate(), 0.0);

        shim.hits = 3;
        shim.misses = 1;
        assert!((shim.hit_rate() - 0.75).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = CacheShim {
            hits: 10,
            misses: 2,
            evictions: 1,
            size_bytes: 5000,
            ..CacheShim::new()
        };
        shim.set("a", b"1");

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 6);
        assert_eq!(metrics[0].name, "cache_hits_total");
        assert_eq!(metrics[0].value, 10.0);
        assert_eq!(metrics[5].name, "cache_hit_rate");
    }
}
