//! Compliance shim — CIS/STIG compliance checking.
//!
//! Checks database configuration against security benchmarks.
//!
//! ## Environment Variables
//!
//! ```text
//! COMPLIANCE_BENCHMARK   Benchmark: cis, stig, custom (default: cis)
//! COMPLIANCE_DB_TYPE     Database type: postgres, mariadb
//! COMPLIANCE_REPORT      Report format: json, text (default: json)
//! COMPLIANCE_OUTPUT      Output: stdout, file, webhook
//! COMPLIANCE_OUTPUT_FILE File path for reports
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Compliance shim.
pub struct ComplianceShim {
    benchmark: String,
    db_type: String,
    report_format: String,
    output: String,
    checks_passed: u64,
    checks_failed: u64,
    checks_total: u64,
    compliance_score: f64,
    last_check: Option<chrono::DateTime<chrono::Utc>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ComplianceShim {
    pub fn new() -> Self {
        Self {
            benchmark: std::env::var("COMPLIANCE_BENCHMARK").unwrap_or_else(|_| "cis".to_string()),
            db_type: std::env::var("COMPLIANCE_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            report_format: std::env::var("COMPLIANCE_REPORT").unwrap_or_else(|_| "json".to_string()),
            output: std::env::var("COMPLIANCE_OUTPUT").unwrap_or_else(|_| "stdout".to_string()),
            checks_passed: 0, checks_failed: 0, checks_total: 0,
            compliance_score: 0.0, last_check: None, shutdown_tx: None,
        }
    }
}

impl Default for ComplianceShim { fn default() -> Self { Self::new() } }

#[async_trait::async_trait]
impl Capability for ComplianceShim {
    fn name(&self) -> &str { "compliance" }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("ComplianceShim initialized (benchmark={}, db={})", self.benchmark, self.db_type);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ComplianceShim started (benchmark={})", self.benchmark);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(true); }
        tracing::info!("ComplianceShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("compliance_checks_passed", self.checks_passed as f64),
            Metric::new("compliance_checks_failed", self.checks_failed as f64),
            Metric::new("compliance_score", self.compliance_score),
        ]
    }
}
