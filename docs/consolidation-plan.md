| `Severity` (enum) | `compliance-shim` only | `audit-shim` lacks severity entirely; should be common |
| `ComplianceReport` | `compliance-shim` only | Audit has `AuditStats`; different shape but same purpose |
| `chrono::DateTime` retention filtering | `audit-shim` | `clear_log_before()`; reusable for compliance violation cleanup |
- **Risk:** Low; `audit-shim` is self-contained, no downstream crates depend on it
| `Compression` config | **Both** | `ARCHIVAL_COMPRESSION` / `BACKUP_COMPRESSION`; same values: none, gzip, zstd |
- **Risk:** Medium; S3 client unification requires careful testing
| Retry logic (exponential backoff) | **Both** | `retry_delay()`; nearly identical formula |
| `RetryConfig` | `scheduler-shim` | `queue-shim` has inline fields; should be shared |
| Job state tracking | **Both** | `JobStatus` vs `TaskState`; same states, different names |
Do NOT merge the shims; they serve different purposes. Instead:
- **Risk:** Low; purely internal refactoring