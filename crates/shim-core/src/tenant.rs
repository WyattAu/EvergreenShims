//! Multi-tenancy isolation for shims.
//!
//! Tracks per-tenant resource usage, enforces quotas, and returns
//! per-tenant metrics.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::{Config, TenantConfig};
use crate::Metric;

/// Per-tenant resource usage snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TenantUsage {
    pub memory_bytes: u64,
    pub cpu_percent: f64,
    pub requests_count: u64,
}

/// Per-tenant metrics returned by the isolator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantMetrics {
    pub tenant_id: String,
    pub memory_bytes: u64,
    pub cpu_percent: f64,
    pub requests_count: u64,
    pub memory_limit_reached: bool,
    pub cpu_limit_reached: bool,
    pub rate_limit_reached: bool,
}

/// Tracks and enforces per-tenant resource quotas.
pub struct TenantIsolator {
    configs: HashMap<String, TenantConfig>,
    usage: HashMap<String, TenantUsage>,
    global_enabled: bool,
}

impl TenantIsolator {
    /// Create from the main [`Config`].
    pub fn from_config(config: &Config) -> Self {
        let mut configs = HashMap::new();
        for tenant in &config.tenants {
            configs.insert(tenant.tenant_id.clone(), tenant.clone());
        }
        Self {
            configs,
            usage: HashMap::new(),
            global_enabled: !config.tenants.is_empty(),
        }
    }

    /// Create an empty isolator (no tenants configured).
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            usage: HashMap::new(),
            global_enabled: false,
        }
    }

    /// Register a tenant at runtime.
    pub fn register_tenant(&mut self, config: TenantConfig) {
        self.configs.insert(config.tenant_id.clone(), config);
        self.global_enabled = true;
    }

    /// Remove a tenant.
    pub fn remove_tenant(&mut self, tenant_id: &str) -> bool {
        self.usage.remove(tenant_id);
        self.configs.remove(tenant_id).is_some()
    }

    /// Record memory usage for a tenant.
    pub fn record_memory(&mut self, tenant_id: &str, bytes: u64) {
        self.usage
            .entry(tenant_id.to_string())
            .or_default()
            .memory_bytes = bytes;
    }

    /// Record CPU usage for a tenant.
    pub fn record_cpu(&mut self, tenant_id: &str, percent: f64) {
        self.usage
            .entry(tenant_id.to_string())
            .or_default()
            .cpu_percent = percent;
    }

    /// Record a request for a tenant.
    pub fn record_request(&mut self, tenant_id: &str) {
        self.usage
            .entry(tenant_id.to_string())
            .or_default()
            .requests_count += 1;
    }

    /// Check if a request is allowed under the tenant's quota.
    /// Returns `true` if allowed, `false` if quota exceeded.
    pub fn check_quota(&self, tenant_id: &str) -> bool {
        if !self.global_enabled {
            return true;
        }

        let config = match self.configs.get(tenant_id) {
            Some(c) => c,
            None => return true,
        };

        if let Some(usage) = self.usage.get(tenant_id) {
            if let Some(max_mem) = config.max_memory_bytes {
                if usage.memory_bytes > max_mem {
                    return false;
                }
            }
            if let Some(max_cpu) = config.max_cpu_percent {
                if usage.cpu_percent > max_cpu {
                    return false;
                }
            }
            if let Some(max_rps) = config.max_requests_per_sec {
                // Simple per-interval check: if requests_count exceeds rate limit
                // we treat it as exceeded (the caller should reset counters periodically).
                if usage.requests_count > max_rps as u64 {
                    return false;
                }
            }
        }

        true
    }

    /// Check if a feature is allowed for a tenant.
    pub fn is_feature_allowed(&self, tenant_id: &str, feature: &str) -> bool {
        let config = match self.configs.get(tenant_id) {
            Some(c) => c,
            None => return true,
        };
        config.allowed_features.iter().any(|f| f == feature)
    }

    /// Get metrics for all tenants.
    pub fn metrics(&self) -> Vec<TenantMetrics> {
        self.configs
            .values()
            .map(|config| {
                let usage = self
                    .usage
                    .get(&config.tenant_id)
                    .cloned()
                    .unwrap_or_default();

                let memory_limit_reached = config
                    .max_memory_bytes
                    .map(|max| usage.memory_bytes > max)
                    .unwrap_or(false);

                let cpu_limit_reached = config
                    .max_cpu_percent
                    .map(|max| usage.cpu_percent > max)
                    .unwrap_or(false);

                let rate_limit_reached = config
                    .max_requests_per_sec
                    .map(|max| usage.requests_count > max as u64)
                    .unwrap_or(false);

                TenantMetrics {
                    tenant_id: config.tenant_id.clone(),
                    memory_bytes: usage.memory_bytes,
                    cpu_percent: usage.cpu_percent,
                    requests_count: usage.requests_count,
                    memory_limit_reached,
                    cpu_limit_reached,
                    rate_limit_reached,
                }
            })
            .collect()
    }

    /// Convert tenant metrics into shim [`Metric`] values.
    pub fn shim_metrics(&self) -> Vec<Metric> {
        let mut metrics = Vec::new();
        for tm in self.metrics() {
            let labels: HashMap<String, String> =
                [("tenant_id".into(), tm.tenant_id.clone())].into();
            metrics.push(Metric::with_labels(
                "tenant_memory_bytes",
                tm.memory_bytes as f64,
                labels.clone(),
            ));
            metrics.push(Metric::with_labels(
                "tenant_cpu_percent",
                tm.cpu_percent,
                labels.clone(),
            ));
            metrics.push(Metric::with_labels(
                "tenant_requests_count",
                tm.requests_count as f64,
                labels.clone(),
            ));
            metrics.push(Metric::with_labels(
                "tenant_memory_limit_reached",
                if tm.memory_limit_reached { 1.0 } else { 0.0 },
                labels.clone(),
            ));
            metrics.push(Metric::with_labels(
                "tenant_cpu_limit_reached",
                if tm.cpu_limit_reached { 1.0 } else { 0.0 },
                labels.clone(),
            ));
            metrics.push(Metric::with_labels(
                "tenant_rate_limit_reached",
                if tm.rate_limit_reached { 1.0 } else { 0.0 },
                labels,
            ));
        }
        metrics
    }

    /// Get tenant config.
    pub fn get_config(&self, tenant_id: &str) -> Option<&TenantConfig> {
        self.configs.get(tenant_id)
    }

    /// Get tenant usage.
    pub fn get_usage(&self, tenant_id: &str) -> Option<&TenantUsage> {
        self.usage.get(tenant_id)
    }

    /// Check if multi-tenancy is enabled.
    pub fn is_enabled(&self) -> bool {
        self.global_enabled
    }

    /// Get tenant count.
    pub fn tenant_count(&self) -> usize {
        self.configs.len()
    }
}

