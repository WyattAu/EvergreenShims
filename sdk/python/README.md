# EvergreenShims Python SDK

Python client library for the EvergreenShims management API.

## Installation

```bash
pip install evergreen-shims
```

## Usage

```python
from evergreen_shims import Client

client = Client("http://localhost:50051")

# Get shim status
status = client.status()
print(f"Health: {status.health}, Version: {status.version}")

# Get metrics
metrics = client.metrics()
for m in metrics.metrics:
    print(f"Metric: {m.name} = {m.value}")

# Health checks
live = client.health_liveness()
ready = client.health_readiness()

# Config reload
reload = client.config_reload()
print(reload.message)

# Backup operations
backups = client.backup_list()
print(f"Backups: {len(backups.backups)}")

trigger = client.backup_trigger()
print(f"Backup triggered: {trigger.success}")

# Migration operations
mig_status = client.migration_status()
print(f"Migration version: {mig_status.current_version}")

apply = client.migration_apply()
print(f"Applied: {apply.migrations_applied} migrations")

rollback = client.migration_rollback()
print(f"Rolled back to version: {rollback.current_version}")
```

## Configuration

```python
from evergreen_shims import Client

# Custom timeout
client = Client("http://localhost:50051", timeout=10.0)
```

## API Methods

| Method | Description |
|--------|-------------|
| `status()` | Get shim health and version |
| `metrics()` | Get all collected metrics |
| `health_liveness()` | Liveness check |
| `health_readiness()` | Readiness check |
| `config_reload(path)` | Trigger config reload |
| `backup_list()` | List available backups |
| `backup_trigger()` | Trigger a new backup |
| `migration_status()` | Get migration status |
| `migration_apply()` | Apply pending migrations |
| `migration_rollback()` | Rollback last migration |

## Exceptions

| Exception | Description |
|-----------|-------------|
| `EvergreenShimError` | Base exception |
| `APIError` | Non-success HTTP status |
| `ConnectionError` | Connection failure |
| `TimeoutError` | Request timeout |
