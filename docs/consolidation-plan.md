# Crate Consolidation Plan

Analysis of overlapping crates with shared responsibilities and a concrete merge plan.

## Overlapping Crate Pairs

| Pair | Crates | Shared Concern | Severity |
|------|--------|----------------|----------|
| A | `audit-shim` / `compliance-shim` | Violation tracking, severity enums, reporting | High |
| B | `archival-shim` / `backup-shim` | Retention logic, lifecycle tiers, compression, S3 client | High |
| C | `queue-shim` / `scheduler-shim` | Job/task management, retry logic, worker execution | Medium |

---

## Pair A: `audit-shim` + `compliance-shim` → `compliance-shim`

### Current Responsibilities

**`audit-shim`** (`crates/audit-shim/src/lib.rs`):
- Captures database query logs as `AuditEntry` structs
- Writes entries to disk (daily rotation), stdout, or webhook
- Filters by table, operation, duration
- Formats output as JSON or CEF
- Maintains in-memory log with rotation

**`compliance-shim`** (`crates/compliance-shim/src/lib.rs`):
- Runs CIS/STIG compliance checks against PostgreSQL, MariaDB, Redis
- Tracks violations with `Violation` struct and `Severity` enum
- Generates compliance reports (`ComplianceReport`)
- Supports custom rules via TOML

### Shared Code Inventory

| Struct/Enum | In Both | Notes |
|-------------|---------|-------|
| `Severity` (enum) | `compliance-shim` only | `audit-shim` lacks severity entirely — should be common |
| `Violation` | `compliance-shim` only | Audit entries have implicit severity (success/failure) |
| `ComplianceReport` | `compliance-shim` only | Audit has `AuditStats` — different shape but same purpose |
| `HashMap<String, u64>` counters | Both | `AuditStats.operations` and `ComplianceShim.violation_counts()` |
| `log_dir` / `output_file` path management | Both | `AuditShim.log_dir` and `ComplianceShim.output` |
| `chrono::DateTime` retention filtering | `audit-shim` | `clear_log_before()` — reusable for compliance violation cleanup |
| `Capability` impl boilerplate | Both | Identical `init`/`start`/`stop`/`metrics` patterns |

### Overlap Assessment

The overlap is **structural, not functional**. They share:
- Severity-classified findings (audit entries have implicit severity; compliance has explicit `Severity`)
- Reporting with filtering and thresholds
- Output destinations (stdout, file, webhook)
- Retention/cleanup of historical records

They differ in:
- Audit captures _query-level_ events; compliance captures _configuration-level_ findings
- Audit has disk rotation; compliance does not persist to disk
- Compliance has database queries for live checks; audit only captures metadata

### Proposed Merge: Add `audit` module to `compliance-shim`

**Do NOT merge into a single struct.** Instead:

1. Move `Severity` enum to `shim-core` as a shared type
2. Add `crates/compliance-shim/src/audit.rs` module containing all current `audit-shim` types and logic
3. Re-export audit types from `compliance-shim` for backward compatibility
4. Add `audit` feature flag to `compliance-shim`
5. Delete `crates/audit-shim/` crate
6. Update `evergreen-shim` feature flags: `compliance-audit` replaces `audit`

**Migration path:**
```toml
# Before
[dependencies]
audit-shim = { path = "../audit-shim" }

# After
compliance-shim = { path = "../compliance-shim", features = ["audit"] }
```

### Impact

- **Breaks:** `use audit_shim::*` imports → `use compliance_shim::audit::*`
- **Risk:** Low — `audit-shim` is self-contained, no downstream crates depend on it
- **Effort:** ~2 hours (move code, add feature flag, update tests)

---

## Pair B: `archival-shim` + `backup-shim` → `backup-shim`

### Current Responsibilities

**`archival-shim`** (`crates/archival-shim/src/lib.rs`):
- Moves old data to cold storage (S3, local disk)
- Manages lifecycle tiers: `Hot` → `Warm` → `Cold`
- Per-table retention rules
- Compression ratio estimation
- Purges expired archives