impl Default for TenantIsolator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResourceQuota;

    fn test_config() -> Config {
        Config {
            tenants: vec![
                TenantConfig {
                    tenant_id: "tenant-a".into(),
                    max_memory_bytes: Some(1024 * 1024),
                    max_cpu_percent: Some(50.0),
                    max_requests_per_sec: Some(100),
                    allowed_features: vec!["feature-x".into(), "feature-y".into()],
                    quota_config: ResourceQuota::default(),
                },
                TenantConfig {
                    tenant_id: "tenant-b".into(),
                    max_memory_bytes: Some(2 * 1024 * 1024),
                    max_cpu_percent: None,
                    max_requests_per_sec: None,
                    allowed_features: vec![],
                    quota_config: ResourceQuota::default(),
                },
            ],
            ..Config::default()
        }
    }

    #[test]
    fn test_from_config() {
        let config = test_config();
        let isolator = TenantIsolator::from_config(&config);
        assert!(isolator.is_enabled());
        assert_eq!(isolator.tenant_count(), 2);
    }

    #[test]
    fn test_new_not_enabled() {
        let isolator = TenantIsolator::new();
        assert!(!isolator.is_enabled());
    }

    #[test]
    fn test_register_tenant() {
        let mut isolator = TenantIsolator::new();
        assert!(!isolator.is_enabled());

        isolator.register_tenant(TenantConfig {
            tenant_id: "new-tenant".into(),
            max_memory_bytes: None,
            max_cpu_percent: None,
            max_requests_per_sec: None,
            allowed_features: vec![],
            quota_config: ResourceQuota::default(),
        });
        assert!(isolator.is_enabled());
        assert_eq!(isolator.tenant_count(), 1);
    }

    #[test]
    fn test_remove_tenant() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        assert!(isolator.remove_tenant("tenant-a"));
        assert_eq!(isolator.tenant_count(), 1);
        assert!(!isolator.remove_tenant("nonexistent"));
    }

    #[test]
    fn test_check_quota_within_limits() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        isolator.record_memory("tenant-a", 512 * 1024);
        isolator.record_cpu("tenant-a", 25.0);
        isolator.record_request("tenant-a");
        assert!(isolator.check_quota("tenant-a"));
    }

    #[test]
    fn test_check_quota_memory_exceeded() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        isolator.record_memory("tenant-a", 2 * 1024 * 1024); // exceeds 1MB limit
        assert!(!isolator.check_quota("tenant-a"));
    }

    #[test]
    fn test_check_quota_cpu_exceeded() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        isolator.record_cpu("tenant-a", 75.0); // exceeds 50% limit
        assert!(!isolator.check_quota("tenant-a"));
    }

    #[test]
    fn test_check_quota_rate_exceeded() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        for _ in 0..101 {
            isolator.record_request("tenant-a");
        }
        assert!(!isolator.check_quota("tenant-a"));
    }

    #[test]
    fn test_check_quota_no_limits() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        // tenant-b has max_memory_bytes: Some(2MB) but no CPU or rate limit
        isolator.record_memory("tenant-b", 1024); // well under 2MB
        isolator.record_cpu("tenant-b", 999.0); // no CPU limit
        for _ in 0..10000 {
            isolator.record_request("tenant-b"); // no rate limit
        }
        assert!(isolator.check_quota("tenant-b"));
    }

    #[test]
    fn test_is_feature_allowed() {
        let config = test_config();
        let isolator = TenantIsolator::from_config(&config);
        assert!(isolator.is_feature_allowed("tenant-a", "feature-x"));
        assert!(isolator.is_feature_allowed("tenant-a", "feature-y"));
        assert!(!isolator.is_feature_allowed("tenant-a", "feature-z"));
        // tenant-b has no allowed features
        assert!(!isolator.is_feature_allowed("tenant-b", "feature-x"));
    }

    #[test]
    fn test_is_feature_allowed_unknown_tenant() {
        let isolator = TenantIsolator::new();
        assert!(isolator.is_feature_allowed("unknown", "anything"));
    }

    #[test]
    fn test_metrics() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        isolator.record_memory("tenant-a", 1024);
        isolator.record_cpu("tenant-a", 10.0);
        isolator.record_request("tenant-a");

        let metrics = isolator.metrics();
        assert_eq!(metrics.len(), 2);

        let tm_a = metrics.iter().find(|m| m.tenant_id == "tenant-a").unwrap();
        assert_eq!(tm_a.memory_bytes, 1024);
        assert!((tm_a.cpu_percent - 10.0).abs() < 0.01);
        assert_eq!(tm_a.requests_count, 1);
        assert!(!tm_a.memory_limit_reached);
        assert!(!tm_a.cpu_limit_reached);
    }

    #[test]
    fn test_shim_metrics() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        isolator.record_memory("tenant-a", 1024);

        let shim_metrics = isolator.shim_metrics();
        assert_eq!(shim_metrics.len(), 12); // 6 per tenant * 2 tenants
    }

    #[test]
    fn test_get_config_and_usage() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);
        isolator.record_memory("tenant-a", 42);

        assert!(isolator.get_config("tenant-a").is_some());
        assert!(isolator.get_config("unknown").is_none());
        assert_eq!(isolator.get_usage("tenant-a").unwrap().memory_bytes, 42);
        assert!(isolator.get_usage("unknown").is_none());
    }

    #[tokio::test]
    async fn test_lifecycle() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config);

        // Record usage across tenants
        isolator.record_memory("tenant-a", 1024);
        isolator.record_cpu("tenant-a", 10.0);
        isolator.record_request("tenant-a");

        isolator.record_memory("tenant-b", 2048);
        isolator.record_cpu("tenant-b", 30.0);

        // Both should pass
        assert!(isolator.check_quota("tenant-a"));
        assert!(isolator.check_quota("tenant-b"));

        // Evict tenant-a
        assert!(isolator.remove_tenant("tenant-a"));
        assert_eq!(isolator.tenant_count(), 1);
        // Usage cleared
        assert!(isolator.get_usage("tenant-a").is_none());
    }
}
