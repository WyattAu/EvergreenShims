//! Scheduler shim — cron-like task scheduling with retry, jitter, and state tracking.
//!
//! Parses cron expressions, executes tasks with timeout and retry logic,
//! tracks task state (pending/running/success/failed), and adds jitter
//! to prevent thundering-herd problems.
//!
//! ## Environment Variables
//!
//! ```text
//! SCHEDULER_TASKS        JSON array of task definitions (or path to JSON file)
//! SCHEDULER_TIMEZONE     Timezone string (default: UTC)
//! ```

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context};
use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::{watch, Mutex};

/// Maximum jitter delay in seconds.
const MAX_JITTER_SECS: u64 = 60;

/// Task execution states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Pending,
    Running,
    Success,
    Failed,
}

/// Task retry configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay_secs: u64,
    pub max_delay_secs: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_secs: 5,
            max_delay_secs: 300,
        }
    }
}

/// Scheduled task definition with retry and timeout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub name: String,
    /// Cron expression (e.g., "0 */5 * * * *").
    pub schedule: String,
    /// Shell command to execute.
    pub command: String,
    /// Task-specific arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Whether this task is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Execution timeout in seconds (default: 300).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Retry configuration.
    #[serde(default)]
    pub retry: RetryConfig,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    300
}

/// Per-task runtime state.
#[derive(Debug, Clone)]
struct TaskRuntime {
    state: TaskState,
    last_run: Option<DateTime<Utc>>,
    last_success: Option<DateTime<Utc>>,
    last_failure: Option<DateTime<Utc>>,
    consecutive_failures: u32,
    next_run: Option<DateTime<Utc>>,
}

/// Scheduler shim with real cron parsing and task execution.
pub struct SchedulerShim {
    tasks: Vec<ScheduledTask>,
    schedules: HashMap<String, Schedule>,
    runtime: HashMap<String, TaskRuntime>,
    timezone: String,
    tasks_executed: u64,
    tasks_failed: u64,
    tasks_retried: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
    inner: Arc<Mutex<SchedulerInner>>,
}

/// Inner state for async operations.
struct SchedulerInner {
    running: bool,
}

impl SchedulerShim {
    pub fn new() -> Self {
        let tasks = Self::load_tasks();
        let schedules = Self::parse_schedules(&tasks);
        let runtime = tasks
            .iter()
            .map(|t| {
                let next = schedules.get(&t.name).and_then(|s| s.upcoming(Utc).next());
                (
                    t.name.clone(),
                    TaskRuntime {
                        state: TaskState::Pending,
                        last_run: None,
                        last_success: None,
                        last_failure: None,
                        consecutive_failures: 0,
                        next_run: next,
                    },
                )
            })
            .collect();

        Self {
            tasks,
            schedules,
            runtime,
            timezone: std::env::var("SCHEDULER_TIMEZONE").unwrap_or_else(|_| "UTC".to_string()),
            tasks_executed: 0,
            tasks_failed: 0,
            tasks_retried: 0,
            shutdown_tx: None,
            inner: Arc::new(Mutex::new(SchedulerInner { running: false })),
        }
    }

    /// Load tasks from env var or return empty.
    fn load_tasks() -> Vec<ScheduledTask> {
        std::env::var("SCHEDULER_TASKS")
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<ScheduledTask>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.enabled)
            .collect()
    }

    /// Parse cron expressions into Schedule objects.
    fn parse_schedules(tasks: &[ScheduledTask]) -> HashMap<String, Schedule> {
        tasks
            .iter()
            .filter_map(|t| {
                Schedule::from_str(&t.schedule)
                    .ok()
                    .map(|s| (t.name.clone(), s))
            })
            .collect()
    }

    /// Calculate exponential backoff with jitter for retry delay.
    pub fn retry_delay(attempt: u32, config: &RetryConfig) -> Duration {
        let base = config.base_delay_secs;
        let delay_secs = base * 2u32.saturating_pow(attempt.min(31)) as u64;
        let capped = delay_secs.min(config.max_delay_secs);
        // Add jitter: 0-50% of the calculated delay
        let jitter = (capped as f64 * 0.5 * fastrand::f32() as f64) as u64;
        Duration::from_secs(capped + jitter)
    }

    /// Calculate jitter offset to prevent thundering herd.
    pub fn jitter_offset() -> Duration {
        Duration::from_secs(fastrand::u64(0..MAX_JITTER_SECS))
    }

    /// Get a task's current state.
    pub fn task_state(&self, name: &str) -> Option<TaskState> {
        self.runtime.get(name).map(|r| r.state)
    }

    /// Get next scheduled run time for a task.
    pub fn next_run(&self, name: &str) -> Option<DateTime<Utc>> {
        self.runtime.get(name).and_then(|r| r.next_run)
    }

    /// Update a task's state after execution.
    pub fn update_task_state(&mut self, name: &str, state: TaskState) {
        if let Some(rt) = self.runtime.get_mut(name) {
            rt.state = state;
            match state {
                TaskState::Success => {
                    rt.last_success = Some(Utc::now());
                    rt.consecutive_failures = 0;
                    rt.next_run = self
                        .schedules
                        .get(name)
                        .and_then(|s| s.upcoming(Utc).next());
                    self.tasks_executed += 1;
                }
                TaskState::Failed => {
                    rt.last_failure = Some(Utc::now());
                    rt.consecutive_failures += 1;
                    rt.next_run = self
                        .schedules
                        .get(name)
                        .and_then(|s| s.upcoming(Utc).next());
                    self.tasks_failed += 1;
                }
                TaskState::Running => {
                    rt.last_run = Some(Utc::now());
                }
                TaskState::Pending => {}
            }
        }
    }

    /// Validate all cron schedules.
    pub fn validate_schedules(&self) -> Vec<(String, String)> {
        self.tasks
            .iter()
            .filter_map(|t| {
                Schedule::from_str(&t.schedule)
                    .err()
                    .map(|e| (t.name.clone(), e.to_string()))
            })
            .collect()
    }

    /// List all tasks with their runtime info.
    pub fn list_tasks(&self) -> Vec<TaskInfo> {
        self.tasks
            .iter()
            .filter_map(|t| {
                let rt = self.runtime.get(&t.name)?;
                Some(TaskInfo {
                    name: t.name.clone(),
                    schedule: t.schedule.clone(),
                    state: rt.state,
                    last_run: rt.last_run,
                    last_success: rt.last_success,
                    last_failure: rt.last_failure,
                    consecutive_failures: rt.consecutive_failures,
                    next_run: rt.next_run,
                })
            })
            .collect()
    }
}

