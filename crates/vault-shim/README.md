# vault-shim

Secrets rotation from HashiCorp Vault or cloud KMS. Reads database credentials from Vault, writes them to a file, and rotates on a schedule.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `VAULT_ADDR` | Vault server URL | `https://127.0.0.1:8200` |
| `VAULT_TOKEN` | Vault token (or use AppRole/K8s auth) | — |
| `VAULT_ROLE` | Vault role for dynamic credentials | — |
| `VAULT_SECRET` | Secret path (e.g. `secret/data/postgres/creds`) | — |
| `VAULT_KEY` | Key within secret | `password` |
| `VAULT_OUTPUT_FILE` | File to write rotated credentials | — |
| `VAULT_ROTATION_SECS` | Rotation interval in seconds | `3600` |
| `VAULT_MOUNT` | Vault mount point | `secret` |
| `VAULT_TLS_SKIP_VERIFY` | Skip TLS certificate verification | `false` |

## Usage

```rust
use vault_shim::VaultShim;
use shim_core::Capability;

let mut shim = VaultShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Supports AppRole, Kubernetes, and token-based Vault authentication.
- Credentials are written to `VAULT_OUTPUT_FILE` in a format suitable for `.pgpass` or environment files.
- Rotation runs on a background task at `VAULT_ROTATION_SECS` interval.

## Metrics Exposed

- `vault_rotation_total` – Total rotation attempts.
- `vault_rotation_success` – Successful rotations.
- `vault_rotation_failure` – Failed rotations.
- `vault_last_rotation_seconds` – Unix timestamp of last rotation.

## Testing

```bash
cargo test -p vault-shim
```
