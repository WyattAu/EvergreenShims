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

resource "evergreen_shims_shim_config" "basic" {
  name            = "basic-shim"
  namespace       = "default"
  shim_image      = "ghcr.io/wyattau/evergreen-shims/shim:latest"
  shim_version    = "v1.0.0"
  target_services = ["nginx-proxy"]

  resource_limits = {
    cpu    = "250m"
    memory = "128Mi"
  }

  resource_requests = {
    cpu    = "100m"
    memory = "64Mi"
  }

  env_vars = {
    LOG_LEVEL = "info"
    MODE      = "passthrough"
  }

  labels = {
    app         = "evergreen-shims"
    environment = "dev"
  }
}

resource "evergreen_shims_deployment" "basic" {
  name                = "basic-shim-deployment"
  namespace           = "default"
  shim_config_name    = evergreen_shims_shim_config.basic.name
  replicas            = 1
  sidecar_enabled     = true
  management_api_port = 8080
  health_check_path   = "/healthz"

  selector_labels = {
    app       = "basic-shim"
    component = "shim"
  }

  labels = {
    app         = "basic-shim"
    component   = "shim"
    environment = "dev"
  }
}

data "evergreen_shims_status" "basic" {
  name      = evergreen_shims_deployment.basic.name
  namespace = "default"
}

output "shim_status" {
  value = data.evergreen_shims_status.basic.status
}

output "shim_health" {
  value = data.evergreen_shims_status.basic.health
}
