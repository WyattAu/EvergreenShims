# migration-shim

Database schema migrations with rollback support.

## Features

- **SQL file-based** — Standard `.up.sql` / `.down.sql` files
- **Version tracking** — Applied migrations stored in `_migrations` table
- **Auto-migrate** — Run pending migrations on startup
- **Multi-database** — PostgreSQL, MariaDB/MySQL

## Migration Files

```
/migrations/
  001_create_users.up.sql
  001_create_users.down.sql
  002_add_email_index.up.sql
  002_add_email_index.down.sql
```

## Configuration

### Environment Variables

```bash
MIGRATION_DIR="/migrations"              # Migration directory
MIGRATION_DATABASE="mydb"                # Database name
MIGRATION_DB_HOST="127.0.0.1"            # Database host
MIGRATION_DB_PORT=5432                    # Database port
MIGRATION_DB_USER="postgres"             # Database user
MIGRATION_DB_PASSWORD="secret"           # Database password
MIGRATION_AUTO_MIGRATE="true"            # Auto-migrate on startup
MIGRATION_DB_TYPE="postgres"             # Database type
```

## Usage

```rust
use migration_shim::MigrationShim;
use shim_core::Capability;

let mut shim = MigrationShim::new();
shim.init(&config).await?;
shim.start().await?;  // Runs migrations if auto_migrate=true
```

## Metrics

| Metric | Description |
|--------|-------------|
| `migration_current_version` | Current schema version |
| `migration_applied_total` | Total migrations applied |
| `migration_last_success_timestamp` | Last successful migration |