**`backup-shim`** (`crates/backup-shim/src/lib.rs`):
- Automated database dumps (pg_dump, mysqldump, BGSAVE)
- SHA-256 checksum verification
- S3 upload with SSE
- Retention-based cleanup
- Backup verification (checksum + optional restore test)

### Shared Code Inventory

| Struct/Function | In Both | Notes |
|----------------|---------|-------|
| `StorageTier` (enum) | `archival-shim` only | Backup implicitly uses tiers (local → S3) |
| `RetentionRule` | `archival-shim` only | Backup has `retention_days` scalar |
| `S3Client` / `build_s3_client()` | **Both** | Nearly identical S3 client construction |
| `upload_to_s3()` | **Both** | Same pattern: put_object + SSE + content_type |
| `delete_from_s3()` | **Both** | Identical delete logic |
| `cleanup_retention()` | **Both** | Different data structures but same concept |
| `Compression` config | **Both** | `ARCHIVAL_COMPRESSION` / `BACKUP_COMPRESSION` — same values: none, gzip, zstd |
| `retention_days` | **Both** | `ARCHIVAL_RETENTION_DAYS` / `BACKUP_RETENTION_DAYS` |
| S3 config fields | **Both** | `s3_region`, `s3_endpoint`, `s3_prefix`, `s3_force_path_style`, `s3_server_side_encryption` |
| `ArchivedRecord` / `BackupEntry` | Both | Both track file path, size, timestamp, checksum |
| `ArchivalSummary` / `BackupState` | Both | Aggregate counters for metrics |

### Overlap Assessment

This is the **highest overlap pair**. Both shims:
- Upload/download to S3 with identical client construction
- Track retention with time-based expiry
- Compute compression ratios
- Have record-level metadata (path, size, timestamp)

They differ in:
- Archival is _data lifecycle_ (tiered); backup is _point-in-time recovery_
- Archival has lifecycle tiers (Hot/Warm/Cold); backup is simpler
- Backup executes dump commands; archival moves existing files

### Proposed Merge: Unify S3 + retention into `backup-shim`

1. Extract shared S3 logic into `crates/backup-shim/src/s3_util.rs`:
   - `S3Config` struct (region, endpoint, prefix, path_style, sse)
   - `build_s3_client(config: &S3Config) -> Result<S3Client>`
   - `upload_to_s3(client, bucket, key, data, config) -> Result<String>`
   - `delete_from_s3(client, bucket, key) -> Result<()>`
2. Move `StorageTier`, `RetentionRule` into `backup-shim` as `archival` module types
3. Move archival lifecycle logic into `backup-shim/src/archival.rs`
4. Add `archival` feature flag to `backup-shim`
5. Delete `crates/archival-shim/` crate
6. Unify env vars: `ARCHIVAL_*` → `BACKUP_*` with backward-compat aliases

**Migration path:**
```toml
# Before
[dependencies]
archival-shim = { path = "../archival-shim" }

# After
backup-shim = { path = "../backup-shim", features = ["archival"] }
```

### Impact

- **Breaks:** `use archival_shim::*` → `use backup_shim::archival::*`
- **Breaks:** Env vars `ARCHIVAL_*` → `BACKUP_*` (with deprecation aliases)
- **Risk:** Medium — S3 client unification requires careful testing
- **Effort:** ~4 hours (extract S3 util, merge retention, update tests)

---

## Pair C: `queue-shim` + `scheduler-shim` → `scheduler-shim`

### Current Responsibilities

**`queue-shim`** (`crates/queue-shim/src/lib.rs`):
- In-memory job queue with FIFO ordering
- Configurable worker pool (concurrent processing)
- Exponential backoff retry with configurable limits
- Dead-letter queue for exhausted jobs
- Job timeout (pending + execution)

**`scheduler-shim`** (`crates/scheduler-shim/src/lib.rs`):
- Cron expression parsing and task scheduling
- Task state tracking (Pending/Running/Success/Failed)
- Retry with exponential backoff + jitter
- Task timeout and consecutive failure tracking

### Shared Code Inventory

