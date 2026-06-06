# tls-shim

Automatic TLS certificate management. Obtains and renews TLS certificates from Let's Encrypt, an internal CA, or Vault PKI.

## Supported Providers

- **internal-ca**: Self-signed certificates using `rustls` key generation.
- **letsencrypt**: ACME HTTP-01 challenge (requires port 80 access).
- **vault-pki**: Certificates from HashiCorp Vault PKI secrets engine.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `TLS_PROVIDER` | Provider: `letsencrypt`, `internal-ca`, `vault-pki` | — (required) |
| `TLS_DOMAIN` | Domain for the certificate | — (required) |
| `TLS_EMAIL` | Email for Let's Encrypt notifications | — |
| `TLS_RENEW_BEFORE` | Renew before expiry (seconds) | `259200` (72h) |
| `TLS_CERT_FILE` | Path to existing certificate | — |
| `TLS_KEY_FILE` | Path to existing key | — |
| `TLS_LISTEN` | Listen address for ACME challenge | `:80` |
| `TLS_DATA_DIR` | Directory to store certificates | `/etc/tls` |
| `TLS_MIN_VERSION` | Minimum TLS version | `TLS1.2` |
| `TLS_VAULT_ADDR` | Vault address (for vault-pki) | `https://127.0.0.1:8200` |
| `TLS_VAULT_ROLE` | Vault PKI role | `pki` |
| `TLS_VAULT_TOKEN` | Vault token (for vault-pki) | — |

## Usage

```rust
use tls_shim::TlsShim;
use shim_core::Capability;

let mut shim = TlsShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Automatic renewal based on `TLS_RENEW_BEFORE` threshold.
- Certificate fingerprint tracking via SHA-256.
- PEM-encoded certificate and key accessible through `CertInfo`.

## Metrics Exposed

- `tls_cert_days_until_expiry` – Days until current cert expires.
- `tls_renewals_total` – Total certificate renewals.
- `tls_renewal_failures_total` – Failed renewal attempts.

## Testing

```bash
cargo test -p tls-shim
```
