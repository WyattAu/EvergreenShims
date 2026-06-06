# archival-shim

Data archival to cold storage. Moves old data from hot storage to cold storage (S3, Glacier, local disk).

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `ARCHIVAL_SCHEDULE` | Cron schedule | `0 3 * * *` |
| `ARCHIVAL_TABLES` | Tables to archive | — |
| `ARCHIVAL_AGE_DAYS` | Archive data older than N days | `90` |
| `ARCHIVAL_STORAGE` | Storage: `s3`, `glacier`, `local` | `s3` |
| `ARCHIVAL_BUCKET` | S3 bucket name or local directory | — |
| `ARCHIVAL_COMPRESSION` | Compression: `none`, `gzip`, `zstd` | `zstd` |
| `ARCHIVAL_LIFECYCLE_DAYS` | Days before moving to colder tier | `0` (disabled) |
| `ARCHIVAL_HOT_DAYS` | Days in hot tier before warm | `0` (disabled) |
| `ARCHIVAL_WARM_DAYS` | Days in warm tier before cold | `0` (disabled) |
| `ARCHIVAL_COLD_DAYS` | Days in cold tier before purge | `0` (disabled) |
| `ARCHIVAL_RETENTION_DAYS` | Global retention days | `365` |
| `ARCHIVAL_ARCHIVE_PATH` | Local archive directory | `/var/lib/archival` |

## Usage

```rust
use archival_shim::ArchivalShim;
use shim_core::Capability;

let mut shim = ArchivalShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Multi-tier lifecycle: hot → warm → cold → purge.
- S3, Glacier, and local disk storage backends.
- Configurable compression (zstd default).

## Metrics Exposed

- `archival_records_archived_total` – Total records archived.
- `archival_bytes_archived_total` – Total bytes archived.
- `archival_errors_total` – Archival failures.

## Testing

```bash
cargo test -p archival-shim
```
