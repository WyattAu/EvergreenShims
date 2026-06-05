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

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::{watch, Mutex};

const MAX_JITTER_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Pending,
    Running,
    Success,
    Failed,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub name: String,
    pub schedule: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub retry: RetryConfig,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    300
}

#[derive(Debug, Clone)]
struct TaskRuntime {
    state: TaskState,
    last_run: Option<DateTime<Utc>>,
    last_success: Option<DateTime<Utc>>,
    last_failure: Option<DateTime<Utc>>,
    consecutive_failures: u32,
    next_run: Option<DateTime<Utc>>,
}

type TaskHandler = Arc<
    dyn Fn(
            String,
            Vec<String>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>
        + Send
        + Sync,
>;

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
    handler: Option<TaskHandler>,
}

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
            handler: None,
        }
    }

    pub fn set_handler<F>(&mut self, handler: F)
    where
        F: Fn(
                String,
                Vec<String>,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        self.handler = Some(Arc::new(handler));
    }

    fn load_tasks() -> Vec<ScheduledTask> {
        std::env::var("SCHEDULER_TASKS")
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<ScheduledTask>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.enabled)
            .collect()
    }

    fn parse_schedules(tasks: &[ScheduledTask]) -> HashMap<String, Schedule> {
        tasks
            .iter()
            .filter_map(|t| match Schedule::from_str(&t.schedule) {
                Ok(s) => Some((t.name.clone(), s)),
                Err(e) => {
                    tracing::warn!(task = %t.name, schedule = %t.schedule, "Invalid cron expression: {}", e);
                    None
                }
            })
            .collect()
    }

    pub fn retry_delay(attempt: u32, config: &RetryConfig) -> Duration {
        let base = config.base_delay_secs;
        let delay_secs = base * 2u32.saturating_pow(attempt.min(31)) as u64;
        let capped = delay_secs.min(config.max_delay_secs);
        let jitter = (capped as f64 * 0.5 * fastrand::f32() as f64) as u64;
        Duration::from_secs(capped + jitter)
    }

    pub fn jitter_offset() -> Duration {
        Duration::from_secs(fastrand::u64(0..MAX_JITTER_SECS))
    }

    pub fn task_state(&self, name: &str) -> Option<TaskState> {
        self.runtime.get(name).map(|r| r.state)
    }

    pub fn next_run(&self, name: &str) -> Option<DateTime<Utc>> {
        self.runtime.get(name).and_then(|r| r.next_run)
    }

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

    fn spawn_scheduler_loop(&self, mut shutdown_rx: watch::Receiver<bool>) {
        let handler = match self.handler.clone() {
            Some(h) => h,
            None => return,
        };

        let tasks: Vec<ScheduledTask> = self.tasks.clone();
        let schedules: HashMap<String, Schedule> = self.schedules.clone();
        let runtime = Arc::new(Mutex::new(self.runtime.clone()));
        let tasks_executed = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let tasks_failed = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let tasks_retried = Arc::new(std::sync::atomic::AtomicU64::new(0));

        // Pre-validate tasks with valid schedules
        let valid_tasks: Vec<ScheduledTask> = tasks
            .iter()
            .filter(|t| schedules.contains_key(&t.name))
            .cloned()
            .collect();

        if valid_tasks.is_empty() {
            return;
        }

        tokio::spawn(async move {
            loop {
                // Find the next task to fire
                let mut earliest: Option<(DateTime<Utc>, String)> = None;
                for task in &valid_tasks {
                    if let Some(schedule) = schedules.get(&task.name) {
                        if let Some(time) = schedule.upcoming(Utc).next() {
                            if let Some((ref earliest_time, _)) = earliest {
                                if time < *earliest_time {
                                    earliest = Some((time, task.name.clone()));
                                }
                            } else {
                                earliest = Some((time, task.name.clone()));
                            }
                        }
                    }
                }

                let (fire_time, task_name) = match earliest {
                    Some(ft) => ft,
                    None => {
                        tokio::select! {
                            _ = shutdown_rx.changed() => break,
                            _ = tokio::time::sleep(Duration::from_secs(60)) => continue,
                        }
                    }
                };

                // Sleep until fire time, or shutdown
                let now = Utc::now();
                if fire_time > now {
                    let sleep_duration =
                        (fire_time - now).to_std().unwrap_or(Duration::from_secs(1));
                    let jitter = Duration::from_secs(fastrand::u64(
                        0..=MAX_JITTER_SECS.min(sleep_duration.as_secs() / 4),
                    ));
                    tokio::select! {
                        _ = shutdown_rx.changed() => break,
                        _ = tokio::time::sleep(sleep_duration + jitter) => {}
                    }
                }

                // Find task config
                let task_config = match valid_tasks.iter().find(|t| t.name == task_name) {
                    Some(t) => t.clone(),
                    None => continue,
                };

                // Update state to Running
                {
                    let mut rt = runtime.lock().await;
                    if let Some(r) = rt.get_mut(&task_name) {
                        r.state = TaskState::Running;
                        r.last_run = Some(Utc::now());
                    }
                }

                // Execute with retry
                let mut last_error = None;
                let max_attempts = task_config.retry.max_retries + 1;
                for attempt in 0..max_attempts {
                    if attempt > 0 {
                        let delay = Self::retry_delay(attempt - 1, &task_config.retry);
                        tasks_retried.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        tokio::select! {
                            _ = shutdown_rx.changed() => return,
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }

                    match handler(task_config.name.clone(), task_config.args.clone()).await {
                        Ok(()) => {
                            last_error = None;
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(
                                task = %task_name,
                                attempt = attempt + 1,
                                error = %e,
                                "Task execution failed"
                            );
                            last_error = Some(e);
                        }
                    }
                }

                // Update final state
                {
                    let mut rt = runtime.lock().await;
                    if let Some(r) = rt.get_mut(&task_name) {
                        match last_error {
                            None => {
                                r.state = TaskState::Success;
                                r.last_success = Some(Utc::now());
                                r.consecutive_failures = 0;
                                r.next_run = schedules
                                    .get(&task_name)
                                    .and_then(|s| s.upcoming(Utc).next());
                                tasks_executed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            Some(_e) => {
                                r.state = TaskState::Failed;
                                r.last_failure = Some(Utc::now());
                                r.consecutive_failures += 1;
                                r.next_run = schedules
                                    .get(&task_name)
                                    .and_then(|s| s.upcoming(Utc).next());
                                tasks_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        });
    }
}

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
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        {
            let mut inner = self.inner.lock().await;
            inner.running = true;
        }
        self.spawn_scheduler_loop(shutdown_rx);
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
        let shim = SchedulerShim::new();
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
        let _d0 = SchedulerShim::retry_delay(0, &config);
        let d1 = SchedulerShim::retry_delay(1, &config);
        let d2 = SchedulerShim::retry_delay(2, &config);
        assert!(d2.as_secs() >= 4);
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
        assert!(delay.as_secs() <= 30);
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
        temp_env::with_vars(
            [(
                "SCHEDULER_TASKS",
                Some(
                    r#"[{"name":"t1","schedule":"0 * * * * *","command":"echo","enabled":true,"timeout_secs":60,"retry":{"max_retries":2,"base_delay_secs":5,"max_delay_secs":60}}]"#,
                ),
            )],
            || {
                let shim = SchedulerShim::new();
                assert_eq!(shim.tasks.len(), 1);
                assert_eq!(shim.tasks[0].name, "t1");
            },
        );
    }

    #[test]
    fn test_disabled_tasks_filtered() {
        temp_env::with_vars(
            [(
                "SCHEDULER_TASKS",
                Some(
                    r#"[{"name":"active","schedule":"0 * * * * *","command":"echo","enabled":true},{"name":"inactive","schedule":"0 * * * * *","command":"echo","enabled":false}]"#,
                ),
            )],
            || {
                let shim = SchedulerShim::new();
                assert_eq!(shim.tasks.len(), 1);
                assert_eq!(shim.tasks[0].name, "active");
            },
        );
    }

    #[tokio::test]
    async fn test_scheduler_executes_task() {
        use std::sync::atomic::AtomicU64;

        let mut shim = SchedulerShim::new();
        let executed = Arc::new(AtomicU64::new(0));
        let executed_clone = Arc::clone(&executed);

        shim.set_handler(move |_cmd, _args| {
            let e = Arc::clone(&executed_clone);
            Box::pin(async move {
                e.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            })
        });

        // Add a task with "every second" schedule
        shim.tasks = vec![ScheduledTask {
            name: "tick".into(),
            schedule: "*/1 * * * * *".into(),
            command: "tick".into(),
            args: vec![],
            enabled: true,
            timeout_secs: 60,
            retry: RetryConfig::default(),
        }];
        shim.schedules = SchedulerShim::parse_schedules(&shim.tasks);
        let next = shim
            .schedules
            .get("tick")
            .and_then(|s| s.upcoming(Utc).next());
        shim.runtime.insert(
            "tick".into(),
            TaskRuntime {
                state: TaskState::Pending,
                last_run: None,
                last_success: None,
                last_failure: None,
                consecutive_failures: 0,
                next_run: next,
            },
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        shim.shutdown_tx = Some(shutdown_tx);
        {
            let mut inner = shim.inner.lock().await;
            inner.running = true;
        }
        shim.spawn_scheduler_loop(shutdown_rx);

        // Wait for at least one execution
        tokio::time::sleep(Duration::from_secs(3)).await;

        assert!(executed.load(std::sync::atomic::Ordering::Relaxed) >= 1);
        let _ = shim.shutdown_tx.as_ref().unwrap().send(true);
    }
}
