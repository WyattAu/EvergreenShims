# vault-shim

Automatic secrets rotation from HashiCorp Vault or cloud KMS.

## Features

- **Static secrets** — Read secrets from Vault KV store
- **Dynamic credentials** — Generate short-lived database credentials
- **Auto-rotation** — Rotate secrets on a configurable schedule
- **File output** — Write credentials to `.pgpass`, `MYSQL_PWD` files

## Configuration

### Environment Variables

```bash
VAULT_ADDR="https://vault.internal:8200"    # Vault server URL
VAULT_TOKEN="hvs.xxxx"                      # Vault token
VAULT_ROLE="postgres-readonly"              # Dynamic credentials role
VAULT_SECRET="secret/data/postgres/creds"   # Static secret path
VAULT_KEY="password"                        # Key within secret
VAULT_OUTPUT_FILE="/run/secrets/db-password" # Output file path
VAULT_ROTATION_SECS=3600                    # Rotation interval
VAULT_MOUNT="secret"                        # Vault mount point
```

### Dynamic vs Static

**Dynamic credentials** (preferred for databases):
```bash
VAULT_ROLE="postgres-readonly"  # Uses database secrets engine
```

**Static secrets** (for pre-existing passwords):
```bash
VAULT_SECRET="secret/data/myapp/db"  # Uses KV store
```

## Usage

```rust
use vault_shim::VaultShim;
use shim_core::Capability;

let mut shim = VaultShim::new();
shim.init(&config).await?;
shim.start().await?;  // Starts rotation loop
```

## Metrics

| Metric | Description |
|--------|-------------|
| `vault_rotation_success_total` | Successful rotations |
| `vault_rotation_failure_total` | Failed rotations |
| `vault_rotation_last_success_timestamp` | Last successful rotation |
