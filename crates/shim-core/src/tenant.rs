//! Multi-tenancy isolation for shims.
//!
//! Tracks per-tenant resource usage, enforces quotas, and returns
//! per-tenant metrics.
//!
//! Hardening measures:
//! - Token-bucket rate limiting (replaces simple counter)
//! - Memory budget enforcement with operation rejection
//! - Strict tenant-ID validation (alphanumeric + hyphens, 3-64 chars)
//! - Audit logging with timestamps
//! - Per-tenant CPU time tracking for runaway detection

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::config::{Config, TenantConfig};
use crate::Metric;

/// Strict tenant-ID pattern: alphanumeric and hyphens, 3-64 characters.
/// Prevents injection of special characters, path traversals, etc.
fn is_valid_tenant_id(id: &str) -> bool {
    let len = id.len();
    if !(3..=64).contains(&len) {
        return false;
    }
    id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Error returned when a tenant ID fails validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidTenantId(pub String);

impl std::fmt::Display for InvalidTenantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid tenant ID '{}': must be 3-64 ASCII alphanumeric/hyphen characters",
            self.0
        )
    }
}

impl std::error::Error for InvalidTenantId {}

/// Result of a hardened quota check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantQuotaResult {
    /// Operation is allowed.
    Allowed,
    /// Rate limit exceeded (token bucket exhausted).
    RateLimited,
    /// Memory budget exceeded.
    MemoryExceeded,
    /// CPU quota exceeded.
    CpuExceeded,
}

/// A single audit log entry for tenant operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO-8601 timestamp of the event.
    pub timestamp: String,
    /// Tenant that performed the action.
    pub tenant_id: String,
    /// Action description (e.g. "register", "request", "memory_record").
    pub action: String,
    /// Whether the operation was allowed.
    pub allowed: bool,
}

/// Token bucket for smooth rate limiting.
///
/// Tokens refill at `refill_rate` per second up to `max_tokens`.
/// A request consumes 1 token; if none are available the request is rejected.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new bucket with the given capacity and refill rate.
    pub fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time since last refill.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }

    /// Try to consume one token. Returns `true` if allowed.
    pub fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Number of tokens currently available.
    pub fn available(&self) -> f64 {
        self.tokens
    }

    /// Reset the bucket to full capacity.
    pub fn reset(&mut self) {
        self.tokens = self.max_tokens;
        self.last_refill = Instant::now();
    }
}

/// Per-CPU-time tracking for detecting runaway tenants.
#[derive(Debug, Clone, Default)]
pub struct CpuTimeTracker {
    /// Total wall-clock time consumed by this tenant (in milliseconds).
    pub total_cpu_ms: f64,
    /// Start of the current measurement window.
    pub window_start: Option<Instant>,
    /// Maximum allowed CPU time in milliseconds before the tenant is flagged.
    pub max_cpu_ms: Option<f64>,
}

impl CpuTimeTracker {
    /// Begin a CPU measurement window.
    pub fn start_window(&mut self) {
        self.window_start = Some(Instant::now());
    }

    /// End the current window and accumulate the elapsed time.
    /// Returns the elapsed milliseconds, or 0.0 if no window was active.
    pub fn end_window(&mut self) -> f64 {
        if let Some(start) = self.window_start.take() {
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            self.total_cpu_ms += elapsed;
            elapsed
        } else {
            0.0
        }
    }

    /// Whether the tenant has exceeded its CPU time budget.
    pub fn is_over_budget(&self) -> bool {
        self.max_cpu_ms
            .map(|max| self.total_cpu_ms > max)
            .unwrap_or(false)
    }

    /// Reset accumulated CPU time.
    pub fn reset(&mut self) {
        self.total_cpu_ms = 0.0;
        self.window_start = None;
    }
}

/// Per-tenant resource usage snapshot.
#[derive(Debug, Clone)]
pub struct TenantUsage {
    /// Memory usage in bytes.
    pub memory_bytes: u64,
    /// CPU usage as a percentage.
    pub cpu_percent: f64,
    /// Number of requests in the current period.
    pub requests_count: u64,
    /// When the request counter was last reset.
    pub last_reset: Instant,
    /// Token bucket for rate limiting (replaces simple counter).
    pub token_bucket: Option<TokenBucket>,
    /// CPU time tracker for detecting runaway tenants.
    pub cpu_time: CpuTimeTracker,
    /// Number of operations rejected due to quota limits.
    pub rejected_count: u64,
}

