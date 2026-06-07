# Hot-Reload Documentation

## Overview

EvergreenShims supports live configuration reloading without process restart. The hot-reload mechanism watches the configuration file for changes, computes a SHA-256 hash of the file contents, and triggers a reload when the hash differs from the previously loaded version.

Reload is triggered by:
1. **SIGHUP signal** — send `kill -HUP <pid>` to trigger an immediate reload.
2. **File change detection** — the `notify` crate watches the config file for write events.
3. **Manual trigger** — use `shimctl config reload` to trigger via the management API.

## TOML Configuration File Format

The configuration file (`shim.toml`) uses the following structure:

```toml
version = "1.0"

[health]
liveness_cmd = "exec:true"
readiness_cmd = "exec:true"
listen = "0.0.0.0:9101"
interval_secs = 10
timeout_secs = 5

[process]
command = "my-app"
args = ["--verbose", "--port=8080"]
working_dir = "/app"
shutdown_timeout_secs = 30

[vault]
addr = "https://vault.example.com:8200"
role = "shim-role"
secret = "secret/data/db"
rotation_secs = 3600

[backup]
schedule = "0 2 * * *"
storage = "s3"
retention_days = 30
database = "mydb"
prefix = "backups/"

[migration]
dir = "./migrations"
database = "mydb"
auto_migrate = false
db_host = "127.0.0.1"
db_port = 5432
db_user = "postgres"
db_password = ""
db_type = "postgres"

[tls]
provider = "letsencrypt"
domain = "example.com"
email = "admin@example.com"
renew_before_secs = 259200

[failover]
primary = "10.0.0.1:5432"
replica = "10.0.0.2:5432"
check_interval_secs = 5
timeout_secs = 10
failure_threshold = 3
webhook = "https://hooks.example.com"
db_type = "postgres"

[replication]
primary = "10.0.0.1:5432"
replicas = ["10.0.0.2:5432", "10.0.0.3:5432"]
mode = "synchronous"
check_interval_secs = 10
db_type = "postgres"
slot_name = "my_slot"

[audit]
database = "mydb"
tables = ["users", "orders"]
format = "json"

[resource_quota]
max_memory_bytes = 1073741824
max_cpu_percent = 80.0
max_open_files = 1024
max_connections = 100

[[tenants]]
tenant_id = "tenant-a"
max_memory_bytes = 1073741824
max_cpu_percent = 80.0
max_requests_per_sec = 500
allowed_features = ["feature-x", "feature-y"]
reset_period_secs = 1
```

## Environment Variable Overrides

Every configuration field can be overridden via environment variables (12-factor app pattern). File values are loaded first, then env vars override.

| Environment Variable | Config Field | Type | Example |
|---------------------|--------------|------|---------|
| `SHIM_CONFIG` | (config file path) | path | `/etc/shim.toml` |
| `SHIM_VALIDATE_CONFIG` | (enable/disable validation) | bool | `true` / `false` |
| `HEALTH_CMD` | `health.liveness_cmd`, `health.readiness_cmd` | string | `pg_isready` |
| `HEALTH_LISTEN` | `health.listen` | socket | `0.0.0.0:9101` |
| `HEALTH_INTERVAL_SECS` | `health.interval_secs` | u64 | `10` |
| `HEALTH_TIMEOUT_SECS` | `health.timeout_secs` | u64 | `5` |
| `PROCESS_COMMAND` | `process.command` | string | `my-app` |
| `PROCESS_ARGS` | `process.args` | string | `--verbose --port=8080` |
| `SHUTDOWN_TIMEOUT_SECS` | `process.shutdown_timeout_secs` | u64 | `30` |
| `SHIM_MAX_MEMORY_BYTES` | `resource_quota.max_memory_bytes` | u64 | `1073741824` |
| `SHIM_MAX_CPU_PERCENT` | `resource_quota.max_cpu_percent` | f64 | `80.0` |
| `SHIM_MAX_OPEN_FILES` | `resource_quota.max_open_files` | u32 | `1024` |
| `SHIM_TENANT_ID` | `tenants[].tenant_id` | string | `tenant-a` |
| `SHIM_TENANT_MAX_MEMORY` | `tenants[].max_memory_bytes` | u64 | `1073741824` |
| `SHIM_TENANT_MAX_CPU` | `tenants[].max_cpu_percent` | f64 | `75.5` |

