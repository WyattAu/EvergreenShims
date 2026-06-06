# dynamodb-shim

Health checks, backup, and cost tracking for DynamoDB.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `DYNAMODB_REGION` | AWS region | `us-east-1` |
| `DYNAMODB_ENDPOINT` | Custom endpoint (for LocalStack) | — |
| `DYNAMODB_TABLE` | Table name to monitor | — |
| `DYNAMODB_BACKUP_TABLE` | Table name for backup exports | — |

## Usage

```rust
use dynamodb_shim::DynamoShim;
use shim_core::Capability;

let mut shim = DynamoShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Supports custom endpoints for LocalStack testing.
- Table-level monitoring with item count and size tracking.
- Backup exports via DynamoDB Export to S3.

## Metrics Exposed

- `dynamodb_health_checks_total` – Total health checks.
- `dynamodb_backup_exports_total` – Total backup exports.
- `dynamodb_item_count` – Item count of monitored table.
- `dynamodb_table_size_bytes` – Size of monitored table.

## Testing

```bash
cargo test -p dynamodb-shim
```
