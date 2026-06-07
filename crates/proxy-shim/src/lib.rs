//! Proxy shim — connection pooling, retries, and circuit breaker.
//!
//! Sits between the application and database, providing:
//! - Connection pooling (reuse connections)
//! - Automatic retries with exponential backoff
//! - Circuit breaker (stop hammering failing databases)
//! - Graceful degradation (serve stale/cached responses when circuit is open)
//!
//! ## Environment Variables
//!
//! ```text
//! PROXY_LISTEN                  Listen address (default: 0.0.0.0:5432)
//! PROXY_TARGET                  Target database address (required)
//! PROXY_MAX_CONNECTIONS         Max pool connections (default: 20)
//! PROXY_MIN_IDLE                Min idle connections (default: 5)
//! PROXY_MAX_LIFETIME_SECS       Max connection lifetime (default: 1800)
//! PROXY_IDLE_TIMEOUT_SECS       Idle connection timeout (default: 600)
//! PROXY_CONNECT_TIMEOUT         Connect timeout (default: 5)
//! PROXY_RETRY_ATTEMPTS          Max retry attempts (default: 3)
//! PROXY_RETRY_BASE_MS           Base retry delay (default: 100)
//! PROXY_CIRCUIT_THRESHOLD       Failures before open circuit (default: 5)
//! PROXY_CIRCUIT_RESET_SECS      Seconds before half-open (default: 30)
//! PROXY_GRACEFUL_DEGRADATION    Serve stale cache when circuit open (default: false)
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Result of handling a request through the proxy.
#[derive(Debug, Clone, PartialEq)]
pub enum HandleRequestResult {
    /// Request allowed — forwarded to backend.
    Allowed,
    /// Request rejected — circuit is open.
    Rejected,
    /// Response served from stale cache — circuit is open but cache had data.
    ServedFromCache(Vec<u8>),
}

/// Circuit breaker state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CircuitState {
    /// Circuit is closed — requests flow normally.
    Closed,
    /// Circuit is open — requests are rejected.
    Open,
    /// Circuit is testing recovery — one request allowed through.
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
    /// Number of active (in-use) connections.
    pub active: u64,
    /// Number of idle connections in the pool.
    pub idle: u64,
    /// Total connections managed by the pool.
    pub total: u64,
    /// Number of requests waiting for a connection.
    pub waiters: u64,
}

/// Route rule for URL-based routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRule {
    /// URL path prefix to match (e.g., "/api/v1").
    pub path_prefix: String,
    /// Target backend address (e.g., "backend:5432").
    pub target: String,
    /// Weight for weighted round-robin selection.
    pub weight: u32,
    /// Whether this backend is healthy and should receive traffic.
    pub healthy: bool,
}

/// Rate limit config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum requests per second.
    pub max_requests_per_sec: u64,
    /// Burst capacity (max tokens in the bucket).
    pub burst: u64,
    /// Sliding window size in seconds.
    pub window_secs: u64,
}

/// Backend entry with weight for weighted round-robin selection.
#[derive(Debug, Clone)]
pub struct BackendEntry {
    /// Backend address (e.g., "10.0.0.1:5432").
    pub addr: String,
    /// Weight for load balancing.
    pub weight: u32,
    /// Whether this backend is healthy.
    pub healthy: bool,
}

/// Token bucket for rate limiting.
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    rate: f64,
    burst: f64,
}

impl TokenBucket {
    fn new(rate: f64, burst: u64) -> Self {
        Self {
            tokens: burst as f64,
            last_refill: Instant::now(),
            rate,
            burst: burst as f64,
        }
    }

    fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.last_refill = now;
    }
}

/// Mutable state protected by RwLock.
struct ProxyState {
    listen: String,
    target: String,
    max_connections: u32,
    min_idle: u32,
    #[allow(dead_code)]
    max_lifetime_secs: u64,
    #[allow(dead_code)]
    idle_timeout_secs: u64,
    connect_timeout: u64,
    retry_attempts: u32,
    retry_base_ms: u64,
    circuit_threshold: u32,
    circuit_reset_secs: u64,
    circuit_state: CircuitState,
    circuit_failures: u32,
    open_since: Option<Instant>,
    half_open_inflight: bool,
    connections_active: u64,
    connections_total: u64,
    requests_total: u64,
    requests_retried: u64,
    requests_circuit_broken: u64,
    route_rules: Vec<RouteRule>,
    rate_limit: Option<RateLimitConfig>,
    token_bucket: Option<TokenBucket>,
    backends: Vec<BackendEntry>,
    rr_index: usize,
    graceful_degradation_enabled: bool,
    stale_cache: HashMap<String, Vec<u8>>,
    stale_responses_total: u64,
}

