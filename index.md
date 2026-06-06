---
layout: default
title: EvergreenShims
---

# EvergreenShims

Rust-native shims for self-managing container images. Single binary, multiple capabilities, zero runtime overhead.

## Documentation

- [Architecture](docs/architecture) - System design, layered architecture, Capability trait
- [Building](docs/building) - musl static builds, cross-compilation, feature flags
- [Testing](docs/testing) - Test pyramid, coverage targets, integration matrix
- [Contributing](CONTRIBUTING) - Development workflow, shim creation guide

## Quick Links

| Resource | Link |
|----------|------|
| GitHub Repository | [github.com/WyattAu/EvergreenShims](https://github.com/WyattAu/EvergreenShims) |
| Releases | [github.com/WyattAu/EvergreenShims/releases](https://github.com/WyattAu/EvergreenShims/releases) |
| Container Images | [ghcr.io/wyattau/evergreenshim](https://github.com/WyattAu/EvergreenShims/pkgs/container/evergreenshim) |
| Issue Tracker | [github.com/WyattAu/EvergreenShims/issues](https://github.com/WyattAu/EvergreenShims/issues) |
| License | Apache-2.0 |

## Pre-Built Binaries

| Binary | Feature Set | Size | Target |
|--------|-------------|------|--------|
| `health-shim` | health | ~300KB | Any container |
| `db-shim` | health + vault + backup + migration + audit | ~1MB | Database containers |
| `proxy-shim` | health + audit + tls | ~700KB | Reverse proxies |
| `ha-shim` | health + failover + replication | ~800KB | HA database clusters |
| `full-shim` | all 27 shims | ~3MB | Full operational stack |

## Container Images

Published to GitHub Container Registry (GHCR):

```bash
docker pull ghcr.io/wyattau/evergreenshim/health-shim:latest
docker pull ghcr.io/wyattau/evergreenshim/db-shim:latest
docker pull ghcr.io/wyattau/evergreenshim/cache-shim:latest
docker pull ghcr.io/wyattau/evergreenshim/evergreen-shim:latest
```

## Test Coverage

577 tests across 30 crates. Unit, integration, and chaos test tiers.

## License

Apache License, Version 2.0.
