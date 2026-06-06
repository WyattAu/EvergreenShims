# config-shim

Hot-reload configuration for applications. Watches a configuration file for changes using SHA-256 content hashing, validates new config via optional shell command, backs up the previous version, and signals the child process to reload.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `CONFIG_PATH` | Path to config file | `/etc/app/config.toml` |
| `CONFIG_WATCH` | Watch for changes | `true` |
| `CONFIG_RELOAD_SIGNAL` | Signal to send on change | `SIGHUP` |
| `CONFIG_RELOAD_DEBOUNCE` | Debounce interval in seconds | `5` |
| `CONFIG_VALIDATE_CMD` | Command to validate config | — |
| `CONFIG_BACKUP` | Keep backup of last config | `true` |

## Usage

```rust
use config_shim::ConfigShim;
use shim_core::Capability;

let mut shim = ConfigShim::new();
let config = shim_core::Config::default();
shim.init(&config).await?;
shim.start().await?;
```

## Configuration Options

- SHA-256 content hashing to detect changes (not mtime-based).
- Optional validation command runs before applying new config.
- Backups stored alongside the original with `.bak` extension.
- Debounce prevents rapid successive reloads.

## Metrics Exposed

- `config_reloads_total` – Total config reload attempts.
- `config_reload_success` – Successful reloads.
- `config_reload_failure` – Failed reloads.
- `config_last_reload_timestamp` – Unix timestamp of last reload.

## Testing

```bash
cargo test -p config-shim
```
