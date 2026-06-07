# EvergreenShims Node.js SDK

TypeScript/JavaScript client library for the EvergreenShims management API.

## Installation

```bash
npm install @evergreen-shims/sdk
```

## Usage

```typescript
import { Client, HealthStatus } from "@evergreen-shims/sdk";

const client = new Client("http://localhost:50051");

// Get shim status
const status = await client.status();
console.log(`Health: ${status.health}, Version: ${status.version}`);

// Get metrics
const metrics = await client.metrics();
metrics.metrics.forEach((m) => {
  console.log(`Metric: ${m.name} = ${m.value}`);
});

// Health checks
const live = await client.healthLiveness();
const ready = await client.healthReadiness();

// Config reload
const reload = await client.configReload();
console.log(reload.message);

// Backup operations
const backups = await client.backupList();
console.log(`Backups: ${backups.backups.length}`);

const trigger = await client.backupTrigger();
console.log(`Backup triggered: ${trigger.success}`);

// Migration operations
const migStatus = await client.migrationStatus();
console.log(`Migration version: ${migStatus.current_version}`);

const apply = await client.migrationApply();
console.log(`Applied: ${apply.migrations_applied} migrations`);

const rollback = await client.migrationRollback();
console.log(`Rolled back to version: ${rollback.current_version}`);
```

## Configuration

```typescript
import { Client } from "@evergreen-shims/sdk";

// Custom timeout
const client = new Client("http://localhost:50051", { timeout: 10_000 });
```

## API Methods

| Method | Description |
|--------|-------------|
| `status()` | Get shim health and version |
| `metrics()` | Get all collected metrics |
| `healthLiveness()` | Liveness check |
| `healthReadiness()` | Readiness check |
| `configReload(path?)` | Trigger config reload |
| `backupList()` | List available backups |
| `backupTrigger()` | Trigger a new backup |
| `migrationStatus()` | Get migration status |
| `migrationApply()` | Apply pending migrations |
| `migrationRollback()` | Rollback last migration |

## Exceptions

| Exception | Description |
|-----------|-------------|
| `EvergreenShimError` | Base exception |
| `APIError` | Non-success HTTP status |
| `ConnectionError` | Connection failure |
| `TimeoutError` | Request timeout |
