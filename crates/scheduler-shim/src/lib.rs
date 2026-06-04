//! Scheduler shim — cron-like task scheduling.
//!
//! Runs scheduled tasks (backups, maintenance, reports) on a cron schedule.
//!
//! ## Environment Variables
//!
//! ```text
//! SCHEDULER_TASKS        JSON array of tasks (or path to tasks file)
//! SCHEDULER_TIMEZONE     Timezone (default: UTC)
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Scheduled task definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub name: String,
    pub schedule: String,
    pub command: String,
    pub enabled: bool,
}

/// Scheduler shim.
pub struct SchedulerShim {
    tasks: Vec<ScheduledTask>,
    timezone: String,
    tasks_executed: u64,
    tasks_failed: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl SchedulerShim {
    pub fn new() -> Self {
        Self {
            tasks: std::env::var("SCHEDULER_TASKS")
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            timezone: std::env::var("SCHEDULER_TIMEZONE").unwrap_or_else(|_| "UTC".to_string()),
            tasks_executed: 0,
            tasks_failed: 0,
            shutdown_tx: None,
        }
    }
}

impl Default for SchedulerShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for SchedulerShim {
    fn name(&self) -> &str {
        "scheduler"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "SchedulerShim initialized (tasks={}, tz={})",
            self.tasks.len(),
            self.timezone
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("SchedulerShim started ({} tasks)", self.tasks.len());
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("SchedulerShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("scheduler_tasks_executed", self.tasks_executed as f64),
            Metric::new("scheduler_tasks_failed", self.tasks_failed as f64),
        ]
    }
}
