# backup-shim

Automated database backups with compression and retention.

## Features

- **Multi-database** — PostgreSQL, MariaDB/MySQL, Redis, MongoDB
- **Compression** — gzip, zstd, or none
- **Retention policy** — Auto-cleanup of old backups
- **Scheduled backups** — Cron-based scheduling

## Supported Databases

| Database | Tool | Notes |
|----------|------|-------|
| PostgreSQL | `pg_dump` | Custom format (`-Fc`) |
| MariaDB/MySQL | `mysqldump` | Single-transaction mode |
| Redis | `BGSAVE` | RDB file copy |
| MongoDB | `mongodump` | Full dump |

## Configuration

### Environment Variables

```bash
BACKUP_SCHEDULE="0 2 * * *"          # Cron schedule
BACKUP_STORAGE="local"               # Storage backend
BACKUP_PATH="/var/backups"           # Local path
BACKUP_RETENTION_DAYS=30             # Keep backups for 30 days
BACKUP_DATABASE="mydb"               # Database name
BACKUP_DB_TYPE="postgres"            # Database type
BACKUP_DB_HOST="127.0.0.1"           # Database host
BACKUP_DB_PORT=5432                   # Database port
BACKUP_DB_USER="postgres"            # Database user
BACKUP_DB_PASSWORD="secret"          # Database password
BACKUP_COMPRESSION="gzip"            # Compression type
```

## Usage

```rust
use backup_shim::BackupShim;
use shim_core::Capability;

let mut shim = BackupShim::new();
shim.init(&config).await?;
shim.start().await?;  // Runs initial backup + scheduled loop
```

## Backup Files

```
/var/backups/
  mydb_20240101_020000.sql.gz    # gzip compressed
  mydb_20240102_020000.sql.zst   # zstd compressed
  mydb_20240103_020000.sql       # uncompressed
```

## Metrics

| Metric | Description |
|--------|-------------|
| `backup_success_total` | Successful backups |
| `backup_failure_total` | Failed backups |
| `backup_size_bytes` | Last backup size |
| `backup_last_success_timestamp` | Last successful backup |
