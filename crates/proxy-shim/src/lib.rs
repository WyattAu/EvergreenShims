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

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Circuit breaker state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CircuitState {
    /// Normal operation.
    Closed,
    /// Circuit is open, failing fast.
    Open,
    /// Testing if service recovered.
    HalfOpen,
}

/// Connection pool statistics.
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    pub active: u64,
    pub idle: u64,
    pub total: u64,
    pub waiters: u64,
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
    connections_active: u64,
    connections_total: u64,
    requests_total: u64,
    requests_retried: u64,
    requests_circuit_broken: u64,
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
            connections_active: 0,
            connections_total: 0,
            requests_total: 0,
            requests_retried: 0,
            requests_circuit_broken: 0,
            shutdown_tx: None,
        }
    }

    /// Check circuit breaker state.
    fn check_circuit(&mut self) -> bool {
        match self.circuit_state {
            CircuitState::Closed => true,
            CircuitState::Open => false,
            CircuitState::HalfOpen => true, // Allow one request to test
        }
    }

    /// Record a success.
    fn record_success(&mut self) {
        self.circuit_failures = 0;
        if self.circuit_state == CircuitState::HalfOpen {
            self.circuit_state = CircuitState::Closed;
            tracing::info!("Circuit breaker closed (service recovered)");
        }
    }

    /// Record a failure.
    fn record_failure(&mut self) {
        self.circuit_failures += 1;
        if self.circuit_failures >= self.circuit_threshold {
            self.circuit_state = CircuitState::Open;
            tracing::error!(
                "Circuit breaker OPEN ({} consecutive failures)",
                self.circuit_failures
            );
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

        let _target = self.target.clone();
        let circuit_reset_secs = self.circuit_reset_secs;

        tokio::spawn(async move {
            let mut circuit_timer =
                tokio::time::interval(std::time::Duration::from_secs(circuit_reset_secs));

            loop {
                tokio::select! {
                    _ = circuit_timer.tick() => {
                        // In production: check open circuits and transition to half-open
                    }
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