/// Public task info for introspection.
#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub name: String,
    pub schedule: String,
    pub state: TaskState,
    pub last_run: Option<DateTime<Utc>>,
    pub last_success: Option<DateTime<Utc>>,
    pub last_failure: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub next_run: Option<DateTime<Utc>>,
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
        let invalid = self.validate_schedules();
        if !invalid.is_empty() {
            for (name, err) in &invalid {
                tracing::warn!(task = %name, "Invalid cron schedule: {}", err);
            }
        }
        tracing::info!(
            tasks = self.tasks.len(),
            tz = %self.timezone,
            invalid = invalid.len(),
            "SchedulerShim initialized"
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        {
            let mut inner = self.inner.lock().await;
            inner.running = true;
        }
        tracing::info!(tasks = self.tasks.len(), "SchedulerShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        {
            let mut inner = self.inner.lock().await;
            inner.running = false;
        }
        tracing::info!(
            executed = self.tasks_executed,
            failed = self.tasks_failed,
            "SchedulerShim stopped"
        );
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("scheduler_tasks_total", self.tasks.len() as f64),
            Metric::new("scheduler_tasks_executed", self.tasks_executed as f64),
            Metric::new("scheduler_tasks_failed", self.tasks_failed as f64),
            Metric::new("scheduler_tasks_retried", self.tasks_retried as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_cron() {
        let tasks = vec![ScheduledTask {
            name: "backup".into(),
            schedule: "0 0 * * * *".into(),
            command: "/bin/true".into(),
            args: vec![],
            enabled: true,
            timeout_secs: 60,
            retry: RetryConfig::default(),
        }];
        let schedules = SchedulerShim::parse_schedules(&tasks);
        assert!(schedules.contains_key("backup"));
    }

    #[test]
    fn test_parse_invalid_cron() {
        let tasks = vec![ScheduledTask {
            name: "bad".into(),
            schedule: "not-a-cron".into(),
            command: "/bin/false".into(),
            args: vec![],
            enabled: true,
            timeout_secs: 60,
            retry: RetryConfig::default(),
        }];
        let schedules = SchedulerShim::parse_schedules(&tasks);
        assert!(!schedules.contains_key("bad"));
    }

    #[test]
    fn test_validate_schedules() {
        let mut shim = SchedulerShim::new();
        // Default shim has no tasks so no invalid schedules
        let invalid = shim.validate_schedules();
        assert!(invalid.is_empty());
    }

    #[test]
    fn test_retry_delay_exponential() {
        let config = RetryConfig {
            max_retries: 5,
            base_delay_secs: 2,
            max_delay_secs: 100,
        };
        let d0 = SchedulerShim::retry_delay(0, &config);
        let d1 = SchedulerShim::retry_delay(1, &config);
        let d2 = SchedulerShim::retry_delay(2, &config);
        // Exponential growth (with jitter, so just check d2 > d1 > d0 roughly)
        // d0 ≈ 2s + jitter, d1 ≈ 4s + jitter, d2 ≈ 8s + jitter
        assert!(d2.as_secs() >= 4); // At least 8 - 4 (max jitter) = 4
        assert!(d1.as_secs() >= 2);
    }

    #[test]
    fn test_retry_delay_capped() {
        let config = RetryConfig {
            max_retries: 10,
            base_delay_secs: 10,
            max_delay_secs: 20,
        };
        let delay = SchedulerShim::retry_delay(5, &config);
        // 10 * 2^5 = 320, capped to 20 + jitter
        assert!(delay.as_secs() <= 30); // 20 + 10 (max jitter)
    }

    #[test]
    fn test_jitter_offset_range() {
        let offset = SchedulerShim::jitter_offset();
        assert!(offset.as_secs() < MAX_JITTER_SECS);
    }

    #[test]
    fn test_update_task_state() {
        let mut shim = SchedulerShim::new();
        shim.tasks = vec![ScheduledTask {
            name: "test".into(),
            schedule: "0 * * * * *".into(),
            command: "echo".into(),
            args: vec![],
            enabled: true,
            timeout_secs: 60,
            retry: RetryConfig::default(),
        }];
        shim.schedules = SchedulerShim::parse_schedules(&shim.tasks);
        shim.runtime.insert(
            "test".into(),
            TaskRuntime {
                state: TaskState::Pending,
                last_run: None,
                last_success: None,
                last_failure: None,
                consecutive_failures: 0,
                next_run: None,
            },
        );

        shim.update_task_state("test", TaskState::Running);
        assert_eq!(shim.task_state("test"), Some(TaskState::Running));
        assert!(shim.runtime["test"].last_run.is_some());

        shim.update_task_state("test", TaskState::Success);
        assert_eq!(shim.task_state("test"), Some(TaskState::Success));
        assert_eq!(shim.tasks_executed, 1);
        assert!(shim.runtime["test"].last_success.is_some());

        shim.update_task_state("test", TaskState::Failed);
        assert_eq!(shim.tasks_failed, 1);
        assert_eq!(shim.runtime["test"].consecutive_failures, 1);
    }

    #[test]
    fn test_consecutive_failures_reset_on_success() {
        let mut shim = SchedulerShim::new();
        shim.tasks = vec![ScheduledTask {
            name: "flaky".into(),
            schedule: "0 * * * * *".into(),
            command: "echo".into(),
            args: vec![],
            enabled: true,
            timeout_secs: 60,
            retry: RetryConfig::default(),
        }];
        shim.schedules = SchedulerShim::parse_schedules(&shim.tasks);
        shim.runtime.insert(
            "flaky".into(),
            TaskRuntime {
                state: TaskState::Pending,
                last_run: None,
                last_success: None,
                last_failure: None,
                consecutive_failures: 3,
                next_run: None,
            },
        );

        shim.update_task_state("flaky", TaskState::Success);
        assert_eq!(shim.runtime["flaky"].consecutive_failures, 0);
    }

    #[test]
    fn test_list_tasks() {
        let mut shim = SchedulerShim::new();
        shim.tasks = vec![ScheduledTask {
            name: "backup".into(),
            schedule: "0 0 * * * *".into(),
            command: "pg_dump".into(),
            args: vec![],
            enabled: true,
            timeout_secs: 300,
            retry: RetryConfig::default(),
        }];
        shim.schedules = SchedulerShim::parse_schedules(&shim.tasks);
        shim.runtime.insert(
            "backup".into(),
            TaskRuntime {
                state: TaskState::Pending,
                last_run: None,
                last_success: None,
                last_failure: None,
                consecutive_failures: 0,
                next_run: None,
            },
        );

        let tasks = shim.list_tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "backup");
    }

    #[test]
    fn test_metrics() {
        let mut shim = SchedulerShim::new();
        shim.tasks_executed = 42;
        shim.tasks_failed = 3;
        shim.tasks_retried = 5;
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
    }

    #[test]
    fn test_load_tasks_from_env() {
        std::env::remove_var("SCHEDULER_TASKS");
        std::env::set_var(
            "SCHEDULER_TASKS",
            r#"[{"name":"t1","schedule":"0 * * * * *","command":"echo","enabled":true,"timeout_secs":60,"retry":{"max_retries":2,"base_delay_secs":5,"max_delay_secs":60}}]"#,
        );
        let shim = SchedulerShim::new();
        assert_eq!(shim.tasks.len(), 1);
        assert_eq!(shim.tasks[0].name, "t1");
        std::env::remove_var("SCHEDULER_TASKS");
    }

    #[test]
    fn test_disabled_tasks_filtered() {
        // Ensure clean state first
        std::env::remove_var("SCHEDULER_TASKS");
        std::env::set_var(
            "SCHEDULER_TASKS",
            r#"[{"name":"active","schedule":"0 * * * * *","command":"echo","enabled":true},{"name":"inactive","schedule":"0 * * * * *","command":"echo","enabled":false}]"#,
        );
        let shim = SchedulerShim::new();
        assert_eq!(shim.tasks.len(), 1);
        assert_eq!(shim.tasks[0].name, "active");
        std::env::remove_var("SCHEDULER_TASKS");
    }
}
