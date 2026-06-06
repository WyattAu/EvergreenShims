# migration-shim

Database schema migrations with rollback support. Runs SQL migration files from a directory in order, tracking applied migrations in a `schema_migrations` table.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `MIGRATION_DIR` | Directory containing migration files | `/migrations` |
| `MIGRATION_DATABASE` | Database name | — |
| `MIGRATION_DB_HOST` | Database host | `127.0.0.1` |
| `MIGRATION_DB_PORT` | Database port | `5432` |
| `MIGRATION_DB_USER` | Database user | `postgres` |
| `MIGRATION_DB_PASSWORD` | Database password | — |
| `MIGRATION_DB_URL` | Full database URL (overrides host/port/user/password/name) | — |
| `MIGRATION_AUTO_MIGRATE` | Auto-migrate on startup | `false` |
| `MIGRATION_DB_TYPE` | Database type: `postgres`, `mysql` | — |

## Migration File Naming

```text
001_create_users.up.sql
001_create_users.down.sql
002_add_email_index.up.sql
002_add_email_index.down.sql
```

## Usage

```rust
use migration_shim::MigrationShim;
use shim_core::Capability;

let mut shim = MigrationShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Supports `postgres` and `mysql` database types.
- Lock file (`.migration.lock`) prevents concurrent migrations.
- Auto-migrate on startup via `MIGRATION_AUTO_MIGRATE=true`.

## Metrics Exposed

- `migration_applied_total` – Total migrations applied.
- `migration_rolled_back_total` – Total rollbacks executed.
- `migration_last_timestamp` – Unix timestamp of the last migration.

## Testing

```bash
cargo test -p migration-shim
```
