# encryption-shim

Transparent data encryption at rest. Provides AES-256-GCM and ChaCha20-Poly1305 encryption with automatic key rotation and envelope encryption.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `ENCRYPTION_METHOD` | Method: `aes-gcm`, `chacha20` | `aes-gcm` |
| `ENCRYPTION_KEY` | 32-byte hex key (or `ENCRYPTION_KEY_PATH` for file) | — |
| `ENCRYPTION_KEY_ID` | Current key ID for rotation tracking | — |
| `ENCRYPTION_PREV_KEYS` | JSON array of previous keys for decryption | — |
| `ENCRYPTION_AAD` | Additional Authenticated Data prefix | — |

## Usage

```rust
use encryption_shim::EncryptionShim;
use shim_core::Capability;

let mut shim = EncryptionShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- AES-256-GCM and ChaCha20-Poly1305 algorithms.
- Envelope encryption for key wrapping.
- Per-tenant key isolation.
- Key rotation with decryption of old data via `ENCRYPTION_PREV_KEYS`.

## Metrics Exposed

- `encryption_operations_total` – Total encrypt/decrypt operations.
- `encryption_key_rotations` – Total key rotations.
- `encryption_current_key_id` – Current active key ID.

## Testing

```bash
cargo test -p encryption-shim
```
