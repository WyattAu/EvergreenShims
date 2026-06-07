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
  kubeconfig_context      = "production-cluster"
  namespace               = "production"
  management_api_endpoint = "http://management-api.production.svc:8080"
}

variable "region" {
  description = "Deployment region"
  type        = string
  default     = "us-west-2"
}

variable "replica_count" {
  description = "Number of shim replicas"
  type        = number
  default     = 3
}

resource "evergreen_shims_shim_config" "ha" {
  name            = "ha-shim-config"
  namespace       = "production"
  shim_image      = "ghcr.io/wyattau/evergreen-shims/shim:v2.1.0"
  shim_version    = "v2.1.0"
  target_services = ["api-gateway", "auth-service", "payment-service"]

  resource_limits = {
    cpu    = "1000m"
    memory = "512Mi"
  }

  resource_requests = {
    cpu    = "500m"
    memory = "256Mi"
  }

  env_vars = {
    LOG_LEVEL       = "warn"
    MODE            = "production"
    CACHE_TTL       = "300"
    MAX_CONNECTIONS = "1000"
  }

  annotations = {
    "prometheus.io/scrape" = "true"
    "prometheus.io/port"   = "9090"
  }

  labels = {
    app         = "evergreen-shims"
    environment = "production"
    region      = var.region
  }
}

resource "evergreen_shims_deployment" "ha" {
  name                = "ha-shim-deployment"
  namespace           = "production"
  shim_config_name    = evergreen_shims_shim_config.ha.name
  replicas            = var.replica_count
  sidecar_enabled     = true
  management_api_port = 8080
  health_check_path   = "/healthz"

  selector_labels = {
    app       = "ha-shim"
    component = "shim"
    tier      = "production"
  }

  annotations = {
    "app.kubernetes.io/managed-by" = "terraform"
    "app.kubernetes.io/part-of"    = "evergreen-shims"
  }

  labels = {
    app         = "ha-shim"
    component   = "shim"
    environment = "production"
    region      = var.region
  }
}

data "evergreen_shims_status" "ha" {
  name      = evergreen_shims_deployment.ha.name
  namespace = "production"
}

output "deployment_status" {
  value = data.evergreen_shims_status.ha.status
}

output "ready_replicas" {
  value = data.evergreen_shims_status.ha.ready_replicas
}

output "health_status" {
  value = data.evergreen_shims_status.ha.health
}

output "last_health_check" {
  value = data.evergreen_shims_status.ha.last_health_check
}
