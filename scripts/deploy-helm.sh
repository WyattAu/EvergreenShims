#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="${NAMESPACE:-evergreen-shims}"
RELEASE="${RELEASE:-evergreen-shims}"
CHART_DIR="${CHART_DIR:-helm/evergreen-shims}"
VALUES_FILE="${VALUES_FILE:-helm/evergreen-shims/values.yaml}"
WAIT="${WAIT:-true}"
TIMEOUT="${TIMEOUT:-300}"

usage() {
  cat <<EOF
Usage: $0 [OPTIONS]

Deploy the EvergreenShims Helm chart to a Kubernetes namespace.

Options:
  -n, --namespace NS     Kubernetes namespace (default: evergreen-shims)
  -r, --release NAME     Helm release name (default: evergreen-shims)
  -f, --values FILE      Custom values file (default: helm/evergreen-shims/values.yaml)
  -c, --chart DIR        Chart directory (default: helm/evergreen-shims)
      --no-wait          Don't wait for rollout
      --timeout SECS     Helm timeout in seconds (default: 300)
  -h, --help             Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case $1 in
    -n|--namespace) NAMESPACE="$2"; shift 2 ;;
    -r|--release) RELEASE="$2"; shift 2 ;;
    -f|--values) VALUES_FILE="$2"; shift 2 ;;
    -c|--chart) CHART_DIR="$2"; shift 2 ;;
    --no-wait) WAIT="false"; shift ;;
    --timeout) TIMEOUT="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1"; usage; exit 1 ;;
  esac
done

echo "==> Deploying evergreen-shims"
echo "    Namespace:  ${NAMESPACE}"
echo "    Release:    ${RELEASE}"
echo "    Chart:      ${CHART_DIR}"
echo "    Values:     ${VALUES_FILE}"

# Ensure kubectl is available
if ! command -v kubectl &>/dev/null; then
  echo "::error::kubectl not found in PATH" >&2
  exit 1
fi

# Ensure helm is available
if ! command -v helm &>/dev/null; then
  echo "::error::helm not found in PATH" >&2
  exit 1
fi

# Create namespace if it doesn't exist
if ! kubectl get namespace "${NAMESPACE}" &>/dev/null; then
  echo "==> Creating namespace ${NAMESPACE}"
  kubectl create namespace "${NAMESPACE}"
fi

# Lint the chart
echo "==> Linting chart"
helm lint "${CHART_DIR}" -f "${VALUES_FILE}"

# Install or upgrade
HELM_ARGS=(
  upgrade --install "${RELEASE}" "${CHART_DIR}"
  --namespace "${NAMESPACE}"
  --values "${VALUES_FILE}"
  --timeout "${TIMEOUT}s"
  --atomic
)

if [[ "${WAIT}" == "true" ]]; then
  HELM_ARGS+=(--wait)
fi

echo "==> Installing/upgrading Helm release"
helm "${HELM_ARGS[@]}"

# Verify deployment
echo "==> Verifying deployment"
kubectl get pods -n "${NAMESPACE}" -l "app.kubernetes.io/name=evergreen-shims"

echo ""
echo "==> Deployment complete"
helm status "${RELEASE}" -n "${NAMESPACE}"
