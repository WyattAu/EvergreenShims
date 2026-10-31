1. **SIGHUP signal**; send `kill -HUP <pid>` to trigger an immediate reload.
2. **File change detection**; the `notify` crate watches the config file for write events.
3. **Manual trigger**; use `shimctl config reload` to trigger via the management API.
No file needed; all configuration via environment variables: