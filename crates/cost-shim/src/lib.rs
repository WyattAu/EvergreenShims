#![allow(dead_code)]
//! Cost shim — resource tracking per tenant.
//!
//! Tracks resource usage (CPU, memory, storage, I/O) per tenant for billing.
//!
//! ## Environment Variables
//!
//! ```text
//! COST_TRACKING_ENABLED  Enable tracking (default: true)
//! COST_TENANT_KEY        Header/key for tenant identification
//! COST_REPORT_SCHEDULE   Report schedule (default: daily)
//! COST_BUDGET_DEFAULT     Default budget per tenant (default: 100.0)
//! COST_ALERT_THRESHOLD   Alert at this % of budget (default: 80)
//! COST_CURRENCY          Currency code (default: USD)
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Resource types that can be metered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResourceType {
    Cpu,
    Memory,
    Storage,
    NetworkIn,
    NetworkOut,
    Requests,
    DatabaseReads,
    DatabaseWrites,
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Memory => write!(f, "memory"),
            Self::Storage => write!(f, "storage"),
            Self::NetworkIn => write!(f, "network_in"),
            Self::NetworkOut => write!(f, "network_out"),
            Self::Requests => write!(f, "requests"),
            Self::DatabaseReads => write!(f, "db_reads"),
            Self::DatabaseWrites => write!(f, "db_writes"),
        }
    }
}

/// A resource usage measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub resource_type: ResourceType,
    pub amount: f64,
    pub unit: String,
    pub cost_per_unit: f64,
    pub recorded_at: String,
}

impl ResourceUsage {
    pub fn total_cost(&self) -> f64 {
        self.amount * self.cost_per_unit
    }
}

/// A budget for a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    pub tenant_id: String,
    pub limit: f64,
    pub spent: f64,
    pub currency: String,
    pub period_start: String,
    pub period_end: String,
}

impl Budget {
    pub fn remaining(&self) -> f64 {
        (self.limit - self.spent).max(0.0)
    }

    pub fn usage_percent(&self) -> f64 {
        if self.limit > 0.0 {
            (self.spent / self.limit) * 100.0
        } else {
            100.0
        }
    }

    pub fn is_over_budget(&self) -> bool {
        self.spent > self.limit
    }

    pub fn is_near_limit(&self, threshold_percent: f64) -> bool {
        self.usage_percent() >= threshold_percent
    }
}

/// Cost projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostProjection {
    pub tenant_id: String,
    pub current_cost: f64,
    pub projected_monthly: f64,
    pub budget_limit: f64,
    pub projected_over_budget: bool,
    pub days_remaining: i64,
}

