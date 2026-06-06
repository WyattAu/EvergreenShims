# scheduler-shim

Cron-like task scheduling with retry, jitter, and state tracking. Parses cron expressions, executes tasks with timeout and retry logic, and tracks task state.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `SCHEDULER_TASKS` | JSON array of task definitions (or path to JSON file) | — |
| `SCHEDULER_TIMEZONE` | Timezone string | `UTC` |

## Task Definition Format

```json
[
  {
    "name": "backup",
    "cron": "0 2 * * *",
    "command": "/usr/local/bin/backup.sh",
    "timeout_secs": 3600,
    "retry": {
      "max_retries": 3,
      "base_delay_secs": 5,
      "max_delay_secs": 300
    }
  }
]
```

## Usage

```rust
use scheduler_shim::SchedulerShim;
use shim_core::Capability;

let mut shim = SchedulerShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Cron expression parsing with timezone support.
- Configurable retry with exponential backoff.
- Jitter to prevent thundering-herd problems (up to 60s).
- Task states: Pending, Running, Success, Failed.

## Metrics Exposed

- `scheduler_tasks_total` – Total registered tasks.
- `scheduler_task_runs_total` – Total task executions.
- `scheduler_task_failures_total` – Total task failures.
- `scheduler_last_run_timestamp` – Unix timestamp of last task run.

## Testing

```bash
cargo test -p scheduler-shim
```
