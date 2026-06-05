#![allow(dead_code)]
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
//! COMPLIANCE_SEVERITY    Minimum severity to report: info, low, medium, high, critical
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Compliance check severity levels.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Hash, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "info" => Ok(Self::Info),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            _ => Err(format!("Unknown severity: {}", s)),
        }
    }
}

/// A single compliance check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceCheck {
    pub id: String,
    pub description: String,
    pub benchmark: String,
    pub severity: Severity,
    pub passed: bool,
    pub evidence: String,
    pub remediation: String,
}

/// A compliance violation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Violation {
    pub check_id: String,
    pub description: String,
    pub severity: Severity,
    pub detected_at: String,
    pub evidence: String,
    pub remediation: String,
    pub resolved: bool,
}

/// Compliance report summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceReport {
    pub benchmark: String,
    pub db_type: String,
    pub generated_at: String,
    pub total_checks: usize,
    pub passed: usize,
    pub failed: usize,
    pub score: f64,
    pub violations: Vec<Violation>,
}

/// Compliance shim.
pub struct ComplianceShim {
    benchmark: String,
    db_type: String,
    report_format: String,
    output: String,
    min_severity: Severity,
    checks_passed: u64,
    checks_failed: u64,
    checks_total: u64,
    compliance_score: f64,
    last_check: Option<chrono::DateTime<chrono::Utc>>,
    violations: Vec<Violation>,
    checks: Vec<ComplianceCheck>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ComplianceShim {
    pub fn new() -> Self {
        let min_sev = std::env::var("COMPLIANCE_SEVERITY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(Severity::Info);

        Self {
            benchmark: std::env::var("COMPLIANCE_BENCHMARK").unwrap_or_else(|_| "cis".to_string()),
            db_type: std::env::var("COMPLIANCE_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            report_format: std::env::var("COMPLIANCE_REPORT")
                .unwrap_or_else(|_| "json".to_string()),
            output: std::env::var("COMPLIANCE_OUTPUT").unwrap_or_else(|_| "stdout".to_string()),
            min_severity: min_sev,
            checks_passed: 0,
            checks_failed: 0,
            checks_total: 0,
            compliance_score: 0.0,
            last_check: None,
            violations: Vec::new(),
            checks: Vec::new(),
            shutdown_tx: None,
        }
    }

    /// Add a compliance check.
    pub fn add_check(&mut self, check: ComplianceCheck) {
        self.checks.push(check);
    }

    /// Run all registered checks.
    pub fn run_checks(&mut self) {
        self.checks_total = self.checks.len() as u64;
        self.checks_passed = 0;
        self.checks_failed = 0;
        self.violations.clear();

        for check in &self.checks {
            if check.passed {
                self.checks_passed += 1;
            } else if check.severity >= self.min_severity {
                self.checks_failed += 1;
                self.violations.push(Violation {
                    check_id: check.id.clone(),
                    description: check.description.clone(),
                    severity: check.severity.clone(),
                    detected_at: chrono::Utc::now().to_rfc3339(),
                    evidence: check.evidence.clone(),
                    remediation: check.remediation.clone(),
                    resolved: false,
                });
            }
        }

        self.compliance_score = if self.checks_total > 0 {
            (self.checks_passed as f64 / self.checks_total as f64) * 100.0
        } else {
            100.0
        };

        self.last_check = Some(chrono::Utc::now());
    }

    /// Get violations at or above a severity level.
    pub fn violations_by_severity(&self, min_severity: &Severity) -> Vec<&Violation> {
        self.violations
            .iter()
            .filter(|v| v.severity >= *min_severity && !v.resolved)
            .collect()
    }

    /// Resolve a violation by check ID.
    pub fn resolve_violation(&mut self, check_id: &str) -> bool {
        if let Some(v) = self.violations.iter_mut().find(|v| v.check_id == check_id) {
            v.resolved = true;
            true
        } else {
            false
        }
    }

    /// Generate a compliance report.
    pub fn generate_report(&self) -> ComplianceReport {
        let total = self.checks.len();
        let passed = self.checks.iter().filter(|c| c.passed).count();
        let failed_violations: Vec<Violation> = self
            .violations
            .iter()
            .filter(|v| !v.resolved)
            .cloned()
            .collect();

        ComplianceReport {
            benchmark: self.benchmark.clone(),
            db_type: self.db_type.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            total_checks: total,
            passed,
            failed: failed_violations.len(),
            score: self.compliance_score,
            violations: failed_violations,
        }
    }

    /// Get count of unresolved violations.
    pub fn unresolved_count(&self) -> usize {
        self.violations.iter().filter(|v| !v.resolved).count()
    }

    /// Get violation counts by severity.
    pub fn violation_counts(&self) -> HashMap<Severity, usize> {
        let mut counts = HashMap::new();
        for v in &self.violations {
            if !v.resolved {
                *counts.entry(v.severity.clone()).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Check if compliance meets a minimum score threshold.
    pub fn meets_threshold(&self, threshold: f64) -> bool {
        self.compliance_score >= threshold
    }
}

impl Default for ComplianceShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ComplianceShim {
    fn name(&self) -> &str {
        "compliance"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "ComplianceShim initialized (benchmark={}, db={})",
            self.benchmark,
            self.db_type
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("ComplianceShim started (benchmark={})", self.benchmark);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("ComplianceShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("compliance_checks_passed", self.checks_passed as f64),
            Metric::new("compliance_checks_failed", self.checks_failed as f64),
            Metric::new("compliance_score", self.compliance_score),
            Metric::new(
                "compliance_violations_total",
                self.unresolved_count() as f64,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_check(id: &str, passed: bool, severity: Severity) -> ComplianceCheck {
        ComplianceCheck {
            id: id.to_string(),
            description: format!("Check {}", id),
            benchmark: "cis".to_string(),
            severity,
            passed,
            evidence: "test evidence".to_string(),
            remediation: format!("Fix {}", id),
        }
    }

    #[test]
    fn test_severity_parse() {
        assert_eq!("info".parse::<Severity>().unwrap(), Severity::Info);
        assert_eq!("critical".parse::<Severity>().unwrap(), Severity::Critical);
        assert!("invalid".parse::<Severity>().is_err());
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::High);
        assert!(Severity::High > Severity::Medium);
        assert!(Severity::Medium > Severity::Low);
        assert!(Severity::Low > Severity::Info);
    }

    #[test]
    fn test_run_checks_all_pass() {
        let mut shim = ComplianceShim::new();
        shim.add_check(make_check("C001", true, Severity::High));
        shim.add_check(make_check("C002", true, Severity::Medium));

        shim.run_checks();
        assert_eq!(shim.checks_passed, 2);
        assert_eq!(shim.checks_failed, 0);
        assert_eq!(shim.compliance_score, 100.0);
        assert!(shim.violations.is_empty());
    }

    #[test]
    fn test_run_checks_some_fail() {
        let mut shim = ComplianceShim::new();
        shim.add_check(make_check("C001", true, Severity::High));
        shim.add_check(make_check("C002", false, Severity::High));
        shim.add_check(make_check("C003", true, Severity::Low));

        shim.run_checks();
        assert_eq!(shim.checks_passed, 2);
        assert_eq!(shim.checks_failed, 1);
        assert!((shim.compliance_score - 66.67).abs() < 0.01);
        assert_eq!(shim.violations.len(), 1);
    }

    #[test]
    fn test_min_severity_filter() {
        let mut shim = ComplianceShim {
            min_severity: Severity::High,
            ..ComplianceShim::new()
        };
        shim.add_check(make_check("C001", false, Severity::Low));
        shim.add_check(make_check("C002", false, Severity::High));

        shim.run_checks();
        assert_eq!(shim.checks_failed, 1);
        assert_eq!(shim.violations.len(), 1);
        assert_eq!(shim.violations[0].check_id, "C002");
    }

    #[test]
    fn test_violations_by_severity() {
        let mut shim = ComplianceShim::new();
        shim.add_check(make_check("C001", false, Severity::Medium));
        shim.add_check(make_check("C002", false, Severity::Critical));
        shim.run_checks();

        let high_and_above = shim.violations_by_severity(&Severity::High);
        assert_eq!(high_and_above.len(), 1);

        let medium_and_above = shim.violations_by_severity(&Severity::Medium);
        assert_eq!(medium_and_above.len(), 2);
    }

    #[test]
    fn test_resolve_violation() {
        let mut shim = ComplianceShim::new();
        shim.add_check(make_check("C001", false, Severity::High));
        shim.run_checks();

        assert_eq!(shim.unresolved_count(), 1);
        assert!(shim.resolve_violation("C001"));
        assert_eq!(shim.unresolved_count(), 0);
        assert!(!shim.resolve_violation("nonexistent"));
    }

    #[test]
    fn test_generate_report() {
        let mut shim = ComplianceShim::new();
        shim.add_check(make_check("C001", true, Severity::High));
        shim.add_check(make_check("C002", false, Severity::Medium));
        shim.run_checks();

        let report = shim.generate_report();
        assert_eq!(report.total_checks, 2);
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.benchmark, "cis");
    }

    #[test]
    fn test_violation_counts() {
        let mut shim = ComplianceShim::new();
        shim.add_check(make_check("C001", false, Severity::High));
        shim.add_check(make_check("C002", false, Severity::High));
        shim.add_check(make_check("C003", false, Severity::Low));
        shim.run_checks();

        let counts = shim.violation_counts();
        assert_eq!(*counts.get(&Severity::High).unwrap_or(&0), 2);
        assert_eq!(*counts.get(&Severity::Low).unwrap_or(&0), 1);
    }

    #[test]
    fn test_meets_threshold() {
        let mut shim = ComplianceShim::new();
        shim.add_check(make_check("C001", true, Severity::High));
        shim.add_check(make_check("C002", true, Severity::Medium));
        shim.run_checks();

        assert!(shim.meets_threshold(90.0));
        assert!(shim.meets_threshold(100.0));
        assert!(!shim.meets_threshold(100.1));
    }

    #[test]
    fn test_empty_checks_score_100() {
        let mut shim = ComplianceShim::new();
        shim.run_checks();
        assert_eq!(shim.compliance_score, 100.0);
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = ComplianceShim::new();
        shim.add_check(make_check("C001", true, Severity::High));
        shim.add_check(make_check("C002", false, Severity::High));
        shim.run_checks();

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].value, 1.0);
        assert_eq!(metrics[1].value, 1.0);
        assert_eq!(metrics[3].value, 1.0);
    }
}
