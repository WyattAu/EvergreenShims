# audit-shim

Database query logging and SIEM export. Captures database queries and exports them to syslog, file, or webhook.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `AUDIT_DATABASE` | Database name to audit | — |
| `AUDIT_TABLES` | Comma-separated tables to audit (empty = all) | — |
| `AUDIT_FORMAT` | Output format: `json`, `syslog`, `cef` | `json` |
| `AUDIT_OUTPUT` | Output destination: `file`, `stdout`, `webhook` | `stdout` |
| `AUDIT_OUTPUT_FILE` | File path when `output=file` | — |
| `AUDIT_WEBHOOK_URL` | Webhook URL when `output=webhook` | — |
| `AUDIT_LOG_QUERIES` | Log full query text | `false` |
| `AUDIT_LOG_PARAMETERS` | Log query parameters | `false` |
| `AUDIT_MIN_DURATION_MS` | Minimum query duration to log | `0` |
| `AUDIT_LOG_DIR` | Directory for audit log files | `/var/log/audit-shim` |
| `AUDIT_MAX_ENTRIES` | Max in-memory entries before rotation | `100000` |

## Usage

```rust
use audit_shim::AuditShim;
use shim_core::Capability;

let mut shim = AuditShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- JSON, syslog, and CEF output formats for SIEM integration.
- Filter by table and minimum query duration.
- In-memory ring buffer with configurable max entries.

## Metrics Exposed

- `audit_entries_total` – Total audit entries captured.
- `audit_entries_dropped` – Entries dropped due to ring buffer overflow.
- `audit_export_errors` – Export failures.

## Testing

```bash
cargo test -p audit-shim
```