impl Serialize for TenantUsage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("TenantUsage", 5)?;
        state.serialize_field("memory_bytes", &self.memory_bytes)?;
        state.serialize_field("cpu_percent", &self.cpu_percent)?;
        state.serialize_field("requests_count", &self.requests_count)?;
        state.serialize_field(
            "token_bucket_available",
            &self.token_bucket.as_ref().map(|b| b.available()),
        )?;
        state.serialize_field("rejected_count", &self.rejected_count)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for TenantUsage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{MapAccess, Visitor};
        use std::fmt;

        struct TenantUsageVisitor;

        impl<'de> Visitor<'de> for TenantUsageVisitor {
            type Value = TenantUsage;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct TenantUsage")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TenantUsage, A::Error> {
                let mut memory_bytes = None;
                let mut cpu_percent = None;
                let mut requests_count = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "memory_bytes" => memory_bytes = Some(map.next_value()?),
                        "cpu_percent" => cpu_percent = Some(map.next_value()?),
                        "requests_count" => requests_count = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }

                Ok(TenantUsage {
                    memory_bytes: memory_bytes.unwrap_or_default(),
                    cpu_percent: cpu_percent.unwrap_or_default(),
                    requests_count: requests_count.unwrap_or_default(),
                    last_reset: Instant::now(),
                    token_bucket: None,
                    cpu_time: CpuTimeTracker::default(),
                    rejected_count: 0,
                })
            }
        }

        const FIELDS: &[&str] = &["memory_bytes", "cpu_percent", "requests_count"];
        deserializer.deserialize_struct("TenantUsage", FIELDS, TenantUsageVisitor)
    }
}

impl Default for TenantUsage {
    fn default() -> Self {
        Self {
            memory_bytes: 0,
            cpu_percent: 0.0,
            requests_count: 0,
            last_reset: Instant::now(),
            token_bucket: None,
            cpu_time: CpuTimeTracker::default(),
            rejected_count: 0,
        }
    }
}

/// Per-tenant metrics returned by the isolator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantMetrics {
    /// Unique tenant identifier.
    pub tenant_id: String,
    /// Current memory usage in bytes.
    pub memory_bytes: u64,
    /// Current CPU usage as a percentage.
    pub cpu_percent: f64,
    /// Number of requests in the current period.
    pub requests_count: u64,
    /// Whether the tenant has exceeded its memory limit.
    pub memory_limit_reached: bool,
    /// Whether the tenant has exceeded its CPU limit.
    pub cpu_limit_reached: bool,
    /// Whether the tenant has exceeded its rate limit.
    pub rate_limit_reached: bool,
    /// Number of rejected operations.
    pub rejected_count: u64,
    /// Total CPU time consumed in milliseconds.
    pub total_cpu_ms: f64,
}

/// Classify the rate limit status without consuming tokens.
/// Returns (result, needs_token_consume).
fn classify_rate(usage: &TenantUsage, config: &TenantConfig) -> (TenantQuotaResult, bool) {
    if usage.token_bucket.is_some() {
        (TenantQuotaResult::Allowed, true)
    } else if let Some(max_rps) = config.max_requests_per_sec {
        if usage.requests_count > max_rps as u64 {
            (TenantQuotaResult::RateLimited, false)
        } else {
            (TenantQuotaResult::Allowed, false)
        }
    } else {
        (TenantQuotaResult::Allowed, false)
    }
}

fn check_cpu_and_rate(usage: &mut TenantUsage, config: &TenantConfig) -> (TenantQuotaResult, bool) {
    let cpu_exceeded = usage.cpu_percent > config.max_cpu_percent.unwrap_or(f64::MAX)
        || usage.cpu_time.is_over_budget();
    if cpu_exceeded {
        usage.rejected_count += 1;
        (TenantQuotaResult::CpuExceeded, false)
    } else {
        classify_rate(usage, config)
    }
}

/// Tracks and enforces per-tenant resource quotas.
pub struct TenantIsolator {
    configs: HashMap<String, TenantConfig>,
    usage: HashMap<String, TenantUsage>,
    global_enabled: bool,
    audit_log: Vec<AuditEntry>,
    max_audit_entries: usize,
}

