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
//! SHARDING_VNODES       Virtual nodes per shard for hash ring (default: 150)
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Sharding strategy enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShardingStrategy {
    Hash,
    Range,
    Directory,
}

impl std::fmt::Display for ShardingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hash => write!(f, "hash"),
            Self::Range => write!(f, "range"),
            Self::Directory => write!(f, "directory"),
        }
    }
}

impl std::str::FromStr for ShardingStrategy {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hash" => Ok(Self::Hash),
            "range" => Ok(Self::Range),
            "directory" => Ok(Self::Directory),
            _ => Err(format!("Unknown sharding strategy: {}", s)),
        }
    }
}

/// A shard mapping entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardMapping {
    pub shard_id: u32,
    pub address: String,
    pub range_start: Option<i64>,
    pub range_end: Option<i64>,
    pub healthy: bool,
}

/// A point on the consistent hash ring.
#[derive(Debug, Clone)]
struct HashRingPoint {
    hash: u64,
    shard_id: u32,
}

/// Sharding shim.
pub struct ShardingShim {
    strategy: ShardingStrategy,
    shard_key: String,
    shard_count: u32,
    #[allow(dead_code)]
    addresses: Vec<String>,
    vnodes: usize,
    shards: HashMap<u32, ShardMapping>,
    hash_ring: Vec<HashRingPoint>,
    directory: HashMap<String, u32>,
    queries_routed: u64,
    queries_missed: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ShardingShim {
    pub fn new() -> Self {
        let strategy_str =
            std::env::var("SHARDING_STRATEGY").unwrap_or_else(|_| "hash".to_string());
        let strategy = strategy_str.parse().unwrap_or(ShardingStrategy::Hash);
        let vnodes = std::env::var("SHARDING_VNODES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(150);

        let shard_count = std::env::var("SHARDING_COUNT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4);

        let addresses: Vec<String> = std::env::var("SHARDING_ADDRESSES")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect();

        let mut shim = Self {
            strategy,
            shard_key: std::env::var("SHARDING_KEY").unwrap_or_default(),
            shard_count,
            addresses: addresses.clone(),
            vnodes,
            shards: HashMap::new(),
            hash_ring: Vec::new(),
            directory: HashMap::new(),
            queries_routed: 0,
            queries_missed: 0,
            shutdown_tx: None,
        };

        for (i, addr) in addresses.iter().enumerate() {
            let shard_id = i as u32;
            shim.shards.insert(
                shard_id,
                ShardMapping {
                    shard_id,
                    address: addr.clone(),
                    range_start: None,
                    range_end: None,
                    healthy: true,
                },
            );
        }

        shim.build_hash_ring();
        shim
    }

    /// Build the consistent hash ring with virtual nodes.
    fn build_hash_ring(&mut self) {
        self.hash_ring.clear();
        for (&shard_id, shard) in &self.shards {
            for i in 0..self.vnodes {
                let key = format!("{}:vnode:{}", shard.address, i);
                let hash = Self::hash_key(&key);
                self.hash_ring.push(HashRingPoint { hash, shard_id });
            }
        }
        self.hash_ring.sort_by_key(|p| p.hash);
    }

    /// Simple FNV-1a-like hash function for keys.
    pub fn hash_key(key: &str) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in key.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Route a key to a shard using the configured strategy.
    pub fn route(&mut self, key: &str) -> anyhow::Result<(u32, String)> {
        let (shard_id, address) = match self.strategy {
            ShardingStrategy::Hash => self.route_hash(key)?,
            ShardingStrategy::Range => self.route_range(key)?,
            ShardingStrategy::Directory => self.route_directory(key)?,
        };

        self.queries_routed += 1;
        Ok((shard_id, address))
    }

    fn route_hash(&self, key: &str) -> anyhow::Result<(u32, String)> {
        let hash = Self::hash_key(key);

        if self.hash_ring.is_empty() {
            anyhow::bail!("No shards available in hash ring");
        }

        let idx = match self.hash_ring.binary_search_by(|p| p.hash.cmp(&hash)) {
            Ok(i) => i,
            Err(i) => i % self.hash_ring.len(),
        };

        let point = &self.hash_ring[idx];
        let shard = self
            .shards
            .get(&point.shard_id)
            .ok_or_else(|| anyhow::anyhow!("Shard {} not found", point.shard_id))?;

        if !shard.healthy {
            return Err(anyhow::anyhow!("Shard {} is unhealthy", point.shard_id));
        }

        Ok((shard.shard_id, shard.address.clone()))
    }

    fn route_range(&self, key: &str) -> anyhow::Result<(u32, String)> {
        let val: i64 = key
            .parse()
            .map_err(|_| anyhow::anyhow!("Range strategy requires numeric keys, got: {}", key))?;

        for shard in self.shards.values() {
            if !shard.healthy {
                continue;
            }
            if let (Some(start), Some(end)) = (shard.range_start, shard.range_end) {
                if val >= start && val < end {
                    return Ok((shard.shard_id, shard.address.clone()));
                }
            }
        }

        anyhow::bail!("No shard found for range value: {}", val)
    }

    fn route_directory(&self, key: &str) -> anyhow::Result<(u32, String)> {
        self.directory
            .get(key)
            .and_then(|&shard_id| self.shards.get(&shard_id))
            .filter(|shard| shard.healthy)
            .map(|shard| (shard.shard_id, shard.address.clone()))
            .ok_or_else(|| anyhow::anyhow!("No mapping found for key: {}", key))
    }

    /// Set a range mapping for a shard.
    pub fn set_range(&mut self, shard_id: u32, start: i64, end: i64) -> anyhow::Result<()> {
        if let Some(shard) = self.shards.get_mut(&shard_id) {
            shard.range_start = Some(start);
            shard.range_end = Some(end);
            Ok(())
        } else {
            anyhow::bail!("Shard {} not found", shard_id)
        }
    }

    /// Add a directory mapping.
    pub fn add_directory_mapping(&mut self, key: &str, shard_id: u32) {
        self.directory.insert(key.to_string(), shard_id);
    }

    /// Mark a shard as healthy/unhealthy.
    pub fn set_shard_health(&mut self, shard_id: u32, healthy: bool) -> bool {
        if let Some(shard) = self.shards.get_mut(&shard_id) {
            shard.healthy = healthy;
            true
        } else {
            false
        }
    }

    /// Get shard mapping.
    pub fn get_shard(&self, shard_id: u32) -> Option<&ShardMapping> {
        self.shards.get(&shard_id)
    }

    /// Get all healthy shard IDs.
    pub fn healthy_shards(&self) -> Vec<u32> {
        self.shards
            .values()
            .filter(|s| s.healthy)
            .map(|s| s.shard_id)
            .collect()
    }

    /// Get routing hit rate.
    pub fn hit_rate(&self) -> f64 {
        let total = self.queries_routed + self.queries_missed;
        if total == 0 {
            0.0
        } else {
            self.queries_routed as f64 / total as f64
        }
    }
}

impl Default for ShardingShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ShardingShim {
    fn name(&self) -> &str {
        "sharding"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "ShardingShim initialized (strategy={}, key={}, shards={})",
            self.strategy,
            self.shard_key,
            self.shard_count
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ShardingShim started ({} shards)", self.shard_count);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("ShardingShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let healthy = self.shards.values().filter(|s| s.healthy).count();
        vec![
            Metric::new("sharding_queries_routed", self.queries_routed as f64),
            Metric::new("sharding_queries_missed", self.queries_missed as f64),
            Metric::new("sharding_shards_total", self.shards.len() as f64),
            Metric::new("sharding_shards_healthy", healthy as f64),
            Metric::new("sharding_hit_rate", self.hit_rate()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strategy_parse() {
        assert_eq!(
            "hash".parse::<ShardingStrategy>().unwrap(),
            ShardingStrategy::Hash
        );
        assert_eq!(
            "range".parse::<ShardingStrategy>().unwrap(),
            ShardingStrategy::Range
        );
        assert_eq!(
            "directory".parse::<ShardingStrategy>().unwrap(),
            ShardingStrategy::Directory
        );
        assert!("invalid".parse::<ShardingStrategy>().is_err());
    }

    #[test]
    fn test_hash_key_deterministic() {
        let h1 = ShardingShim::hash_key("user:123");
        let h2 = ShardingShim::hash_key("user:123");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_key_distributes() {
        let h1 = ShardingShim::hash_key("user:1");
        let h2 = ShardingShim::hash_key("user:2");
        let h3 = ShardingShim::hash_key("user:3");
        assert_ne!(h1, h2);
        assert_ne!(h2, h3);
    }

    #[test]
    fn test_route_hash_consistent() {
        let mut shim = ShardingShim {
            strategy: ShardingStrategy::Hash,
            addresses: vec!["shard-0:5432".to_string(), "shard-1:5432".to_string()],
            vnodes: 150,
            ..ShardingShim::new()
        };
        shim.shards.clear();
        shim.shards.insert(
            0,
            ShardMapping {
                shard_id: 0,
                address: "shard-0:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );
        shim.shards.insert(
            1,
            ShardMapping {
                shard_id: 1,
                address: "shard-1:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );
        shim.build_hash_ring();

        let (id1, _) = shim.route("user:123").unwrap();
        let (id2, _) = shim.route("user:123").unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_route_range() {
        let mut shim = ShardingShim {
            strategy: ShardingStrategy::Range,
            shards: HashMap::new(),
            ..ShardingShim::new()
        };
        shim.shards.insert(
            0,
            ShardMapping {
                shard_id: 0,
                address: "shard-0:5432".to_string(),
                range_start: Some(0),
                range_end: Some(100),
                healthy: true,
            },
        );
        shim.shards.insert(
            1,
            ShardMapping {
                shard_id: 1,
                address: "shard-1:5432".to_string(),
                range_start: Some(100),
                range_end: Some(200),
                healthy: true,
            },
        );

        let (id, _) = shim.route("50").unwrap();
        assert_eq!(id, 0);

        let (id, _) = shim.route("150").unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn test_route_range_non_numeric_fails() {
        let mut shim = ShardingShim {
            strategy: ShardingStrategy::Range,
            shards: HashMap::new(),
            ..ShardingShim::new()
        };
        shim.shards.insert(
            0,
            ShardMapping {
                shard_id: 0,
                address: "shard-0:5432".to_string(),
                range_start: Some(0),
                range_end: Some(100),
                healthy: true,
            },
        );
        assert!(shim.route("abc").is_err());
    }

    #[test]
    fn test_route_directory() {
        let mut shim = ShardingShim {
            strategy: ShardingStrategy::Directory,
            shards: HashMap::new(),
            directory: HashMap::new(),
            ..ShardingShim::new()
        };
        shim.shards.insert(
            0,
            ShardMapping {
                shard_id: 0,
                address: "shard-0:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );
        shim.shards.insert(
            1,
            ShardMapping {
                shard_id: 1,
                address: "shard-1:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );
        shim.add_directory_mapping("tenant-a", 0);
        shim.add_directory_mapping("tenant-b", 1);

        let (id, _) = shim.route("tenant-a").unwrap();
        assert_eq!(id, 0);

        let (id, _) = shim.route("tenant-b").unwrap();
        assert_eq!(id, 1);
    }

    #[test]
    fn test_set_shard_health() {
        let mut shim = ShardingShim::new();
        shim.shards.insert(
            0,
            ShardMapping {
                shard_id: 0,
                address: "shard-0:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );

        assert!(shim.set_shard_health(0, false));
        assert!(!shim.get_shard(0).unwrap().healthy);
        assert!(!shim.set_shard_health(99, false));
    }

    #[test]
    fn test_healthy_shards() {
        let mut shim = ShardingShim::new();
        shim.shards.insert(
            0,
            ShardMapping {
                shard_id: 0,
                address: "shard-0:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );
        shim.shards.insert(
            1,
            ShardMapping {
                shard_id: 1,
                address: "shard-1:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: false,
            },
        );
        shim.shards.insert(
            2,
            ShardMapping {
                shard_id: 2,
                address: "shard-2:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );

        let healthy = shim.healthy_shards();
        assert_eq!(healthy.len(), 2);
    }

    #[test]
    fn test_hit_rate() {
        let shim = ShardingShim {
            queries_routed: 80,
            queries_missed: 20,
            ..ShardingShim::new()
        };
        assert!((shim.hit_rate() - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_set_range() {
        let mut shim = ShardingShim::new();
        shim.shards.insert(
            0,
            ShardMapping {
                shard_id: 0,
                address: "shard-0:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );

        shim.set_range(0, 0, 100).unwrap();
        let shard = shim.get_shard(0).unwrap();
        assert_eq!(shard.range_start, Some(0));
        assert_eq!(shard.range_end, Some(100));
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = ShardingShim::new();
        shim.queries_routed = 100;
        shim.queries_missed = 5;
        shim.shards.insert(
            0,
            ShardMapping {
                shard_id: 0,
                address: "shard-0:5432".to_string(),
                range_start: None,
                range_end: None,
                healthy: true,
            },
        );

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 5);
        assert_eq!(metrics[0].value, 100.0);
    }
}
