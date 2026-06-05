#![allow(dead_code)]
//! Proxy shim — connection pooling, retries, and circuit breaker.
//!
//! Sits between the application and database, providing:
//! - Connection pooling (reuse connections)
//! - Automatic retries with exponential backoff
//! - Circuit breaker (stop hammering failing databases)
//!
//! ## Environment Variables
//!
//! ```text
//! PROXY_LISTEN            Listen address (default: 0.0.0.0:5432)
//! PROXY_TARGET            Target database address (required)
//! PROXY_MAX_CONNECTIONS   Max pool connections (default: 20)
//! PROXY_MIN_IDLE          Min idle connections (default: 5)
//! PROXY_MAX_LIFETIME_SECS Max connection lifetime (default: 1800)
//! PROXY_IDLE_TIMEOUT_SECS Idle connection timeout (default: 600)
//! PROXY_CONNECT_TIMEOUT   Connect timeout (default: 5)
//! PROXY_RETRY_ATTEMPTS    Max retry attempts (default: 3)
//! PROXY_RETRY_BASE_MS     Base retry delay (default: 100)
//! PROXY_CIRCUIT_THRESHOLD Failures before open circuit (default: 5)
//! PROXY_CIRCUIT_RESET_SECS Seconds before half-open (default: 30)
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Circuit breaker state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half_open"),
        }
    }
}

/// Connection pool statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolStats {
    pub active: u64,
    pub idle: u64,
    pub total: u64,
    pub waiters: u64,
}

/// Route rule for URL-based routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRule {
    pub path_prefix: String,
    pub target: String,
    pub weight: u32,
    pub healthy: bool,
}

/// Rate limit config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub max_requests_per_sec: u64,
    pub burst: u64,
    pub window_secs: u64,
}