impl TenantIsolator {
    /// Create from the main [`Config`].
    ///
    /// Returns an error if any tenant ID is invalid.
    pub fn from_config(config: &Config) -> Result<Self, InvalidTenantId> {
        let mut configs = HashMap::new();
        let mut usage = HashMap::new();
        for tenant in &config.tenants {
            if !is_valid_tenant_id(&tenant.tenant_id) {
                return Err(InvalidTenantId(tenant.tenant_id.clone()));
            }
            configs.insert(tenant.tenant_id.clone(), tenant.clone());
            let mut tenant_usage = TenantUsage::default();
            if let Some(max_rps) = tenant.max_requests_per_sec {
                tenant_usage.token_bucket = Some(TokenBucket::new(max_rps as f64, max_rps as f64));
            }
            usage.insert(tenant.tenant_id.clone(), tenant_usage);
        }
        Ok(Self {
            configs,
            usage,
            global_enabled: !config.tenants.is_empty(),
            audit_log: Vec::new(),
            max_audit_entries: 10_000,
        })
    }

    /// Create an empty isolator (no tenants configured).
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            usage: HashMap::new(),
            global_enabled: false,
            audit_log: Vec::new(),
            max_audit_entries: 10_000,
        }
    }

    /// Set the maximum number of audit log entries retained.
    pub fn set_max_audit_entries(&mut self, max: usize) {
        self.max_audit_entries = max;
    }

    /// Validate a tenant ID against the strict pattern.
    pub fn validate_tenant_id(id: &str) -> Result<(), InvalidTenantId> {
        if is_valid_tenant_id(id) {
            Ok(())
        } else {
            Err(InvalidTenantId(id.to_string()))
        }
    }

    /// Register a tenant at runtime.
    ///
    /// Returns `Err` if the tenant ID is invalid.
    pub fn register_tenant(&mut self, config: TenantConfig) -> Result<(), InvalidTenantId> {
        if !is_valid_tenant_id(&config.tenant_id) {
            return Err(InvalidTenantId(config.tenant_id.clone()));
        }
        let tenant_id = config.tenant_id.clone();
        self.configs.insert(tenant_id.clone(), config);
        self.global_enabled = true;

        if let Some(max_rps) = self
            .configs
            .get(&tenant_id)
            .and_then(|c| c.max_requests_per_sec)
        {
            let usage = self.usage.entry(tenant_id.clone()).or_default();
            usage.token_bucket = Some(TokenBucket::new(max_rps as f64, max_rps as f64));
        }

        self.record_audit_entry(&tenant_id, "register", true);
        Ok(())
    }

    /// Remove a tenant.
    pub fn remove_tenant(&mut self, tenant_id: &str) -> bool {
        self.usage.remove(tenant_id);
        let removed = self.configs.remove(tenant_id).is_some();
        if removed {
            self.record_audit_entry(tenant_id, "remove", true);
        }
        removed
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

    /// Record a request for a tenant (legacy counter, kept for backward compat).
    pub fn record_request(&mut self, tenant_id: &str) {
        self.usage
            .entry(tenant_id.to_string())
            .or_default()
            .requests_count += 1;
    }

    /// Reset the request counter if the configured period has elapsed.
    pub fn check_and_reset(&mut self, tenant_id: &str) {
        let reset_period = self
            .configs
            .get(tenant_id)
            .map(|c| c.reset_period_secs)
            .unwrap_or(1);

        if let Some(usage) = self.usage.get_mut(tenant_id) {
            if usage.last_reset.elapsed().as_secs() >= reset_period {
                usage.requests_count = 0;
                usage.last_reset = Instant::now();
                if let Some(ref mut bucket) = usage.token_bucket {
                    bucket.reset();
                }
            }
        }
    }

    /// Check if a request is allowed under the tenant's quota.
    /// Returns `true` if allowed, `false` if quota exceeded.
    ///
    /// This is the legacy API; prefer [`check_quota_hardened`] for full details.
    pub fn check_quota(&mut self, tenant_id: &str) -> bool {
        matches!(
            self.check_quota_hardened(tenant_id),
            TenantQuotaResult::Allowed
        )
    }

    /// Hardened quota check that returns a detailed result.
    ///
    /// Uses the token bucket for rate limiting when available, falls back
    /// to the legacy counter otherwise.
    pub fn check_quota_hardened(&mut self, tenant_id: &str) -> TenantQuotaResult {
        if !self.global_enabled {
            return TenantQuotaResult::Allowed;
        }

        let config = match self.configs.get(tenant_id) {
            Some(c) => c.clone(),
            None => return TenantQuotaResult::Allowed,
        };

        // Ensure token bucket exists for tenants with a rate limit
        {
            let usage = self.usage.entry(tenant_id.to_string()).or_default();
            if usage.token_bucket.is_none() {
                if let Some(max_rps) = config.max_requests_per_sec {
                    usage.token_bucket = Some(TokenBucket::new(max_rps as f64, max_rps as f64));
                }
            }
        }

        let (result, needs_token_consume): (TenantQuotaResult, bool) = {
            let usage = self.usage.entry(tenant_id.to_string()).or_default();

            let reset_period = config.reset_period_secs;
            if usage.last_reset.elapsed().as_secs() >= reset_period {
                usage.requests_count = 0;
                usage.last_reset = Instant::now();
                if let Some(ref mut bucket) = usage.token_bucket {
                    bucket.reset();
                }
            }

            if let Some(max_mem) = config.max_memory_bytes {
                if usage.memory_bytes > max_mem {
                    usage.rejected_count += 1;
                    (TenantQuotaResult::MemoryExceeded, false)
                } else {
                    check_cpu_and_rate(usage, &config)
                }
            } else {
                check_cpu_and_rate(usage, &config)
            }
        };

        if needs_token_consume {
            let usage = match self.usage.get_mut(tenant_id) {
                Some(u) => u,
                None => {
                    self.record_audit_entry(tenant_id, "quota_check", false);
                    return TenantQuotaResult::RateLimited;
                }
            };
            if let Some(ref mut bucket) = usage.token_bucket {
                if !bucket.try_consume() {
                    usage.rejected_count += 1;
                    self.record_audit_entry(tenant_id, "quota_check", false);
                    return TenantQuotaResult::RateLimited;
                }
            }
        }

        let allowed = matches!(result, TenantQuotaResult::Allowed);
        self.record_audit_entry(tenant_id, "quota_check", allowed);
        result
    }

    /// Check if a feature is allowed for a tenant.
    pub fn is_feature_allowed(&self, tenant_id: &str, feature: &str) -> bool {
        let config = match self.configs.get(tenant_id) {
            Some(c) => c,
            None => return true,
        };
        config.allowed_features.iter().any(|f| f == feature)
    }

    /// Start a CPU time measurement window for a tenant.
    pub fn start_cpu_window(&mut self, tenant_id: &str) {
        self.usage
            .entry(tenant_id.to_string())
            .or_default()
            .cpu_time
            .start_window();
    }

    /// End the CPU time measurement window and return elapsed milliseconds.
    pub fn end_cpu_window(&mut self, tenant_id: &str) -> f64 {
        self.usage
            .entry(tenant_id.to_string())
            .or_default()
            .cpu_time
            .end_window()
    }

    /// Get the audit log entries.
    pub fn audit_log(&self) -> &[AuditEntry] {
        &self.audit_log
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

                let rate_limit_reached = if let Some(ref bucket) = usage.token_bucket {
                    bucket.available() < 1.0
                } else {
                    config
                        .max_requests_per_sec
                        .map(|max| usage.requests_count > max as u64)
                        .unwrap_or(false)
                };

                TenantMetrics {
                    tenant_id: config.tenant_id.clone(),
                    memory_bytes: usage.memory_bytes,
                    cpu_percent: usage.cpu_percent,
                    requests_count: usage.requests_count,
                    memory_limit_reached,
                    cpu_limit_reached,
                    rate_limit_reached,
                    rejected_count: usage.rejected_count,
                    total_cpu_ms: usage.cpu_time.total_cpu_ms,
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
                labels.clone(),
            ));
            metrics.push(Metric::with_labels(
                "tenant_rejected_count",
                tm.rejected_count as f64,
                labels.clone(),
            ));
            metrics.push(Metric::with_labels(
                "tenant_total_cpu_ms",
                tm.total_cpu_ms,
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

    /// Append an audit entry, evicting oldest if at capacity.
    fn record_audit_entry(&mut self, tenant_id: &str, action: &str, allowed: bool) {
        let entry = AuditEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            tenant_id: tenant_id.to_string(),
            action: action.to_string(),
            allowed,
        };
        if self.audit_log.len() >= self.max_audit_entries {
            self.audit_log.remove(0);
        }
        self.audit_log.push(entry);
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
                    reset_period_secs: 1,
                },
                TenantConfig {
                    tenant_id: "tenant-b".into(),
                    max_memory_bytes: Some(2 * 1024 * 1024),
                    max_cpu_percent: None,
                    max_requests_per_sec: None,
                    allowed_features: vec![],
                    quota_config: ResourceQuota::default(),
                    reset_period_secs: 1,
                },
            ],
            ..Config::default()
        }
    }

    #[test]
    fn test_from_config() {
        let config = test_config();
        let isolator = TenantIsolator::from_config(&config).unwrap();
        assert!(isolator.is_enabled());
        assert_eq!(isolator.tenant_count(), 2);
    }

    #[test]
    fn test_from_config_invalid_tenant_id() {
        let mut config = test_config();
        config.tenants.push(TenantConfig {
            tenant_id: "a".into(),
            max_memory_bytes: None,
            max_cpu_percent: None,
            max_requests_per_sec: None,
            allowed_features: vec![],
            quota_config: ResourceQuota::default(),
            reset_period_secs: 1,
        });
        match TenantIsolator::from_config(&config) {
            Err(InvalidTenantId(id)) => assert_eq!(id, "a"),
            Ok(_) => panic!("expected error for invalid tenant ID"),
        }
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

        isolator
            .register_tenant(TenantConfig {
                tenant_id: "new-tenant".into(),
                max_memory_bytes: None,
                max_cpu_percent: None,
                max_requests_per_sec: None,
                allowed_features: vec![],
                quota_config: ResourceQuota::default(),
                reset_period_secs: 1,
            })
            .unwrap();
        assert!(isolator.is_enabled());
        assert_eq!(isolator.tenant_count(), 1);
    }

    #[test]
    fn test_register_tenant_invalid_id() {
        let mut isolator = TenantIsolator::new();
        let result = isolator.register_tenant(TenantConfig {
            tenant_id: "x".into(),
            max_memory_bytes: None,
            max_cpu_percent: None,
            max_requests_per_sec: None,
            allowed_features: vec![],
            quota_config: ResourceQuota::default(),
            reset_period_secs: 1,
        });
        assert!(result.is_err());
        assert!(!isolator.is_enabled());
    }

    #[test]
    fn test_remove_tenant() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        assert!(isolator.remove_tenant("tenant-a"));
        assert_eq!(isolator.tenant_count(), 1);
        assert!(!isolator.remove_tenant("nonexistent"));
    }

    #[test]
    fn test_check_quota_within_limits() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-a", 512 * 1024);
        isolator.record_cpu("tenant-a", 25.0);
        isolator.record_request("tenant-a");
        assert!(isolator.check_quota("tenant-a"));
    }

    #[test]
    fn test_check_quota_memory_exceeded() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-a", 2 * 1024 * 1024);
        assert!(!isolator.check_quota("tenant-a"));
    }

    #[test]
    fn test_check_quota_cpu_exceeded() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_cpu("tenant-a", 75.0);
        assert!(!isolator.check_quota("tenant-a"));
    }

    #[test]
    fn test_check_quota_rate_exceeded() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        for _ in 0..100 {
            isolator.check_quota_hardened("tenant-a");
        }
        assert_eq!(
            isolator.check_quota_hardened("tenant-a"),
            TenantQuotaResult::RateLimited
        );
    }

    #[test]
    fn test_check_quota_no_limits() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-b", 1024);
        isolator.record_cpu("tenant-b", 999.0);
        assert!(isolator.check_quota("tenant-b"));
    }

    #[test]
    fn test_check_quota_hardened_results() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();

        isolator.record_memory("tenant-a", 512 * 1024);
        isolator.record_cpu("tenant-a", 25.0);
        assert_eq!(
            isolator.check_quota_hardened("tenant-a"),
            TenantQuotaResult::Allowed
        );

        isolator.record_memory("tenant-a", 2 * 1024 * 1024);
        assert_eq!(
            isolator.check_quota_hardened("tenant-a"),
            TenantQuotaResult::MemoryExceeded
        );
    }

    #[test]
    fn test_is_feature_allowed() {
        let config = test_config();
        let isolator = TenantIsolator::from_config(&config).unwrap();
        assert!(isolator.is_feature_allowed("tenant-a", "feature-x"));
        assert!(isolator.is_feature_allowed("tenant-a", "feature-y"));
        assert!(!isolator.is_feature_allowed("tenant-a", "feature-z"));
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
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
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
        assert_eq!(tm_a.rejected_count, 0);
    }

    #[test]
    fn test_shim_metrics() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-a", 1024);

        let shim_metrics = isolator.shim_metrics();
        assert_eq!(shim_metrics.len(), 16);
    }

    #[test]
    fn test_get_config_and_usage() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-a", 42);

        assert!(isolator.get_config("tenant-a").is_some());
        assert!(isolator.get_config("unknown").is_none());
        assert_eq!(isolator.get_usage("tenant-a").unwrap().memory_bytes, 42);
        assert!(isolator.get_usage("unknown").is_none());
    }

    #[tokio::test]
    async fn test_lifecycle() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();

        isolator.record_memory("tenant-a", 1024);
        isolator.record_cpu("tenant-a", 10.0);
        isolator.record_request("tenant-a");

        isolator.record_memory("tenant-b", 2048);
        isolator.record_cpu("tenant-b", 30.0);

        assert!(isolator.check_quota("tenant-a"));
        assert!(isolator.check_quota("tenant-b"));

        assert!(isolator.remove_tenant("tenant-a"));
        assert_eq!(isolator.tenant_count(), 1);
        assert!(isolator.get_usage("tenant-a").is_none());
    }

    // --- Token bucket tests ---

    #[test]
    fn test_token_bucket_consume() {
        let mut bucket = TokenBucket::new(10.0, 5.0);
        assert!(bucket.try_consume());
        assert_eq!(bucket.available(), 9.0);
    }

    #[test]
    fn test_token_bucket_exhaustion() {
        let mut bucket = TokenBucket::new(3.0, 1.0);
        assert!(bucket.try_consume());
        assert!(bucket.try_consume());
        assert!(bucket.try_consume());
        assert!(!bucket.try_consume());
    }

    #[test]
    fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(2.0, 1.0);
        assert!(bucket.try_consume());
        assert!(bucket.try_consume());
        assert!(!bucket.try_consume());

        bucket.last_refill = Instant::now() - std::time::Duration::from_secs(1);
        assert!(bucket.try_consume());
    }

    #[test]
    fn test_token_bucket_reset() {
        let mut bucket = TokenBucket::new(2.0, 1.0);
        bucket.try_consume();
        bucket.try_consume();
        assert!(!bucket.try_consume());
        bucket.reset();
        assert!(bucket.try_consume());
    }

    #[test]
    fn test_token_bucket_no_burst_above_max() {
        let mut bucket = TokenBucket::new(5.0, 10.0);
        bucket.last_refill = Instant::now() - std::time::Duration::from_secs(100);
        bucket.refill();
        assert_eq!(bucket.available(), 5.0);
    }

    // --- Tenant ID validation tests ---

    #[test]
    fn test_valid_tenant_ids() {
        assert!(is_valid_tenant_id("abc"));
        assert!(is_valid_tenant_id("tenant-1"));
        assert!(is_valid_tenant_id("my-tenant-abc-123"));
        assert!(is_valid_tenant_id("ABC-123"));
        assert!(is_valid_tenant_id("a".repeat(64).as_str()));
    }

    #[test]
    fn test_invalid_tenant_ids() {
        assert!(!is_valid_tenant_id("ab"));
        assert!(!is_valid_tenant_id(&"a".repeat(65)));
        assert!(!is_valid_tenant_id("tenant_id"));
        assert!(!is_valid_tenant_id("tenant id"));
        assert!(!is_valid_tenant_id("tenant.id"));
        assert!(!is_valid_tenant_id("tenant/id"));
        assert!(!is_valid_tenant_id("tenant@id"));
        assert!(!is_valid_tenant_id(""));
    }

    #[test]
    fn test_validate_tenant_id_method() {
        assert!(TenantIsolator::validate_tenant_id("valid-id").is_ok());
        assert!(TenantIsolator::validate_tenant_id("x").is_err());
    }

    // --- Audit log tests ---

    #[test]
    fn test_audit_log_records_operations() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-a", 1024);

        isolator.record_memory("tenant-a", 2 * 1024 * 1024);
        isolator.check_quota("tenant-a");

        let log = isolator.audit_log();
        assert!(!log.is_empty());
        let failed_checks: Vec<_> = log
            .iter()
            .filter(|e| e.action == "quota_check" && !e.allowed)
            .collect();
        assert!(!failed_checks.is_empty());
        assert_eq!(failed_checks[0].tenant_id, "tenant-a");
    }

    #[test]
    fn test_audit_log_max_entries() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.set_max_audit_entries(5);

        for _ in 0..10 {
            isolator.record_memory("tenant-a", 2 * 1024 * 1024);
            isolator.check_quota("tenant-a");
        }

        assert!(isolator.audit_log().len() <= 5);
    }

    #[test]
    fn test_audit_entry_timestamp_format() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-a", 2 * 1024 * 1024);
        isolator.check_quota("tenant-a");

        let log = isolator.audit_log();
        let entry = log.iter().find(|e| e.action == "quota_check").unwrap();
        assert!(entry.timestamp.contains('T'));
        assert!(entry.timestamp.ends_with("Z") || entry.timestamp.contains('+'));
    }

    // --- CPU time tracking tests ---

    #[test]
    fn test_cpu_time_tracker_basic() {
        let mut tracker = CpuTimeTracker {
            total_cpu_ms: 0.0,
            window_start: None,
            max_cpu_ms: Some(100.0),
        };
        assert!(!tracker.is_over_budget());

        tracker.start_window();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = tracker.end_window();
        assert!(elapsed >= 5.0);
        assert!(!tracker.is_over_budget());
    }

    #[test]
    fn test_cpu_time_tracker_over_budget() {
        let mut tracker = CpuTimeTracker {
            total_cpu_ms: 0.0,
            window_start: None,
            max_cpu_ms: Some(0.001),
        };
        tracker.start_window();
        std::thread::sleep(std::time::Duration::from_millis(5));
        tracker.end_window();
        assert!(tracker.is_over_budget());
    }

    #[test]
    fn test_cpu_time_tracker_reset() {
        let mut tracker = CpuTimeTracker {
            total_cpu_ms: 50.0,
            window_start: None,
            max_cpu_ms: Some(10.0),
        };
        assert!(tracker.is_over_budget());
        tracker.reset();
        assert!(!tracker.is_over_budget());
        assert_eq!(tracker.total_cpu_ms, 0.0);
    }

    #[test]
    fn test_cpu_time_tracking_via_isolator() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();

        isolator.start_cpu_window("tenant-a");
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = isolator.end_cpu_window("tenant-a");
        assert!(elapsed >= 5.0);

        let usage = isolator.get_usage("tenant-a").unwrap();
        assert!(usage.cpu_time.total_cpu_ms >= 5.0);
    }

    #[test]
    fn test_rejected_count_increments() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-a", 2 * 1024 * 1024);

        for _ in 0..5 {
            isolator.check_quota("tenant-a");
        }

        let usage = isolator.get_usage("tenant-a").unwrap();
        assert_eq!(usage.rejected_count, 5);
    }

    #[test]
    fn test_token_bucket_with_isolator() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();

        let usage = isolator.get_usage("tenant-a").unwrap();
        assert!(usage.token_bucket.is_some());

        for _ in 0..100 {
            assert_eq!(
                isolator.check_quota_hardened("tenant-a"),
                TenantQuotaResult::Allowed
            );
        }
        assert_eq!(
            isolator.check_quota_hardened("tenant-a"),
            TenantQuotaResult::RateLimited
        );
    }

    #[test]
    fn test_metrics_includes_new_fields() {
        let config = test_config();
        let mut isolator = TenantIsolator::from_config(&config).unwrap();
        isolator.record_memory("tenant-a", 2 * 1024 * 1024);
        isolator.check_quota("tenant-a");

        let metrics = isolator.metrics();
        let tm_a = metrics.iter().find(|m| m.tenant_id == "tenant-a").unwrap();
        assert_eq!(tm_a.rejected_count, 1);
        assert!(tm_a.memory_limit_reached);
    }
}
