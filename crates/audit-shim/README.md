# audit-shim

Database query logging and SIEM export.

## Features

- **Query logging** — Capture database operations
- **Multiple formats** — JSON, CEF (Common Event Format)
- **Multiple outputs** — stdout, file, webhook
- **Filtering** — By table, minimum duration

## Configuration

### Environment Variables

```bash
AUDIT_DATABASE="mydb"                  # Database to audit
AUDIT_TABLES="users,orders"            # Tables to audit (empty = all)
AUDIT_FORMAT="json"                    # Output format
AUDIT_OUTPUT="stdout"                  # Output destination
AUDIT_OUTPUT_FILE="/var/log/audit.log" # File path (when output=file)
AUDIT_WEBHOOK_URL="https://..."        # Webhook URL (when output=webhook)
AUDIT_LOG_QUERIES="false"              # Log full query text
AUDIT_MIN_DURATION_MS=100              # Only log queries > 100ms
```

## Output Formats

### JSON
```json
{
  "timestamp": "2024-01-01T12:00:00Z",
  "database": "mydb",
  "operation": "SELECT",
  "table": "users",
  "duration_ms": 15,
  "success": true
}
```

### CEF
```
CEF:0|EvergreenShim|audit|1.0|1|SELECT|15|database=mydb operation=SELECT table=users
```

## Usage

```rust
use audit_shim::AuditShim;
use shim_core::Capability;

let mut shim = AuditShim::new();
shim.init(&config).await?;
shim.start().await?;
```

## Metrics

| Metric | Description |
|--------|-------------|
| `audit_queries_logged_total` | Total queries logged |