## SIGHUP Reload Workflow

```
                    ┌─────────────────┐
                    │  Config loaded   │
                    │  SHA-256 hash    │
                    └────────┬────────┘
                             │
                    ┌────────▼────────┐
                    │  Watch for       │
                    │  file changes /  │
                    │  SIGHUP signal   │
                    └────────┬────────┘
                             │
                    ┌────────▼────────┐
                    │  Re-read file    │
                    │  Compute SHA-256 │
                    └────────┬────────┘
                             │
                    ┌────────▼────────┐
                    │  Hash changed?   │
                    └───┬─────────┬───┘
                        │         │
                   Yes  │         │  No
                        │         │
              ┌─────────▼───┐     │
              │  Validate    │     │
              │  new config  │     │
              └──────┬──────┘     │
                     │            │
              ┌──────▼──────┐     │
              │  Errors?     │     │
              └──┬───────┬──┘     │
                 │       │        │
            Yes  │       │ No     │
                 │       │        │
        ┌────────▼──┐    │   ┌────▼─────┐
        │  Log error │    │   │  No-op   │
        │  Keep old  │    │   │  (same   │
        │  config    │    │   │   hash)  │
        └───────────┘    │   └──────────┘
                         │
                ┌────────▼────────┐
                │  Apply new       │
                │  config to all   │
                │  capabilities    │
                └─────────────────┘
```

### Step-by-step

1. **Startup**: `Config::load()` reads `shim.toml` (or `$SHIM_CONFIG`), computes SHA-256, and stores the hash.
2. **Watch**: The `notify` crate watches the config file for metadata/content changes.
3. **SIGHUP**: `nix::sys::signal::signal(SIGHUP, ...)` triggers immediate reload without waiting for file watcher.
4. **Reload**: The file is re-read, a new SHA-256 is computed, and compared against the stored hash.
5. **Validate**: If the hash differs, the new config is validated (`config.validate()`). Errors are logged and the old config is kept.
6. **Apply**: If validation passes, the new config is broadcast via `ShimBus` to all capabilities.

## SHA-256 Hash Change Detection

The reload mechanism uses SHA-256 hashing to avoid unnecessary reloads:

```rust
use sha2::{Sha256, Digest};

fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}
```

- **Same hash**: No changes detected; reload is a no-op.
- **Different hash**: File content changed; trigger validation and reload.
- **Atomic reads**: The file is read in a single `read_to_string()` call to avoid partial reads during concurrent writes.

This approach is efficient for small config files (typically <10 KB) and avoids the overhead of file stat() comparisons which can miss content-only changes.

## Example Config Files

### Minimal (env-only)

No file needed — all configuration via environment variables:

```bash
export HEALTH_CMD="pg_isready"
export PROCESS_COMMAND="my-app"
export PROCESS_ARGS="--port=8080"
```

### Development

```toml
version = "1.0"

[health]
listen = "127.0.0.1:9101"
interval_secs = 30

[process]
command = "cargo"
args = ["run", "--bin", "my-app"]
shutdown_timeout_secs = 5
```

### Production with Vault and Backup

```toml
version = "1.0"

[health]
listen = "0.0.0.0:9101"
interval_secs = 10
timeout_secs = 5

[process]
command = "/app/my-app"
args = ["--release", "--port=8080"]
shutdown_timeout_secs = 30

[vault]
addr = "https://vault.prod.internal:8200"
role = "production"
secret = "secret/data/production/db"
rotation_secs = 1800

[backup]
schedule = "0 2 * * *"
storage = "s3"
retention_days = 90
database = "production"
prefix = "backups/production/"

[resource_quota]
max_memory_bytes = 4294967296
max_cpu_percent = 80.0
max_open_files = 4096
max_connections = 200
```

### Multi-Tenant

```toml
version = "1.0"

[health]
listen = "0.0.0.0:9101"

[[tenants]]
tenant_id = "team-alpha"
max_memory_bytes = 1073741824
max_cpu_percent = 50.0
max_requests_per_sec = 100
allowed_features = ["backup", "migration"]
reset_period_secs = 1

[[tenants]]
tenant_id = "team-beta"
max_memory_bytes = 2147483648
max_cpu_percent = 75.0
max_requests_per_sec = 500
allowed_features = ["backup", "migration", "failover", "replication"]
reset_period_secs = 1
```
