# EvergreenShims Go SDK

Go client library for the EvergreenShims management API.

## Installation

```bash
go get github.com/WyattAu/EvergreenShims/sdk/go
```

## Usage

```go
package main

import (
    "fmt"
    "log"

    shim "github.com/WyattAu/EvergreenShims/sdk/go"
)

func main() {
    client := shim.NewClient("http://localhost:50051")

    // Get shim status
    status, err := client.Status()
    if err != nil {
        log.Fatal(err)
    }
    fmt.Printf("Health: %s, Version: %s\n", status.Health, status.Version)

    // Get metrics
    metrics, err := client.Metrics()
    if err != nil {
        log.Fatal(err)
    }
    for _, m := range metrics.Metrics {
        fmt.Printf("Metric: %s = %f\n", m.Name, m.Value)
    }

    // Health checks
    live, _ := client.HealthLiveness()
    ready, _ := client.HealthReadiness()
    fmt.Printf("Liveness: %v, Readiness: %v\n", live, ready)

    // Config reload
    reload, _ := client.ConfigReload("")
    fmt.Printf("Reload: %s\n", reload.Message)

    // Backup operations
    backups, _ := client.BackupList()
    fmt.Printf("Backups: %d\n", len(backups.Backups))

    trigger, _ := client.BackupTrigger()
    fmt.Printf("Backup triggered: %v\n", trigger.Success)

    // Migration operations
    migStatus, _ := client.MigrationStatus()
    fmt.Printf("Migration version: %d\n", migStatus.CurrentVersion)

    apply, _ := client.MigrationApply()
    fmt.Printf("Applied: %d migrations\n", apply.MigrationsApplied)

    rollback, _ := client.MigrationRollback()
    fmt.Printf("Rolled back to version: %d\n", rollback.CurrentVersion)
}
```

## Configuration

```go
// Custom HTTP client
client := shim.NewClient("http://localhost:50051",
    shim.WithTimeout(10 * time.Second),
    shim.WithHTTPClient(&http.Client{Timeout: 5 * time.Second}),
)
```

## API Methods

| Method | Description |
|--------|-------------|
| `Status()` | Get shim health and version |
| `Metrics()` | Get all collected metrics |
| `HealthLiveness()` | Liveness check |
| `HealthReadiness()` | Readiness check |
| `ConfigReload(path)` | Trigger config reload |
| `BackupList()` | List available backups |
| `BackupTrigger()` | Trigger a new backup |
| `MigrationStatus()` | Get migration status |
| `MigrationApply()` | Apply pending migrations |
| `MigrationRollback()` | Rollback last migration |