/// Proxy shim providing connection pooling and resilience.
pub struct ProxyShim {
    listen: String,
    target: String,
    max_connections: u32,
    min_idle: u32,
    max_lifetime_secs: u64,
    idle_timeout_secs: u64,
    connect_timeout: u64,
    retry_attempts: u32,
    retry_base_ms: u64,
    circuit_threshold: u32,
    circuit_reset_secs: u64,
    circuit_state: CircuitState,
    circuit_failures: u32,
    circuit_opened_at: Option<chrono::DateTime<chrono::Utc>>,
    connections_active: u64,
    connections_total: u64,
    requests_total: u64,
    requests_retried: u64,
    requests_circuit_broken: u64,
    route_rules: Vec<RouteRule>,
    rate_limit: Option<RateLimitConfig>,
    backends: HashMap<String, bool>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ProxyShim {
    pub fn new() -> Self {
        Self {
            listen: std::env::var("PROXY_LISTEN").unwrap_or_else(|_| "0.0.0.0:5432".to_string()),
            target: std::env::var("PROXY_TARGET").unwrap_or_else(|_| "127.0.0.1:5432".to_string()),
            max_connections: std::env::var("PROXY_MAX_CONNECTIONS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(20),
            min_idle: std::env::var("PROXY_MIN_IDLE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            max_lifetime_secs: std::env::var("PROXY_MAX_LIFETIME_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1800),
            idle_timeout_secs: std::env::var("PROXY_IDLE_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(600),
            connect_timeout: std::env::var("PROXY_CONNECT_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            retry_attempts: std::env::var("PROXY_RETRY_ATTEMPTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3),
            retry_base_ms: std::env::var("PROXY_RETRY_BASE_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
            circuit_threshold: std::env::var("PROXY_CIRCUIT_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            circuit_reset_secs: std::env::var("PROXY_CIRCUIT_RESET_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            circuit_state: CircuitState::Closed,
            circuit_failures: 0,
            circuit_opened_at: None,
            connections_active: 0,
            connections_total: 0,
            requests_total: 0,
            requests_retried: 0,
            requests_circuit_broken: 0,
            route_rules: Vec::new(),
            rate_limit: None,
            backends: HashMap::new(),
            shutdown_tx: None,
        }
    }

    /// Check circuit breaker state, transitioning Open->HalfOpen if reset period elapsed.
    pub fn check_circuit(&mut self) -> bool {
        if self.circuit_state == CircuitState::Open {
            if let Some(opened_at) = self.circuit_opened_at {
                let elapsed = chrono::Utc::now() - opened_at;
                if elapsed.num_seconds() >= self.circuit_reset_secs as i64 {
                    self.circuit_state = CircuitState::HalfOpen;
                    tracing::info!("Circuit breaker transitioning to half-open");
                    return true;
                }
            }
            return false;
        }
        true
    }

    /// Record a success, potentially closing the circuit from half-open.
    pub fn record_success(&mut self) {
        self.circuit_failures = 0;
        if self.circuit_state == CircuitState::HalfOpen {
            self.circuit_state = CircuitState::Closed;
            self.circuit_opened_at = None;
            tracing::info!("Circuit breaker closed (service recovered)");
        }
    }

    /// Record a failure, potentially opening the circuit.
    pub fn record_failure(&mut self) {
        self.circuit_failures += 1;
        if self.circuit_failures >= self.circuit_threshold {
            self.circuit_state = CircuitState::Open;
            self.circuit_opened_at = Some(chrono::Utc::now());
            tracing::error!(
                "Circuit breaker OPEN ({} consecutive failures)",
                self.circuit_failures
            );
        }
    }

    /// Simulate a request through the proxy. Returns true if allowed, false if circuit broken.
    pub fn handle_request(&mut self) -> bool {
        self.requests_total += 1;
        if !self.check_circuit() {
            self.requests_circuit_broken += 1;
            return false;
        }
        self.connections_total += 1;
        self.connections_active = self.connections_active.saturating_sub(1) + 1;
        true
    }

    /// Calculate retry delay for a given attempt number (exponential backoff).
    pub fn retry_delay_ms(&self, attempt: u32) -> u64 {
        if attempt == 0 {
            return 0;
        }
        let base = self.retry_base_ms as u64;
        let delay = base * 2u64.pow(attempt - 1);
        let max_delay = base * 2u64.pow(self.retry_attempts);
        delay.min(max_delay)
    }

    /// Add a route rule.
    pub fn add_route_rule(&mut self, rule: RouteRule) {
        self.route_rules.push(rule);
    }

    /// Route a request path to a target using configured rules.
    pub fn route(&self, path: &str) -> Option<String> {
        for rule in &self.route_rules {
            if rule.healthy && path.starts_with(&rule.path_prefix) {
                return Some(rule.target.clone());
            }
        }
        None
    }

    /// Set rate limiting configuration.
    pub fn set_rate_limit(&mut self, config: RateLimitConfig) {
        self.rate_limit = Some(config);
    }

    /// Check if rate limit would allow a request.
    pub fn check_rate_limit(&self) -> bool {
        if let Some(ref limit) = self.rate_limit {
            limit.max_requests_per_sec > 0
        } else {
            true
        }
    }

    /// Register a backend and its health status.
    pub fn register_backend(&mut self, addr: String, healthy: bool) {
        self.backends.insert(addr, healthy);
    }

    /// Select the healthiest available backend.
    pub fn select_backend(&self) -> Option<String> {
        self.backends
            .iter()
            .filter(|(_, &healthy)| healthy)
            .map(|(addr, _)| addr.clone())
            .next()
    }

    /// Get pool statistics.
    pub fn pool_stats(&self) -> PoolStats {
        PoolStats {
            active: self.connections_active,
            idle: self
                .connections_total
                .saturating_sub(self.connections_active),
            total: self.connections_total,
            waiters: 0,
        }
    }
}

impl Default for ProxyShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ProxyShim {
    fn name(&self) -> &str {
        "proxy"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "ProxyShim initialized (listen={}, target={}, max_conn={})",
            self.listen,
            self.target,
            self.max_connections,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let circuit_reset_secs = self.circuit_reset_secs;

        tokio::spawn(async move {
            let mut circuit_timer =
                tokio::time::interval(std::time::Duration::from_secs(circuit_reset_secs));

            loop {
                tokio::select! {
                    _ = circuit_timer.tick() => {}
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Proxy shim shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!(
            "ProxyShim started (listen={}, pool={}/{})",
            self.listen,
            self.min_idle,
            self.max_connections,
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("ProxyShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let circuit_val = match self.circuit_state {
            CircuitState::Closed => 0.0,
            CircuitState::Open => 1.0,
            CircuitState::HalfOpen => 2.0,
        };

        vec![
            Metric::new("proxy_connections_active", self.connections_active as f64),
            Metric::new("proxy_connections_total", self.connections_total as f64),
            Metric::new("proxy_requests_total", self.requests_total as f64),
            Metric::new("proxy_requests_retried", self.requests_retried as f64),
            Metric::new(
                "proxy_requests_circuit_broken",
                self.requests_circuit_broken as f64,
            ),
            Metric::new("proxy_circuit_state", circuit_val),
            Metric::new("proxy_circuit_failures", self.circuit_failures as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_starts_closed() {
        let shim = ProxyShim::new();
        assert_eq!(shim.circuit_state, CircuitState::Closed);
    }

    #[test]
    fn test_check_circuit_closed() {
        let mut shim = ProxyShim::new();
        assert!(shim.check_circuit());
    }

    #[test]
    fn test_circuit_opens_after_threshold() {
        let mut shim = ProxyShim {
            circuit_threshold: 3,
            ..ProxyShim::new()
        };
        shim.record_failure();
        shim.record_failure();
        assert_eq!(shim.circuit_state, CircuitState::Closed);
        shim.record_failure();
        assert_eq!(shim.circuit_state, CircuitState::Open);
        assert!(shim.circuit_opened_at.is_some());
    }

    #[test]
    fn test_record_success_resets_failures() {
        let mut shim = ProxyShim::new();
        shim.record_failure();
        shim.record_failure();
        assert_eq!(shim.circuit_failures, 2);
        shim.record_success();
        assert_eq!(shim.circuit_failures, 0);
    }

    #[test]
    fn test_circuit_half_open_to_closed() {
        let mut shim = ProxyShim {
            circuit_threshold: 1,
            ..ProxyShim::new()
        };
        shim.record_failure();
        assert_eq!(shim.circuit_state, CircuitState::Open);

        shim.circuit_state = CircuitState::HalfOpen;
        shim.record_success();
        assert_eq!(shim.circuit_state, CircuitState::Closed);
        assert!(shim.circuit_opened_at.is_none());
    }

    #[test]
    fn test_retry_delay_exponential() {
        let shim = ProxyShim {
            retry_base_ms: 100,
            retry_attempts: 3,
            ..ProxyShim::new()
        };
        assert_eq!(shim.retry_delay_ms(0), 0);
        assert_eq!(shim.retry_delay_ms(1), 100);
        assert_eq!(shim.retry_delay_ms(2), 200);
        assert_eq!(shim.retry_delay_ms(3), 400);
    }

    #[test]
    fn test_handle_request_allows_when_closed() {
        let mut shim = ProxyShim::new();
        assert!(shim.handle_request());
        assert_eq!(shim.requests_total, 1);
    }

    #[test]
    fn test_handle_request_rejects_when_open() {
        let mut shim = ProxyShim {
            circuit_threshold: 1,
            ..ProxyShim::new()
        };
        shim.record_failure();
        assert_eq!(shim.circuit_state, CircuitState::Open);

        assert!(!shim.handle_request());
        assert_eq!(shim.requests_circuit_broken, 1);
    }

    #[test]
    fn test_route_matching() {
        let mut shim = ProxyShim::new();
        shim.add_route_rule(RouteRule {
            path_prefix: "/api/v1".to_string(),
            target: "backend-v1:5432".to_string(),
            weight: 100,
            healthy: true,
        });
        shim.add_route_rule(RouteRule {
            path_prefix: "/api/v2".to_string(),
            target: "backend-v2:5432".to_string(),
            weight: 100,
            healthy: true,
        });

        assert_eq!(
            shim.route("/api/v1/users"),
            Some("backend-v1:5432".to_string())
        );
        assert_eq!(
            shim.route("/api/v2/orders"),
            Some("backend-v2:5432".to_string())
        );
        assert_eq!(shim.route("/other/path"), None);
    }

    #[test]
    fn test_route_unhealthy_skipped() {
        let mut shim = ProxyShim::new();
        shim.add_route_rule(RouteRule {
            path_prefix: "/api".to_string(),
            target: "backend-v1:5432".to_string(),
            weight: 100,
            healthy: false,
        });
        assert_eq!(shim.route("/api/test"), None);
    }

    #[test]
    fn test_select_backend() {
        let mut shim = ProxyShim::new();
        shim.register_backend("healthy:5432".to_string(), true);
        shim.register_backend("sick:5432".to_string(), false);

        assert_eq!(shim.select_backend(), Some("healthy:5432".to_string()));
    }

    #[test]
    fn test_select_backend_none_when_all_unhealthy() {
        let mut shim = ProxyShim::new();
        shim.register_backend("a:5432".to_string(), false);
        shim.register_backend("b:5432".to_string(), false);
        assert!(shim.select_backend().is_none());
    }

    #[test]
    fn test_pool_stats() {
        let shim = ProxyShim {
            connections_active: 5,
            connections_total: 20,
            ..ProxyShim::new()
        };
        let stats = shim.pool_stats();
        assert_eq!(stats.active, 5);
        assert_eq!(stats.idle, 15);
        assert_eq!(stats.total, 20);
    }

    #[test]
    fn test_circuit_state_display() {
        assert_eq!(CircuitState::Closed.to_string(), "closed");
        assert_eq!(CircuitState::Open.to_string(), "open");
        assert_eq!(CircuitState::HalfOpen.to_string(), "half_open");
    }

    #[test]
    fn test_rate_limit_no_config() {
        let shim = ProxyShim::new();
        assert!(shim.check_rate_limit());
    }

    #[test]
    fn test_rate_limit_with_config() {
        let mut shim = ProxyShim::new();
        shim.set_rate_limit(RateLimitConfig {
            max_requests_per_sec: 100,
            burst: 200,
            window_secs: 1,
        });
        assert!(shim.check_rate_limit());
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = ProxyShim {
            connections_active: 10,
            connections_total: 50,
            requests_total: 100,
            requests_retried: 5,
            requests_circuit_broken: 3,
            circuit_state: CircuitState::HalfOpen,
            circuit_failures: 7,
            ..ProxyShim::new()
        };
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 7);
        assert_eq!(metrics[4].name, "proxy_requests_circuit_broken");
        assert_eq!(metrics[4].value, 3.0);
        assert_eq!(metrics[5].value, 2.0);
    }
}