| Struct/Function | In Both | Notes |
|----------------|---------|-------|
| Retry logic (exponential backoff) | **Both** | `retry_delay()` — nearly identical formula |
| `max_retries` / `retry_base` / `retry_max` | **Both** | Same config pattern |
| `RetryConfig` | `scheduler-shim` | `queue-shim` has inline fields — should be shared |
| Job state tracking | **Both** | `JobStatus` vs `TaskState` — same states, different names |
| Worker/executor spawning | **Both** | `spawn_workers()` / `spawn_scheduler_loop()` |
| Shutdown via `watch::channel` | **Both** | Identical pattern |
| `handler` callback | **Both** | Same `Fn(Job) -> Future<Result<()>>` pattern |
| `AtomicU64` counters | **Both** | enqueued/processed/failed/retried/dead |
| Job timeout logic | **Both** | Pending timeout + execution timeout |

### Overlap Assessment

The overlap is **retry and state management**, not the execution model. They differ in:
- Queue is _worker-pull_ (N workers dequeue); scheduler is _time-push_ (cron fires)
- Queue has a dead-letter queue; scheduler tracks `consecutive_failures`
- Scheduler has jitter; queue does not
- Queue processes arbitrary payloads; scheduler runs named commands

### Proposed Merge: Share retry types in `shim-core`

Do NOT merge the shims — they serve different purposes. Instead:

1. Move `RetryConfig` to `shim-core` (shared across all shims with retry)
2. Move retry delay calculation to `shim-core::retry` module
3. Rename `JobStatus` and `TaskState` to a shared `RunState` enum in `shim-core`
4. Both shims import from `shim-core` instead of duplicating

**Migration path:**
```rust
// Before (queue-shim)
let delay = self.retry_base_secs * 2u32.saturating_pow(attempt.min(31)) as u64;

// After
use shim_core::retry::{RetryConfig, compute_delay};
let delay = compute_delay(attempt, &self.retry_config);
```

### Impact

- **Breaks:** Internal field names change (`retry_base_secs` → `retry_config.base_delay_secs`)
- **Risk:** Low — purely internal refactoring
- **Effort:** ~2 hours (extract to shim-core, update both crates)

---

## Migration Path Summary

| Phase | Action | Candles |
|-------|--------|---------|
| 1 | Move `Severity` to `shim-core` | Pair A prerequisite |
| 2 | Move `RetryConfig` + `compute_delay()` to `shim-core` | Pair C |
| 3 | Add `audit` module to `compliance-shim` | Pair A |
| 4 | Add `s3_util` + `archival` module to `backup-shim` | Pair B |
| 5 | Delete `audit-shim` and `archival-shim` crates | Cleanup |
| 6 | Update `evergreen-shim` feature flags | Integration |
| 7 | Update all import paths in tests and examples | Final |

## Risk Assessment

| Pair | Risk | Mitigation |
|------|------|------------|
| A (audit→compliance) | Low | Feature-flagged, no shared state |
| B (archival→backup) | Medium | S3 client unification needs integration tests; env var aliases needed |
| C (shared retry) | Low | Pure extraction to shim-core, no behavior change |

## What Breaks

- All downstream `use audit_shim::*` imports
- All downstream `use archival_shim::*` imports
- `ARCHIVAL_*` environment variables (backward-compat aliases provided)
- Feature flag names in `evergreen-shim`
- `Dockerfile.shim-image` build targets if features change

## Shared Code Inventory (All Pairs)

| Item | Location | Shared With |
|------|----------|-------------|
| `Severity` enum | compliance-shim | Should be in shim-core |
| `RetryConfig` struct | scheduler-shim | Should be in shim-core |
| `compute_delay()` fn | queue-shim, scheduler-shim | Should be in shim-core |
| `build_s3_client()` fn | archival-shim, backup-shim | Should be in backup-shim::s3_util |
| `upload_to_s3()` fn | archival-shim, backup-shim | Should be in backup-shim::s3_util |
| `delete_from_s3()` fn | archival-shim, backup-shim | Should be in backup-shim::s3_util |
| `retention_days` field | archival-shim, backup-shim | Unified in backup-shim |
| `compression` field | archival-shim, backup-shim | Unified in backup-shim |
| `watch::channel` shutdown | All 6 crates | Already in shim-core pattern |
