//! Chaos shim — fault injection for resilience testing.
//!
//! Injects faults (latency, errors, partitions) to test application resilience.
//!
//! ## Environment Variables
//!
//! ```text
//! CHAOS_ENABLED          Enable chaos (default: false)
//! CHAOS_LATENCY_MS       Add latency to requests (default: 0)
//! CHAOS_ERROR_RATE       Error rate 0.0-1.0 (default: 0.0)
//! CHAOS_PARTITION        Simulate network partition (default: false)
//! CHAOS_KILL_PROBABILITY Probability of killing process (default: 0.0)
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Chaos shim.
pub struct ChaosShim {
    enabled: bool,
    latency_ms: u64,
    error_rate: f64,
    partition: bool,
    kill_probability: f64,
    faults_injected: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ChaosShim {
    pub fn new() -> Self {
        Self {
            enabled: std::env::var("CHAOS_ENABLED").map(|v| v == "true" || v == "1").unwrap_or(false),
            latency_ms: std::env::var("CHAOS_LATENCY_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(0),
            error_rate: std::env::var("CHAOS_ERROR_RATE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0),
            partition: std::env::var("CHAOS_PARTITION").map(|v| v == "true" || v == "1").unwrap_or(false),
            kill_probability: std::env::var("CHAOS_KILL_PROBABILITY").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0),
            faults_injected: 0, shutdown_tx: None,
        }
    }
}

impl Default for ChaosShim { fn default() -> Self { Self::new() } }

#[async_trait::async_trait]
impl Capability for ChaosShim {
    fn name(&self) -> &str { "chaos" }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("ChaosShim initialized (enabled={}, latency={}ms, error_rate={})",
            self.enabled, self.latency_ms, self.error_rate);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ChaosShim started (enabled={})", self.enabled);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(true); }
        tracing::info!("ChaosShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("chaos_faults_injected", self.faults_injected as f64),
        ]
    }
}
