# auth-shim

Authentication/authorization layer for database connections. Provides token validation, API key management, and role-based access control.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `AUTH_METHOD` | Method: `password`, `certificate`, `ldap`, `oauth` | `password` |
| `AUTH_LDAP_URL` | LDAP server URL | — |
| `AUTH_LDAP_BASE` | LDAP search base | — |
| `AUTH_OAUTH_ISSUER` | OAuth2 issuer URL | — |
| `AUTH_OAUTH_AUDIENCE` | OAuth2 audience | — |
| `AUTH_TOKEN_EXPIRY_SECS` | Token expiry in seconds | `3600` |
| `AUTH_MAX_FAILED_LOGINS` | Max failed login attempts before lockout | `5` |
| `AUTH_LOCKOUT_SECS` | Lockout duration in seconds | `300` |
| `AUTH_HMAC_KEY` | HMAC signing key (hex-encoded). Random key generated if unset | — |

## Usage

```rust
use auth_shim::AuthShim;
use shim_core::Capability;

let mut shim = AuthShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- Role-based access control: Admin, ReadWrite, ReadOnly.
- Account lockout after configurable failed attempts.
- HMAC-SHA256 token signing with auto-generated or manual keys.

## Metrics Exposed

- `auth_login_attempts_total` – Total login attempts.
- `auth_login_success` – Successful logins.
- `auth_login_failure` – Failed logins.
- `auth_active_tokens` – Currently active tokens.

## Testing

```bash
cargo test -p auth-shim
```
