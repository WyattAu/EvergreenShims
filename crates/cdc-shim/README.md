# cdc-shim

Change Data Capture for event-driven architectures. Reads database WAL/binlog and publishes changes to Kafka, NATS, or webhooks.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `CDC_OUTPUT` | Output: `kafka`, `nats`, `webhook` | — (required) |
| `CDC_TABLES` | Comma-separated tables (empty = all) | — |
| `CDC_FORMAT` | Format: `json`, `avro`, `protobuf` | `json` |
| `CDC_COMPRESSION` | Compression: `none`, `zstd` | `none` |
| `CDC_KAFKA_BROKERS` | Kafka brokers (for kafka output) | — |
| `CDC_KAFKA_TOPIC` | Kafka topic | — |
| `CDC_WEBHOOK_URL` | Webhook URL (for webhook output) | — |
| `CDC_DB_TYPE` | Database type: `postgres`, `mariadb` | — |
| `CDC_SLOT` | Replication slot (PostgreSQL) | — |
| `CDC_BATCH_SIZE` | Batch size for publishing | `100` |
| `CDC_PUBLISH_INTERVAL` | Publish interval in seconds | `10` |

## Usage

```rust
use cdc_shim::CdcShim;
use shim_core::Capability;

let mut shim = CdcShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Supports Kafka, NATS, and webhook output targets.
- JSON, Avro, and Protobuf serialization formats.
- Batch publishing with configurable batch size and interval.
- PostgreSQL WAL and MariaDB binlog support.

## Metrics Exposed

- `cdc_events_published_total` – Total events published.
- `cdc_events_pending` – Events in the publish queue.
- `cdc_publish_errors_total` – Total publish failures.
- `cdc_wal_lsn` – Current WAL LSN being processed.

## Testing

```bash
cargo test -p cdc-shim
```