/// Cost shim.
pub struct CostShim {
    enabled: bool,
    tenant_key: String,
    report_schedule: String,
    budget_default: f64,
    alert_threshold: f64,
    currency: String,
    tenants_tracked: u64,
    resources_tracked: u64,
    budgets: HashMap<String, Budget>,
    usage_log: Vec<ResourceUsage>,
    alert_threshold_triggered: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CostShim {
    pub fn new() -> Self {
        Self {
            enabled: std::env::var("COST_TRACKING_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            tenant_key: std::env::var("COST_TENANT_KEY")
                .unwrap_or_else(|_| "X-Tenant-ID".to_string()),
            report_schedule: std::env::var("COST_REPORT_SCHEDULE")
                .unwrap_or_else(|_| "daily".to_string()),
            budget_default: std::env::var("COST_BUDGET_DEFAULT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100.0),
            alert_threshold: std::env::var("COST_ALERT_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(80.0),
            currency: std::env::var("COST_CURRENCY").unwrap_or_else(|_| "USD".to_string()),
            tenants_tracked: 0,
            resources_tracked: 0,
            budgets: HashMap::new(),
            usage_log: Vec::new(),
            alert_threshold_triggered: 0,
            shutdown_tx: None,
        }
    }

    /// Create a budget for a tenant.
    pub fn create_budget(&mut self, tenant_id: &str, limit: f64) {
        let now = chrono::Utc::now();
        self.budgets.insert(
            tenant_id.to_string(),
            Budget {
                tenant_id: tenant_id.to_string(),
                limit,
                spent: 0.0,
                currency: self.currency.clone(),
                period_start: now.to_rfc3339(),
                period_end: (now + chrono::Duration::days(30)).to_rfc3339(),
            },
        );
        self.tenants_tracked = self.budgets.len() as u64;
    }

    /// Record resource usage and bill it against a tenant's budget.
    pub fn record_usage(
        &mut self,
        tenant_id: &str,
        resource_type: ResourceType,
        amount: f64,
        unit: &str,
        cost_per_unit: f64,
    ) -> f64 {
        let cost = amount * cost_per_unit;

        if let Some(budget) = self.budgets.get_mut(tenant_id) {
            budget.spent += cost;
        }

        let usage = ResourceUsage {
            resource_type,
            amount,
            unit: unit.to_string(),
            cost_per_unit,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        };
        self.usage_log.push(usage);
        self.resources_tracked += 1;

        cost
    }

    /// Get a budget for a tenant.
    pub fn get_budget(&self, tenant_id: &str) -> Option<&Budget> {
        self.budgets.get(tenant_id)
    }

    /// Check if tenant is over budget.
    pub fn is_over_budget(&self, tenant_id: &str) -> bool {
        self.budgets
            .get(tenant_id)
            .map(|b| b.is_over_budget())
            .unwrap_or(false)
    }

    /// Check budget alerts for all tenants. Returns list of tenants over threshold.
    pub fn check_alerts(&mut self) -> Vec<String> {
        let mut alerted = Vec::new();
        for (tenant_id, budget) in &self.budgets {
            if budget.is_near_limit(self.alert_threshold) {
                alerted.push(tenant_id.clone());
                self.alert_threshold_triggered += 1;
            }
        }
        alerted
    }

    /// Project costs for a tenant based on current usage rate.
    pub fn project_cost(&self, tenant_id: &str) -> Option<CostProjection> {
        let budget = self.budgets.get(tenant_id)?;

        let now = chrono::Utc::now();
        let period_start = budget
            .period_start
            .parse::<chrono::DateTime<chrono::Utc>>()
            .ok()?;
        let period_end = budget
            .period_end
            .parse::<chrono::DateTime<chrono::Utc>>()
            .ok()?;

        let elapsed_secs = (now - period_start).num_seconds().max(1) as f64;
        let _total_period_secs = (period_end - period_start).num_seconds() as f64;
        let rate = budget.spent / elapsed_secs;
        let projected_monthly = rate * 30.0 * 86400.0;

        let days_remaining = ((period_end - now).num_days()).max(0);

        Some(CostProjection {
            tenant_id: tenant_id.to_string(),
            current_cost: budget.spent,
            projected_monthly,
            budget_limit: budget.limit,
            projected_over_budget: projected_monthly > budget.limit,
            days_remaining,
        })
    }

    /// Get total spend across all tenants.
    pub fn total_spend(&self) -> f64 {
        self.budgets.values().map(|b| b.spent).sum()
    }

    /// Get tenant count.
    pub fn tenant_count(&self) -> usize {
        self.budgets.len()
    }

    /// Get usage count.
    pub fn usage_count(&self) -> usize {
        self.usage_log.len()
    }

    /// Reset all budgets (new billing period).
    pub fn reset_budgets(&mut self) {
        for budget in self.budgets.values_mut() {
            budget.spent = 0.0;
            let now = chrono::Utc::now();
            budget.period_start = now.to_rfc3339();
            budget.period_end = (now + chrono::Duration::days(30)).to_rfc3339();
        }
    }
}

impl Default for CostShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for CostShim {
    fn name(&self) -> &str {
        "cost"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "CostShim initialized (enabled={}, key={})",
            self.enabled,
            self.tenant_key
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("CostShim started (enabled={})", self.enabled);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("CostShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("cost_tenants_tracked", self.tenants_tracked as f64),
            Metric::new("cost_resources_tracked", self.resources_tracked as f64),
            Metric::new("cost_total_spend", self.total_spend()),
            Metric::new(
                "cost_alerts_triggered",
                self.alert_threshold_triggered as f64,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_type_display() {
        assert_eq!(ResourceType::Cpu.to_string(), "cpu");
        assert_eq!(ResourceType::Memory.to_string(), "memory");
        assert_eq!(ResourceType::DatabaseReads.to_string(), "db_reads");
    }

    #[test]
    fn test_resource_usage_total_cost() {
        let usage = ResourceUsage {
            resource_type: ResourceType::Cpu,
            amount: 100.0,
            unit: "hours".to_string(),
            cost_per_unit: 0.05,
            recorded_at: "2025-01-01T00:00:00Z".to_string(),
        };
        assert!((usage.total_cost() - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_budget_remaining() {
        let budget = Budget {
            tenant_id: "t1".to_string(),
            limit: 100.0,
            spent: 30.0,
            currency: "USD".to_string(),
            period_start: "2025-01-01T00:00:00Z".to_string(),
            period_end: "2025-02-01T00:00:00Z".to_string(),
        };
        assert!((budget.remaining() - 70.0).abs() < 0.01);
    }

    #[test]
    fn test_budget_usage_percent() {
        let budget = Budget {
            tenant_id: "t1".to_string(),
            limit: 100.0,
            spent: 75.0,
            currency: "USD".to_string(),
            period_start: "2025-01-01T00:00:00Z".to_string(),
            period_end: "2025-02-01T00:00:00Z".to_string(),
        };
        assert!((budget.usage_percent() - 75.0).abs() < 0.01);
    }

    #[test]
    fn test_budget_over_budget() {
        let budget = Budget {
            tenant_id: "t1".to_string(),
            limit: 50.0,
            spent: 60.0,
            currency: "USD".to_string(),
            period_start: "2025-01-01T00:00:00Z".to_string(),
            period_end: "2025-02-01T00:00:00Z".to_string(),
        };
        assert!(budget.is_over_budget());
    }

    #[test]
    fn test_budget_near_limit() {
        let budget = Budget {
            tenant_id: "t1".to_string(),
            limit: 100.0,
            spent: 85.0,
            currency: "USD".to_string(),
            period_start: "2025-01-01T00:00:00Z".to_string(),
            period_end: "2025-02-01T00:00:00Z".to_string(),
        };
        assert!(budget.is_near_limit(80.0));
        assert!(!budget.is_near_limit(90.0));
    }

    #[test]
    fn test_create_budget_and_get() {
        let mut shim = CostShim::new();
        shim.create_budget("tenant-a", 200.0);

        let budget = shim.get_budget("tenant-a").unwrap();
        assert_eq!(budget.limit, 200.0);
        assert_eq!(budget.spent, 0.0);
        assert_eq!(shim.tenant_count(), 1);
    }

    #[test]
    fn test_record_usage() {
        let mut shim = CostShim::new();
        shim.create_budget("tenant-a", 100.0);

        let cost = shim.record_usage("tenant-a", ResourceType::Cpu, 10.0, "hours", 0.05);
        assert!((cost - 0.5).abs() < 0.01);

        let budget = shim.get_budget("tenant-a").unwrap();
        assert!((budget.spent - 0.5).abs() < 0.01);
        assert_eq!(shim.resources_tracked, 1);
    }

    #[test]
    fn test_is_over_budget() {
        let mut shim = CostShim::new();
        shim.create_budget("tenant-a", 10.0);
        shim.record_usage("tenant-a", ResourceType::Memory, 1000.0, "GB", 0.02);

        assert!(shim.is_over_budget("tenant-a"));
        assert!(!shim.is_over_budget("nonexistent"));
    }

    #[test]
    fn test_check_alerts() {
        let mut shim = CostShim {
            alert_threshold: 80.0,
            ..CostShim::new()
        };
        shim.create_budget("tenant-a", 100.0);
        shim.record_usage("tenant-a", ResourceType::Cpu, 500.0, "hours", 0.20);

        let alerted = shim.check_alerts();
        assert_eq!(alerted.len(), 1);
        assert_eq!(shim.alert_threshold_triggered, 1);
    }

    #[test]
    fn test_project_cost() {
        let mut shim = CostShim::new();
        shim.create_budget("tenant-a", 100.0);
        shim.record_usage("tenant-a", ResourceType::Cpu, 100.0, "hours", 0.01);

        let projection = shim.project_cost("tenant-a").unwrap();
        assert!(projection.projected_monthly > 0.0);
        assert_eq!(projection.tenant_id, "tenant-a");
    }

    #[test]
    fn test_total_spend() {
        let mut shim = CostShim::new();
        shim.create_budget("t1", 100.0);
        shim.create_budget("t2", 100.0);
        shim.record_usage("t1", ResourceType::Cpu, 10.0, "h", 0.05);
        shim.record_usage("t2", ResourceType::Memory, 5.0, "GB", 0.10);

        assert!((shim.total_spend() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_reset_budgets() {
        let mut shim = CostShim::new();
        shim.create_budget("tenant-a", 100.0);
        shim.record_usage("tenant-a", ResourceType::Cpu, 100.0, "h", 0.05);

        assert!(shim.get_budget("tenant-a").unwrap().spent > 0.0);
        shim.reset_budgets();
        assert_eq!(shim.get_budget("tenant-a").unwrap().spent, 0.0);
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = CostShim::new();
        shim.create_budget("t1", 100.0);
        shim.create_budget("t2", 100.0);
        shim.record_usage("t1", ResourceType::Cpu, 10.0, "h", 0.05);

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].value, 2.0);
        assert_eq!(metrics[1].value, 1.0);
        assert!((metrics[2].value - 0.5).abs() < 0.01);
    }
}