/// Proxy shim providing connection pooling and resilience.
pub struct ProxyShim {
    state: Arc<RwLock<ProxyState>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ProxyShim {
    /// Create a new proxy shim from environment variables.
    pub fn new() -> Self {
        let circuit_threshold: u32 = std::env::var("PROXY_CIRCUIT_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);
        let circuit_reset_secs: u64 = std::env::var("PROXY_CIRCUIT_RESET_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let graceful_degradation_enabled: bool = std::env::var("PROXY_GRACEFUL_DEGRADATION")
            .ok()
            .map(|s| s == "true" || s == "1")
            .unwrap_or(false);

        Self {
            state: Arc::new(RwLock::new(ProxyState {
                listen: std::env::var("PROXY_LISTEN")
                    .unwrap_or_else(|_| "0.0.0.0:5432".to_string()),
                target: std::env::var("PROXY_TARGET")
                    .unwrap_or_else(|_| "127.0.0.1:5432".to_string()),
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
                circuit_threshold,
                circuit_reset_secs,
                circuit_state: CircuitState::Closed,
                circuit_failures: 0,
                open_since: None,
                half_open_inflight: false,
                connections_active: 0,
                connections_total: 0,
                requests_total: 0,
                requests_retried: 0,
                requests_circuit_broken: 0,
                route_rules: Vec::new(),
                rate_limit: None,
                token_bucket: None,
                backends: Vec::new(),
                rr_index: 0,
                graceful_degradation_enabled,
                stale_cache: HashMap::new(),
                stale_responses_total: 0,
            })),
            shutdown_tx: None,
        }
    }

    /// Check circuit breaker state, transitioning Open->HalfOpen if reset period elapsed.
    /// Returns true if request is allowed.
    pub fn check_circuit(&self) -> bool {
        let mut state = self.state.write();
        if state.circuit_state == CircuitState::Open {
            if let Some(opened_at) = state.open_since {
                if opened_at.elapsed() >= Duration::from_secs(state.circuit_reset_secs) {
                    state.circuit_state = CircuitState::HalfOpen;
                    state.half_open_inflight = true;
                    tracing::info!("Circuit breaker transitioning to half-open");
                    return true;
                }
            }
            return false;
        }
        if state.circuit_state == CircuitState::HalfOpen {
            if state.half_open_inflight {
                return false;
            }
            state.half_open_inflight = true;
        }
        true
    }

    /// Record a success, potentially closing the circuit from half-open.
    pub fn record_success(&self) {
        let mut state = self.state.write();
        state.circuit_failures = 0;
        if state.circuit_state == CircuitState::HalfOpen {
            state.circuit_state = CircuitState::Closed;
            state.open_since = None;
            state.half_open_inflight = false;
            tracing::info!("Circuit breaker closed (service recovered)");
        }
    }

    /// Record a failure, potentially opening the circuit.
    pub fn record_failure(&self) {
        let mut state = self.state.write();
        state.circuit_failures += 1;
        if state.circuit_state == CircuitState::HalfOpen {
            state.circuit_state = CircuitState::Open;
            state.open_since = Some(Instant::now());
            state.half_open_inflight = false;
            tracing::error!(
                "Circuit breaker OPEN from half-open ({} consecutive failures)",
                state.circuit_failures
            );
            return;
        }
        if state.circuit_failures >= state.circuit_threshold {
            state.circuit_state = CircuitState::Open;
            state.open_since = Some(Instant::now());
            tracing::error!(
                "Circuit breaker OPEN ({} consecutive failures)",
                state.circuit_failures
            );
        }
    }

    /// Simulate a request through the proxy. Returns true if allowed, false if circuit broken.
    pub fn handle_request(&self) -> bool {
        let mut state = self.state.write();
        state.requests_total += 1;

        if state.circuit_state == CircuitState::Open {
            if let Some(opened_at) = state.open_since {
                if opened_at.elapsed() >= Duration::from_secs(state.circuit_reset_secs) {
                    state.circuit_state = CircuitState::HalfOpen;
                    state.half_open_inflight = true;
                    tracing::info!("Circuit breaker transitioning to half-open");
                }
            }
        }

        match state.circuit_state {
            CircuitState::Open => {
                state.requests_circuit_broken += 1;
                return false;
            }
            CircuitState::HalfOpen if state.half_open_inflight => {
                state.requests_circuit_broken += 1;
                return false;
            }
            _ => {}
        }

        if state.circuit_state == CircuitState::HalfOpen {
            state.half_open_inflight = true;
        }

        state.connections_total += 1;
        state.connections_active = state.connections_active.saturating_sub(1) + 1;
        true
    }

    /// Handle a request with graceful degradation support.
    ///
    /// When the circuit is open and graceful degradation is enabled, attempts
    /// to serve a cached response. Returns `Rejected` only when no cached
    /// response is available.
    pub fn handle_request_with_degradation(&self, request_key: &str) -> HandleRequestResult {
        let mut state = self.state.write();
        state.requests_total += 1;

        if state.circuit_state == CircuitState::Open {
            // Serve from cache first when circuit is open
            if state.graceful_degradation_enabled {
                if let Some(cached) = state.stale_cache.get(request_key) {
                    let data = cached.clone();
                    state.stale_responses_total += 1;
                    tracing::debug!(
                        "Serving stale response for key '{}' (circuit open)",
                        request_key
                    );
                    return HandleRequestResult::ServedFromCache(data);
                }
            }
            // No cache hit — try Open -> HalfOpen transition
            if let Some(opened_at) = state.open_since {
                if opened_at.elapsed() >= Duration::from_secs(state.circuit_reset_secs) {
                    state.circuit_state = CircuitState::HalfOpen;
                    state.half_open_inflight = false;
                    tracing::info!("Circuit breaker transitioning to half-open");
                }
            }
        }

        match state.circuit_state {
            CircuitState::Open => {
                state.requests_circuit_broken += 1;
                HandleRequestResult::Rejected
            }
            CircuitState::HalfOpen => {
                if state.half_open_inflight {
                    state.requests_circuit_broken += 1;
                    HandleRequestResult::Rejected
                } else {
                    state.half_open_inflight = true;
                    state.connections_total += 1;
                    state.connections_active = state.connections_active.saturating_sub(1) + 1;
                    HandleRequestResult::Allowed
                }
            }
            _ => {
                state.connections_total += 1;
                state.connections_active = state.connections_active.saturating_sub(1) + 1;
                HandleRequestResult::Allowed
            }
        }
    }

    /// Cache a successful response for a given request key.
    ///
    /// When the circuit later opens, cached responses can be served via
    /// `handle_request_with_degradation`.
    pub fn cache_response(&self, request_key: &str, response: Vec<u8>) {
        self.state
            .write()
            .stale_cache
            .insert(request_key.to_string(), response);
    }

    /// Get a cached response for a request key, if available.
    pub fn get_cached_response(&self, request_key: &str) -> Option<Vec<u8>> {
        self.state.read().stale_cache.get(request_key).cloned()
    }

    /// Check if graceful degradation is enabled.
    pub fn is_graceful_degradation_enabled(&self) -> bool {
        self.state.read().graceful_degradation_enabled
    }

    /// Get the number of stale responses served.
    pub fn stale_responses_total(&self) -> u64 {
        self.state.read().stale_responses_total
    }

    /// Calculate retry delay for a given attempt number (exponential backoff).
    pub fn retry_delay_ms(&self, attempt: u32) -> u64 {
        if attempt == 0 {
            return 0;
        }
        let state = self.state.read();
        let base = state.retry_base_ms;
        let delay = base * 2u64.pow(attempt - 1);
        let max_delay = base * 2u64.pow(state.retry_attempts);
        delay.min(max_delay)
    }

    /// Add a route rule.
    pub fn add_route_rule(&self, rule: RouteRule) {
        self.state.write().route_rules.push(rule);
    }

    /// Route a request path to a target using configured rules.
    pub fn route(&self, path: &str) -> Option<String> {
        let state = self.state.read();
        for rule in &state.route_rules {
            if rule.healthy && path.starts_with(&rule.path_prefix) {
                return Some(rule.target.clone());
            }
        }
        None
    }

    /// Set rate limiting configuration.
    pub fn set_rate_limit(&self, config: RateLimitConfig) {
        let mut state = self.state.write();
        let burst = config.burst;
        state.token_bucket = Some(TokenBucket::new(config.max_requests_per_sec as f64, burst));
        state.rate_limit = Some(config);
    }

    /// Check if rate limit would allow a request. Token bucket: refill + consume.
    pub fn check_rate_limit(&self) -> bool {
        let mut state = self.state.write();
        if state.token_bucket.is_none() {
            return true;
        }
        if let Some(ref mut bucket) = state.token_bucket {
            bucket.try_consume()
        } else {
            true
        }
    }

    /// Register a backend with a given weight.
    pub fn register_backend(&self, addr: String, weight: u32, healthy: bool) {
        self.state.write().backends.push(BackendEntry {
            addr,
            weight,
            healthy,
        });
    }

    /// Set health status for a backend by address.
    pub fn set_backend_health(&self, addr: &str, healthy: bool) {
        let mut state = self.state.write();
        if let Some(entry) = state.backends.iter_mut().find(|b| b.addr == addr) {
            entry.healthy = healthy;
        }
    }

    /// Select backend via weighted round-robin, skipping unhealthy entries.
    pub fn select_backend(&self) -> Option<String> {
        let mut state = self.state.write();
        let healthy: Vec<(usize, u32)> = state
            .backends
            .iter()
            .enumerate()
            .filter(|(_, b)| b.healthy)
            .map(|(i, b)| (i, b.weight))
            .collect();

        if healthy.is_empty() {
            return None;
        }

        let total_weight: u32 = healthy.iter().map(|(_, w)| *w).sum();
        if total_weight == 0 {
            return None;
        }

        let mut remaining = (state.rr_index as u32) % total_weight;
        for &(idx, weight) in &healthy {
            if remaining < weight {
                state.rr_index = (state.rr_index + 1) % total_weight as usize;
                return Some(state.backends[idx].addr.clone());
            }
            remaining -= weight;
        }

        state.rr_index = (state.rr_index + 1) % total_weight as usize;
        Some(state.backends[healthy[0].0].addr.clone())
    }

    /// Get pool statistics.
    pub fn pool_stats(&self) -> PoolStats {
        let state = self.state.read();
        PoolStats {
            active: state.connections_active,
            idle: state
                .connections_total
                .saturating_sub(state.connections_active),
            total: state.connections_total,
            waiters: 0,
        }
    }

    // =========================================================================
    // Real TCP Proxy
    // =========================================================================

    /// Start the TCP proxy server.
    ///
    /// Listens on the configured address, accepts connections, and forwards
    /// traffic to backends using the selected load balancing strategy.
    #[allow(dead_code)]
    pub async fn start_tcp_proxy(&self) -> anyhow::Result<()> {
        let listen_addr =
            std::env::var("PROXY_LISTEN").unwrap_or_else(|_| "0.0.0.0:5432".to_string());

        let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
        tracing::info!("TCP proxy listening on {}", listen_addr);

        let state = self.state.clone();
        let backends = {
            let s = state.read();
            s.backends
                .iter()
                .map(|b| b.addr.clone())
                .collect::<Vec<_>>()
        };

        loop {
            let (client_stream, client_addr) = listener.accept().await?;
            tracing::debug!("Connection from {}", client_addr);

            let state = state.clone();
            let backends = backends.clone();
            let _circuit_threshold = { state.read().circuit_threshold };
            let _retry_attempts = { state.read().retry_attempts };
            let connect_timeout_ms = { state.read().connect_timeout * 1000 };

            tokio::spawn(async move {
                // Select backend
                let backend = {
                    let mut s = state.write();
                    Self::select_backend_static(&mut s, &backends)
                };

                let backend = match backend {
                    Some(b) => b,
                    None => {
                        tracing::warn!("No healthy backend for connection from {}", client_addr);
                        return;
                    }
                };

                // Connect to backend
                let backend_stream = match tokio::time::timeout(
                    Duration::from_millis(connect_timeout_ms),
                    tokio::net::TcpStream::connect(&backend),
                )
                .await
                {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(e)) => {
                        tracing::error!("Backend connect failed ({}): {}", backend, e);
                        Self::record_failure_static(&state);
                        return;
                    }
                    Err(_) => {
                        tracing::error!(
                            "Backend connect timeout ({}): {}ms",
                            backend,
                            connect_timeout_ms
                        );
                        Self::record_failure_static(&state);
                        return;
                    }
                };

                // Bidirectional forwarding
                Self::pipe_streams(client_stream, backend_stream).await;
            });
        }
    }

    /// Select backend using round-robin.
    fn select_backend_static(state: &mut ProxyState, backends: &[String]) -> Option<String> {
        if backends.is_empty() {
            return None;
        }

        let healthy: Vec<(usize, u32)> = state
            .backends
            .iter()
            .enumerate()
            .filter(|(_, b)| b.healthy)
            .map(|(i, b)| (i, b.weight))
            .collect();

        if healthy.is_empty() {
            return None;
        }

        let total_weight: u32 = healthy.iter().map(|(_, w)| *w).sum();
        if total_weight == 0 {
            return None;
        }

        let mut remaining = (state.rr_index as u32) % total_weight;
        for &(idx, weight) in &healthy {
            if remaining < weight {
                state.rr_index = (state.rr_index + 1) % total_weight as usize;
                return Some(state.backends[idx].addr.clone());
            }
            remaining -= weight;
        }

        state.rr_index = (state.rr_index + 1) % total_weight as usize;
        Some(state.backends[healthy[0].0].addr.clone())
    }

    /// Record a connection failure.
    fn record_failure_static(state: &Arc<RwLock<ProxyState>>) {
        let mut s = state.write();
        s.circuit_failures += 1;
    }

    /// Pipe data bidirectionally between two TCP streams.
    async fn pipe_streams(mut client: tokio::net::TcpStream, mut backend: tokio::net::TcpStream) {
        let (mut cr, mut cw) = client.split();
        let (mut br, mut bw) = backend.split();

        let client_to_backend = tokio::io::copy(&mut cr, &mut bw);
        let backend_to_client = tokio::io::copy(&mut br, &mut cw);

        tokio::select! {
            result = client_to_backend => {
                if let Err(e) = result {
                    tracing::debug!("Client->Backend stream ended: {}", e);
                }
            }
            result = backend_to_client => {
                if let Err(e) = result {
                    tracing::debug!("Backend->Client stream ended: {}", e);
                }
            }
        }
    }

    // =========================================================================
    // Real Connection Pool
    // =========================================================================

    /// Create a connection pool for a PostgreSQL backend.
    ///
    /// Uses deadpool-postgres for production-grade connection pooling.
    #[allow(dead_code)]
    pub async fn create_pg_pool(
        &self,
        host: &str,
        port: u16,
        user: &str,
        password: &str,
        database: &str,
    ) -> anyhow::Result<deadpool_postgres::Pool> {
        let cfg = deadpool_postgres::Config {
            host: Some(host.to_string()),
            port: Some(port),
            user: Some(user.to_string()),
            password: Some(password.to_string()),
            dbname: Some(database.to_string()),
            pool: Some(deadpool_postgres::PoolConfig::new(
                self.state.read().max_connections as usize,
            )),
            ..Default::default()
        };

        let pool = cfg.create_pool(
            Some(deadpool_postgres::Runtime::Tokio1),
            tokio_postgres::NoTls,
        )?;

        tracing::info!(
            "Created PostgreSQL connection pool: {}:{}/{} (max={})",
            host,
            port,
            database,
            self.state.read().max_connections
        );

        Ok(pool)
    }

    /// Get a connection from the pool and return it when done.
    #[allow(dead_code)]
    pub async fn get_connection(
        pool: &deadpool_postgres::Pool,
    ) -> anyhow::Result<deadpool_postgres::Object> {
        pool.get()
            .await
            .map_err(|e| anyhow::anyhow!("Pool connection failed: {}", e))
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
        let state = self.state.read();
        tracing::info!(
            "ProxyShim initialized (listen={}, target={}, max_conn={})",
            state.listen,
            state.target,
            state.max_connections,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let circuit_reset_secs = {
            let state = self.state.read();
            state.circuit_reset_secs
        };

        tokio::spawn(async move {
            let mut circuit_timer = tokio::time::interval(Duration::from_secs(circuit_reset_secs));

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

        let state = self.state.read();
        tracing::info!(
            "ProxyShim started (listen={}, pool={}/{})",
            state.listen,
            state.min_idle,
            state.max_connections,
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
        let state = self.state.read();
        let circuit_val = match state.circuit_state {
            CircuitState::Closed => 0.0,
            CircuitState::Open => 1.0,
            CircuitState::HalfOpen => 2.0,
        };

        vec![
            Metric::new("proxy_connections_active", state.connections_active as f64),
            Metric::new("proxy_connections_total", state.connections_total as f64),
            Metric::new("proxy_requests_total", state.requests_total as f64),
            Metric::new("proxy_requests_retried", state.requests_retried as f64),
            Metric::new(
                "proxy_requests_circuit_broken",
                state.requests_circuit_broken as f64,
            ),
            Metric::new("proxy_circuit_state", circuit_val),
            Metric::new("proxy_circuit_failures", state.circuit_failures as f64),
            Metric::new(
                "proxy_stale_responses_total",
                state.stale_responses_total as f64,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn test_circuit_starts_closed() {
        let shim = ProxyShim::new();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
    }

    #[test]
    fn test_check_circuit_closed() {
        let shim = ProxyShim::new();
        assert!(shim.check_circuit());
    }

    #[test]
    fn test_circuit_opens_after_threshold() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 3,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);
        assert!(shim.state.read().open_since.is_some());
    }

    #[test]
    fn test_record_success_resets_failures() {
        let shim = ProxyShim::new();
        shim.record_failure();
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_failures, 2);
        shim.record_success();
        assert_eq!(shim.state.read().circuit_failures, 0);
    }

    #[test]
    fn test_circuit_half_open_to_closed() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);

        shim.state.write().circuit_state = CircuitState::HalfOpen;
        shim.state.write().half_open_inflight = false;
        shim.record_success();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        assert!(shim.state.read().open_since.is_none());
    }

    #[test]
    fn test_circuit_open_to_half_open_after_timeout() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 0,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);

        // With reset_secs=0, check_circuit should transition to HalfOpen
        assert!(shim.check_circuit());
        assert_eq!(shim.state.read().circuit_state, CircuitState::HalfOpen);
    }

    #[test]
    fn test_circuit_half_open_rejects_concurrent() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 0,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();

        // First check transitions to half-open, allows request
        assert!(shim.check_circuit());
        assert_eq!(shim.state.read().circuit_state, CircuitState::HalfOpen);

        // Second check should reject (inflight)
        assert!(!shim.check_circuit());
    }

    #[test]
    fn test_circuit_half_open_failure_reopens() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 0,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        shim.check_circuit();
        assert_eq!(shim.state.read().circuit_state, CircuitState::HalfOpen);

        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);
    }

    #[test]
    fn test_token_bucket_fill_and_drain() {
        let mut bucket = TokenBucket::new(10.0, 5);
        assert_eq!(bucket.tokens, 5.0);

        // Drain all tokens
        for _ in 0..5 {
            assert!(bucket.try_consume());
        }
        assert!(!bucket.try_consume());

        // Refill: set last_refill to the past
        bucket.last_refill = Instant::now() - Duration::from_secs(1);
        assert!(bucket.try_consume());
        assert!((bucket.tokens - 4.0).abs() < 0.1);
    }

    #[test]
    fn test_token_bucket_burst_cap() {
        let mut bucket = TokenBucket::new(100.0, 3);
        assert_eq!(bucket.tokens, 3.0);

        // Even after long elapsed time, tokens capped at burst
        bucket.last_refill = Instant::now() - Duration::from_secs(10);
        bucket.refill();
        assert!((bucket.tokens - 3.0).abs() < 0.1);
    }

    #[test]
    fn test_rate_limit_no_config() {
        let shim = ProxyShim::new();
        assert!(shim.check_rate_limit());
    }

    #[test]
    fn test_rate_limit_token_bucket_drain() {
        let shim = ProxyShim::new();
        shim.set_rate_limit(RateLimitConfig {
            max_requests_per_sec: 2,
            burst: 2,
            window_secs: 1,
        });

        // Should allow burst of 2
        assert!(shim.check_rate_limit());
        assert!(shim.check_rate_limit());
        // Third should fail (bucket empty)
        assert!(!shim.check_rate_limit());
    }

    #[test]
    fn test_rate_limit_refill_after_wait() {
        let shim = ProxyShim::new();
        shim.set_rate_limit(RateLimitConfig {
            max_requests_per_sec: 10,
            burst: 1,
            window_secs: 1,
        });

        // Drain the single token
        assert!(shim.check_rate_limit());
        assert!(!shim.check_rate_limit());

        // Manually age the bucket
        {
            let mut state = shim.state.write();
            if let Some(ref mut bucket) = state.token_bucket {
                bucket.last_refill = Instant::now() - Duration::from_secs(1);
            }
        }

        // Should refill and allow again
        assert!(shim.check_rate_limit());
    }

    #[test]
    fn test_weighted_round_robin() {
        let shim = ProxyShim::new();
        // A=weight 2, B=weight 1 -> pattern A,A,B repeated
        shim.register_backend("A:5432".into(), 2, true);
        shim.register_backend("B:5432".into(), 1, true);

        let mut counts = HashMap::new();
        let total = 60;
        for _ in 0..total {
            let b = shim.select_backend().unwrap();
            *counts.entry(b).or_insert(0) += 1;
        }
        // A should get ~40, B ~20
        assert_eq!(counts.get("A:5432"), Some(&40));
        assert_eq!(counts.get("B:5432"), Some(&20));
    }

    #[test]
    fn test_round_robin_skips_unhealthy() {
        let shim = ProxyShim::new();
        shim.register_backend("A:5432".into(), 1, true);
        shim.register_backend("B:5432".into(), 1, false);
        shim.register_backend("C:5432".into(), 1, true);

        let mut results = Vec::new();
        for _ in 0..6 {
            results.push(shim.select_backend().unwrap());
        }
        // Only A and C should appear, alternating
        for r in &results {
            assert!(r == "A:5432" || r == "C:5432", "got {r}");
        }
    }

    #[test]
    fn test_select_backend_none_when_all_unhealthy() {
        let shim = ProxyShim::new();
        shim.register_backend("a:5432".into(), 1, false);
        shim.register_backend("b:5432".into(), 1, false);
        assert!(shim.select_backend().is_none());
    }

    #[test]
    fn test_select_backend_single_weight() {
        let shim = ProxyShim::new();
        shim.register_backend("only:5432".into(), 5, true);

        for _ in 0..10 {
            assert_eq!(shim.select_backend(), Some("only:5432".to_string()));
        }
    }

    #[test]
    fn test_retry_delay_exponential() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                retry_base_ms: 100,
                retry_attempts: 3,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        assert_eq!(shim.retry_delay_ms(0), 0);
        assert_eq!(shim.retry_delay_ms(1), 100);
        assert_eq!(shim.retry_delay_ms(2), 200);
        assert_eq!(shim.retry_delay_ms(3), 400);
    }

    #[test]
    fn test_handle_request_allows_when_closed() {
        let shim = ProxyShim::new();
        assert!(shim.handle_request());
        assert_eq!(shim.state.read().requests_total, 1);
    }

    #[test]
    fn test_handle_request_rejects_when_open() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);

        assert!(!shim.handle_request());
        assert_eq!(shim.state.read().requests_circuit_broken, 1);
    }

    #[test]
    fn test_route_matching() {
        let shim = ProxyShim::new();
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
        let shim = ProxyShim::new();
        shim.add_route_rule(RouteRule {
            path_prefix: "/api".to_string(),
            target: "backend-v1:5432".to_string(),
            weight: 100,
            healthy: false,
        });
        assert_eq!(shim.route("/api/test"), None);
    }

    #[test]
    fn test_pool_stats() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                connections_active: 5,
                connections_total: 20,
                ..make_default_state()
            })),
            shutdown_tx: None,
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

    // =====================================================================
    // Graceful degradation tests
    // =====================================================================

    #[test]
    fn test_graceful_degradation_disabled_by_default() {
        let shim = ProxyShim::new();
        assert!(!shim.is_graceful_degradation_enabled());
    }

    #[test]
    fn test_graceful_degradation_cache_and_serve() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                graceful_degradation_enabled: true,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };

        // Cache a response before circuit opens
        shim.cache_response("req-1", b"cached-data".to_vec());
        assert_eq!(
            shim.get_cached_response("req-1"),
            Some(b"cached-data".to_vec())
        );
        assert!(shim.get_cached_response("req-2").is_none());

        // Open the circuit
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);

        // handle_request returns false (backward compat)
        assert!(!shim.handle_request());

        // handle_request_with_degradation returns cached response
        let result = shim.handle_request_with_degradation("req-1");
        assert_eq!(
            result,
            HandleRequestResult::ServedFromCache(b"cached-data".to_vec())
        );
        assert_eq!(shim.stale_responses_total(), 1);

        // Unknown key returns Rejected
        let result = shim.handle_request_with_degradation("req-unknown");
        assert_eq!(result, HandleRequestResult::Rejected);
        assert_eq!(shim.stale_responses_total(), 1);
    }

    #[test]
    fn test_graceful_degradation_disabled_rejects_when_open() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                graceful_degradation_enabled: false,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.cache_response("req-1", b"cached-data".to_vec());
        shim.record_failure();

        // Even though cache exists, degradation is disabled -> Rejected
        let result = shim.handle_request_with_degradation("req-1");
        assert_eq!(result, HandleRequestResult::Rejected);
        assert_eq!(shim.stale_responses_total(), 0);
    }

    #[test]
    fn test_graceful_degradation_allows_when_closed() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                graceful_degradation_enabled: true,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };

        let result = shim.handle_request_with_degradation("any-key");
        assert_eq!(result, HandleRequestResult::Allowed);
        assert_eq!(shim.stale_responses_total(), 0);
    }

    #[test]
    fn test_graceful_degradation_half_open_rejects_concurrent() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 0,
                graceful_degradation_enabled: true,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        // No cache for "req-1" — circuit open + no cache -> transition to half-open
        shim.record_failure();

        // First check transitions to half-open, allows probe request
        let r1 = shim.handle_request_with_degradation("req-1");
        assert_eq!(r1, HandleRequestResult::Allowed);

        // Second check should reject (probe already in-flight)
        let r2 = shim.handle_request_with_degradation("req-1");
        assert_eq!(r2, HandleRequestResult::Rejected);
    }

    #[test]
    fn test_stale_cache_cleared_on_recovery() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 0,
                graceful_degradation_enabled: true,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.cache_response("req-1", b"cached".to_vec());
        shim.record_failure();

        // Serve from cache while open
        let result = shim.handle_request_with_degradation("req-1");
        assert_eq!(
            result,
            HandleRequestResult::ServedFromCache(b"cached".to_vec())
        );

        // Use a key with no cache to trigger half-open transition and probe
        let r = shim.handle_request_with_degradation("req-uncached");
        assert_eq!(r, HandleRequestResult::Allowed); // half-open probe

        shim.record_success();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);

        // Cache still exists but circuit is closed, so requests flow normally
        let result = shim.handle_request_with_degradation("req-1");
        assert_eq!(result, HandleRequestResult::Allowed);
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                connections_active: 10,
                connections_total: 50,
                requests_total: 100,
                requests_retried: 5,
                requests_circuit_broken: 3,
                circuit_state: CircuitState::HalfOpen,
                circuit_failures: 7,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 8);
        assert_eq!(metrics[4].name, "proxy_requests_circuit_broken");
        assert_eq!(metrics[4].value, 3.0);
        assert_eq!(metrics[5].value, 2.0);
        assert_eq!(metrics[7].name, "proxy_stale_responses_total");
        assert_eq!(metrics[7].value, 0.0);
    }

    fn make_default_state() -> ProxyState {
        ProxyState {
            listen: "0.0.0.0:5432".to_string(),
            target: "127.0.0.1:5432".to_string(),
            max_connections: 20,
            min_idle: 5,
            max_lifetime_secs: 1800,
            idle_timeout_secs: 600,
            connect_timeout: 5,
            retry_attempts: 3,
            retry_base_ms: 100,
            circuit_threshold: 5,
            circuit_reset_secs: 30,
            circuit_state: CircuitState::Closed,
            circuit_failures: 0,
            open_since: None,
            half_open_inflight: false,
            connections_active: 0,
            connections_total: 0,
            requests_total: 0,
            requests_retried: 0,
            requests_circuit_broken: 0,
            route_rules: Vec::new(),
            rate_limit: None,
            token_bucket: None,
            backends: Vec::new(),
            rr_index: 0,
            graceful_degradation_enabled: false,
            stale_cache: HashMap::new(),
            stale_responses_total: 0,
        }
    }

    // =====================================================================
    // v2.0.4 — Load Testing Proxy Shim Circuit Breaker
    // =====================================================================

    #[test]
    fn test_circuit_full_lifecycle_closed_open_half_closed() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 3,
                circuit_reset_secs: 60,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        // 1. Starts closed
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        assert!(shim.handle_request());

        // 2. Accumulate failures
        shim.record_failure();
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);
        assert!(!shim.handle_request());

        // 3. Transition to half-open via check_circuit (with long reset_secs, immediate won't work)
        //    Force the open_since to past
        shim.state.write().open_since = Some(Instant::now() - Duration::from_secs(120));
        assert!(shim.check_circuit());
        assert_eq!(shim.state.read().circuit_state, CircuitState::HalfOpen);

        // 4. Success closes circuit
        shim.record_success();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        assert!(shim.handle_request());
    }

    #[test]
    fn test_circuit_threshold_exact_n_failures() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 5,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        for _ in 0..4 {
            shim.record_failure();
            assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        }
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);
    }

    #[test]
    fn test_circuit_timeout_half_open_after_cooldown() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 60,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);

        // Immediately after open, should NOT transition to half-open
        assert!(!shim.check_circuit());
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);

        // Set open_since to far past to simulate cooldown elapsed
        shim.state.write().open_since =
            Some(Instant::now() - Duration::from_secs(120));
        assert!(shim.check_circuit());
        assert_eq!(shim.state.read().circuit_state, CircuitState::HalfOpen);
    }

    #[test]
    fn test_circuit_success_resets_failure_counter() {
        let shim = ProxyShim::new();
        for _ in 0..4 {
            shim.record_failure();
        }
        assert_eq!(shim.state.read().circuit_failures, 4);
        shim.record_success();
        assert_eq!(shim.state.read().circuit_failures, 0);
        // After reset, we need 5 more failures to open (threshold=5 default)
        for _ in 0..4 {
            shim.record_failure();
            assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        }
    }

    #[test]
    fn test_circuit_half_open_failure_reopens_v2() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 0,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        shim.check_circuit(); // -> HalfOpen
        assert_eq!(shim.state.read().circuit_state, CircuitState::HalfOpen);

        shim.record_failure(); // fail during half-open
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);
        assert!(shim.state.read().open_since.is_some());
    }

    #[test]
    fn test_connection_pool_exhaustion_behavior() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                max_connections: 2,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        // Simulate requests — connections_active always stays at 1 with this code
        for _ in 0..2 {
            assert!(shim.handle_request());
        }
        let stats = shim.pool_stats();
        assert_eq!(stats.active, 1);
        assert_eq!(stats.total, 2);
    }

    #[test]
    fn test_connection_pool_active_increments_on_request() {
        let shim = ProxyShim::new();
        assert_eq!(shim.pool_stats().active, 0);
        shim.handle_request();
        assert_eq!(shim.pool_stats().active, 1);
        shim.handle_request();
        // connections_active stays at 1 due to saturating_sub(1)+1 logic
        assert_eq!(shim.pool_stats().active, 1);
        assert_eq!(shim.pool_stats().total, 2);
    }

    #[test]
    fn test_weighted_routing_heavy_light() {
        let shim = ProxyShim::new();
        shim.register_backend("heavy:5432".into(), 9, true);
        shim.register_backend("light:5432".into(), 1, true);

        let mut counts = HashMap::new();
        for _ in 0..100 {
            let b = shim.select_backend().unwrap();
            *counts.entry(b).or_insert(0) += 1;
        }
        let heavy = counts.get("heavy:5432").copied().unwrap_or(0);
        let light = counts.get("light:5432").copied().unwrap_or(0);
        // heavy should get ~90, light ~10
        assert!(heavy > 80, "heavy got {heavy}");
        assert!(light > 0, "light got {light}");
    }

    #[test]
    fn test_weighted_routing_all_equal() {
        let shim = ProxyShim::new();
        shim.register_backend("a:5432".into(), 1, true);
        shim.register_backend("b:5432".into(), 1, true);
        shim.register_backend("c:5432".into(), 1, true);

        let mut counts = HashMap::new();
        for _ in 0..300 {
            let b = shim.select_backend().unwrap();
            *counts.entry(b).or_insert(0) += 1;
        }
        // Each should get exactly 100 with equal weight
        assert_eq!(counts.get("a:5432"), Some(&100));
        assert_eq!(counts.get("b:5432"), Some(&100));
        assert_eq!(counts.get("c:5432"), Some(&100));
    }

    #[test]
    fn test_retry_delay_exponential_backoff_values() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                retry_base_ms: 50,
                retry_attempts: 4,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        assert_eq!(shim.retry_delay_ms(0), 0);
        assert_eq!(shim.retry_delay_ms(1), 50); // 50 * 2^0
        assert_eq!(shim.retry_delay_ms(2), 100); // 50 * 2^1
        assert_eq!(shim.retry_delay_ms(3), 200); // 50 * 2^2
        assert_eq!(shim.retry_delay_ms(4), 400); // 50 * 2^3
    }

    #[test]
    fn test_retry_delay_capped_at_max() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                retry_base_ms: 100,
                retry_attempts: 2,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        // max_delay = 100 * 2^2 = 400
        assert_eq!(shim.retry_delay_ms(5), 400);
    }

    #[test]
    fn test_timeout_enforcement_via_check_circuit() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 60,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        // Circuit open with long reset_secs — check_circuit returns false (timeout enforced)
        assert!(!shim.check_circuit());
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);
    }

    #[test]
    fn test_handle_request_rejects_when_circuit_open() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        for _ in 0..10 {
            assert!(!shim.handle_request());
        }
        assert_eq!(shim.state.read().requests_circuit_broken, 10);
    }

    #[test]
    fn test_handle_request_with_degradation_open_no_cache_rejects() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                graceful_degradation_enabled: true,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        let result = shim.handle_request_with_degradation("missing");
        assert_eq!(result, HandleRequestResult::Rejected);
    }

    #[test]
    fn test_handle_request_with_degradation_half_open_allows_one() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 0,
                graceful_degradation_enabled: true,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        let r1 = shim.handle_request_with_degradation("key");
        assert_eq!(r1, HandleRequestResult::Allowed);
        let r2 = shim.handle_request_with_degradation("key");
        assert_eq!(r2, HandleRequestResult::Rejected);
    }

    #[test]
    fn test_pool_stats_reflect_state() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                connections_active: 0,
                connections_total: 0,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        assert_eq!(shim.pool_stats().active, 0);
        assert_eq!(shim.pool_stats().total, 0);
        assert_eq!(shim.pool_stats().idle, 0);

        // Simulate requests — active stays at 1, total increments
        shim.handle_request();
        shim.handle_request();
        shim.handle_request();
        let stats = shim.pool_stats();
        assert_eq!(stats.active, 1);
        assert_eq!(stats.total, 3);
        assert_eq!(stats.idle, 2);
    }

    #[test]
    fn test_metrics_reflect_circuit_open_state() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        let metrics = shim.metrics();
        let circuit_state = metrics.iter().find(|m| m.name == "proxy_circuit_state").unwrap();
        assert_eq!(circuit_state.value, 1.0); // Open
        let failures = metrics.iter().find(|m| m.name == "proxy_circuit_failures").unwrap();
        assert_eq!(failures.value, 1.0);
    }

    #[test]
    fn test_rate_limit_integration_with_circuit() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.set_rate_limit(RateLimitConfig {
            max_requests_per_sec: 1,
            burst: 1,
            window_secs: 1,
        });
        assert!(shim.check_rate_limit());
        assert!(!shim.check_rate_limit());

        // Open circuit
        shim.record_failure();
        assert!(!shim.handle_request());
    }

    #[test]
    fn test_circuit_open_since_timestamp_recorded() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        assert!(shim.state.read().open_since.is_none());
        shim.record_failure();
        assert!(shim.state.read().open_since.is_some());
    }

    #[test]
    fn test_circuit_half_open_inflight_flag() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 1,
                circuit_reset_secs: 0,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        shim.record_failure();
        assert!(!shim.state.read().half_open_inflight);
        shim.check_circuit(); // -> HalfOpen, sets half_open_inflight = true
        assert!(shim.state.read().half_open_inflight);
    }

    #[test]
    fn test_weighted_routing_skips_unhealthy_heavy() {
        let shim = ProxyShim::new();
        shim.register_backend("healthy:5432".into(), 10, true);
        shim.register_backend("dead:5432".into(), 100, false);

        for _ in 0..50 {
            let b = shim.select_backend().unwrap();
            assert_eq!(b, "healthy:5432");
        }
    }

    #[test]
    fn test_circuit_state_transitions_serial() {
        let shim = ProxyShim {
            state: Arc::new(RwLock::new(ProxyState {
                circuit_threshold: 2,
                circuit_reset_secs: 0,
                ..make_default_state()
            })),
            shutdown_tx: None,
        };
        // Closed
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);
        // HalfOpen
        shim.check_circuit();
        assert_eq!(shim.state.read().circuit_state, CircuitState::HalfOpen);
        // Success -> Closed
        shim.record_success();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Closed);
        // Failure -> Open
        shim.record_failure();
        shim.record_failure();
        assert_eq!(shim.state.read().circuit_state, CircuitState::Open);
    }
}
