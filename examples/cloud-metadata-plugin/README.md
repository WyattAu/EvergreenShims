# Cloud Metadata Plugin

A shared library plugin for EvergreenShim that fetches instance metadata from cloud provider metadata endpoints.

## What It Does

This plugin queries the cloud provider's instance metadata service (e.g., AWS EC2 metadata at `http://169.254.169.254/`) and exposes the following information as metrics:

- **instance_type**: The EC2 instance type (e.g., `t3.medium`)
- **instance_id**: The instance ID (e.g., `i-0123456789abcdef0`)
- **region**: The AWS region (e.g., `us-east-1`)
- **availability_zone**: The AZ (e.g., `us-east-1a`)
- **ami_id**: The AMI ID used to launch the instance
- **hostname**: The instance hostname
- **local_ipv4**: The instance's private IP address

## Building

```bash
cd examples/cloud-metadata-plugin
cargo build --release
```

This produces a shared library at `target/release/libcloud_metadata_plugin.so` (Linux) or `target/release/libcloud_metadata_plugin.dylib` (macOS).

## Deployment

1. Copy the built shared library to your EvergreenShim plugin directory:
   ```bash
   cp target/release/libcloud_metadata_plugin.so /etc/evergreen-shims/plugins/
   ```

2. Add the plugin to your EvergreenShim configuration:
   ```toml
   [[plugins]]
   name = "cloud-metadata"
   path = "/etc/evergreen-shims/plugins/libcloud_metadata_plugin.so"
   enabled = true
   ```

3. Restart EvergreenShim or trigger a config reload via the management API.

## Configuration Options

The plugin accepts the following JSON configuration:

```json
{
  "metadata_url": "http://169.254.169.254/latest/meta-data/"
}
```

| Option | Default | Description |
|--------|---------|-------------|
| `metadata_url` | `http://169.254.169.254/latest/meta-data/` | Base URL for the cloud metadata endpoint |

### Provider-Specific URLs

- **AWS EC2**: `http://169.254.169.254/latest/meta-data/`
- **GCP Compute**: `http://metadata.google.internal/computeMetadata/v1/` (requires `Metadata-Flavor: Google` header)
- **Azure**: `http://169.254.169.254/metadata/instance?api-version=2021-02-01` (requires `Metadata: true` header)

Note: Currently only the AWS EC2 metadata format is supported. GCP and Azure would require additional header handling.

## Plugin Interface

This plugin implements the EvergreenShim `PluginVTable` ABI:

- `shim_plugin_init()` - Returns the vtable pointer
- `plugin_init(config_json)` - Parses configuration and sets up the HTTP client
- `plugin_start()` - Marks the plugin as started
- `plugin_stop()` - Marks the plugin as stopped
- `plugin_metrics()` - Fetches and returns cloud metadata as JSON metrics
- `plugin_name()` - Returns `"cloud-metadata"`

## Testing

To test the plugin in isolation (requires access to a cloud metadata endpoint):

```bash
cargo test
```
