# compliance-shim

CIS/STIG compliance checking. Checks database configuration against security benchmarks.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `COMPLIANCE_BENCHMARK` | Benchmark: `cis`, `stig`, `custom` | `cis` |
| `COMPLIANCE_DB_TYPE` | Database type: `postgres`, `mariadb` | — |
| `COMPLIANCE_REPORT` | Report format: `json`, `text` | `json` |
| `COMPLIANCE_OUTPUT` | Output: `stdout`, `file`, `webhook` | — |
| `COMPLIANCE_OUTPUT_FILE` | File path for reports | — |
| `COMPLIANCE_SEVERITY` | Minimum severity to report: `info`, `low`, `medium`, `high`, `critical` | — |

## Usage

```rust
use compliance_shim::ComplianceShim;
use shim_core::Capability;

let mut shim = ComplianceShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- CIS and STIG benchmark profiles.
- Configurable minimum severity filtering.
- JSON and text report formats.
- Output to stdout, file, or webhook.

## Metrics Exposed

- `compliance_checks_total` – Total compliance checks run.
- `compliance_findings` – Findings by severity.
- `compliance_last_run_timestamp` – Unix timestamp of last scan.

## Testing

```bash
cargo test -p compliance-shim
```
