# mongodb-shim

Health checks, backup, and CDC for MongoDB.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `MONGO_URI` | MongoDB connection URI | `mongodb://localhost:27017` |
| `MONGO_DATABASE` | Database name | — |
| `MONGO_BACKUP_DIR` | Backup output directory | `/tmp/mongo-backups` |
| `MONGO_BACKUP_CMD` | Backup command | `mongodump` |
| `MONGO_RETENTION_DAYS` | Backup retention days | `30` |
| `MONGO_CDC_OUTPUT` | CDC output: `kafka`, `webhook`, `log` | `log` |

## Usage

```rust
use mongodb_shim::MongoShim;
use shim_core::Capability;

let mut shim = MongoShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Health checks via MongoDB connection status.
- Backup using `mongodump` with configurable retention.
- CDC output to Kafka, webhook, or log.

## Metrics Exposed

- `mongo_health_checks_total` – Total health checks.
- `mongo_backup_success` – Successful backups.
- `mongo_backup_failure` – Failed backups.
- `mongo_last_backup_timestamp` – Unix timestamp of last backup.

## Testing

```bash
cargo test -p mongodb-shim
```
