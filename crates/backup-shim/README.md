# backup-shim

Automated database backups with S3 upload and retention. Supports PostgreSQL (`pg_dump`), MariaDB/MySQL (`mysqldump`), Redis (`BGSAVE`), and MongoDB (`mongodump`).

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `BACKUP_SCHEDULE` | Cron schedule | `0 2 * * *` |
| `BACKUP_STORAGE` | Storage backend: `s3`, `local` | `local` |
| `BACKUP_PATH` | Local path or S3 bucket | — |
| `BACKUP_PREFIX` | Key prefix for backups | — |
| `BACKUP_RETENTION_DAYS` | Days to keep backups | `30` |
| `BACKUP_DATABASE` | Database name | — |
| `BACKUP_DB_TYPE` | Database type: `postgres`, `mariadb`, `mysql`, `redis`, `mongo` | — |
| `BACKUP_DB_HOST` | Database host | `127.0.0.1` |
| `BACKUP_DB_PORT` | Database port | — |
| `BACKUP_DB_USER` | Database user | — |
| `BACKUP_DB_PASSWORD` | Database password | — |
| `BACKUP_CMD` | Backup command override | `pg_dump` |
| `BACKUP_OUTPUT_DIR` | Output directory | `/tmp/backups` |
| `REDIS_URL` | Redis connection URL | `redis://localhost:6379` |
| `REDIS_PASSWORD` | Redis password | — |
| `BACKUP_COMPRESSION` | Compression: `none`, `gzip`, `zstd` | `gzip` |
| `BACKUP_TIMEOUT_SECS` | Timeout for dump command | `3600` |

## Usage

```rust
use backup_shim::BackupShim;
use shim_core::Capability;

let mut shim = BackupShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Cron-based scheduling for automated backups.
- S3 or local storage backends.
- Configurable compression and retention policies.
- SHA-256 checksums generated for each backup.

## Metrics Exposed

- `backup_total` – Total backups attempted.
- `backup_success` – Successful backups.
- `backup_failure` – Failed backups.
- `backup_size_bytes` – Size of the last backup.
- `backup_duration_seconds` – Duration of the last backup.

## Testing

```bash
cargo test -p backup-shim
```
