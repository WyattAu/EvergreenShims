# EvergreenShims Terraform Provider

Terraform provider for managing EvergreenShims resources on Kubernetes.

## Overview

This provider allows you to manage EvergreenShims using Terraform, including:

- **ShimConfig** - Configure shim deployments with target services and resource settings
- **ShimDeployment** - Deploy shim containers with sidecar injection
- **ShimStatus** - Query current status of deployed shims

## Prerequisites

- Terraform >= 1.0
- Go >= 1.21 (for building)
- Access to a Kubernetes cluster
- EvergreenShims management API endpoint

## Installation

### Building from source

```bash
cd terraform/evergreen-shims
go build -o terraform-provider-evergreen-shims
```

Install locally:

```bash
mkdir -p ~/.terraform.d/plugins/registry.terraform.io/wyattau/evergreen-shims/0.1.0/linux_amd64
cp terraform-provider-evergreen-shims ~/.terraform.d/plugins/registry.terraform.io/wyattau/evergreen-shims/0.1.0/linux_amd64/
```

## Configuration

```hcl
terraform {
  required_providers {
    evergreen-shims = {
      source  = "registry.terraform.io/wyattau/evergreen-shims"
      version = "~> 0.1.0"
    }
  }
}

provider "evergreen-shims" {
  kubeconfig_path         = "~/.kube/config"
  kubeconfig_context      = "my-cluster"
  namespace               = "default"
  management_api_endpoint = "http://management-api:8080"
}
```

### Provider Arguments

| Argument | Description | Required | Default |
|----------|-------------|----------|---------|
| `kubeconfig_path` | Path to kubeconfig file | No | `$KUBECONFIG` |
| `kubeconfig_context` | Kubeconfig context to use | No | - |
| `namespace` | Default namespace | No | `default` |
| `management_api_endpoint` | Management API endpoint | Yes | - |

## Resources

### `evergreen_shims_shim_config`

Manages a ShimConfig custom resource.

```hcl
resource "evergreen_shims_shim_config" "example" {
  name            = "my-shim-config"
  namespace       = "default"
  shim_image      = "ghcr.io/wyattau/evergreen-shims/shim:latest"
  shim_version    = "v1.0.0"
  target_services = ["service-a", "service-b"]

  resource_limits = {
    cpu    = "500m"
    memory = "256Mi"
  }

  labels = {
    environment = "production"
  }
}
```

### `evergreen_shims_deployment`

Manages a shim deployment.

```hcl
resource "evergreen_shims_deployment" "example" {
  name               = "my-shim-deployment"
  namespace          = "default"
  shim_config_name   = evergreen_shims_shim_config.example.name
  replicas           = 3
  sidecar_enabled    = true
  management_api_port = 8080

  selector_labels = {
    app = "my-shim"
  }
}
```

## Data Sources

### `evergreen_shims_status`

Query the status of a deployed shim.

```hcl
data "evergreen_shims_status" "example" {
  name      = "my-shim-deployment"
  namespace = "default"
}

output "shim_health" {
  value = data.evergreen_shims_status.example.health
}
```

## Examples

See the `examples/` directory for complete usage examples:

- `examples/basic/` - Simple single-shim deployment
- `examples/ha/` - High-availability multi-replica deployment

## Development

### Building

```bash
cd terraform/evergreen-shims
go build -v
```

### Testing

```bash
go test -v ./...
```

### Local Development

To use the provider locally during development:

```bash
# Build the provider
go build -o terraform-provider-evergreen-shims

# Initialize Terraform with local provider
terraform init
```

## License

MIT
