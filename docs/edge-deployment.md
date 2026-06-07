# Edge Deployment Guide

This guide covers deploying EvergreenShims on resource-constrained edge devices, including RISC-V and ARM Cortex-A platforms.

## Supported Edge Targets

| Architecture | Target Triple | Notes |
|-------------|---------------|-------|
| RISC-V 64-bit | `riscv64gc-unknown-linux-musl` | StarFive VisionFive 2, Sipeed LicheeRV |
| ARM 64-bit | `aarch64-unknown-linux-musl` | Raspberry Pi 4+, NVIDIA Jetson |
| x86_64 | `x86_64-unknown-linux-musl` | Intel/AMD edge gateways |

## Cross-Compilation

### RISC-V 64-bit (riscv64gc-unknown-linux-musl)

**Prerequisites:**
```bash
# Ubuntu/Debian
sudo apt-get install gcc-riscv64-linux-gnu musl-tools

# Add Rust target
rustup target add riscv64gc-unknown-linux-musl
```

**Build:**
```bash
# Using edge feature (minimal binary)
cargo build --release \
  --target riscv64gc-unknown-linux-musl \
  -p evergreen-shim \
  --no-default-features \
  --features edge \
  --config "target.riscv64gc-unknown-linux-musl.linker='riscv64-linux-gnu-gcc'"

# Strip and optimize
riscv64-linux-gnu-strip target/riscv64gc-unknown-linux-musl/release/shim
```

**Output binary size:** ~1.5-2.0 MB (edge feature)

### ARM 64-bit (aarch64-unknown-linux-musl)

**Prerequisites:**
```bash
# Ubuntu/Debian
sudo apt-get install gcc-aarch64-linux-gnu musl-tools binutils-aarch64-linux-gnu

# Add Rust target
rustup target add aarch64-unknown-linux-musl
```

**Build:**
```bash
# Using edge feature (minimal binary)
cargo build --release \
  --target aarch64-unknown-linux-musl \
  -p evergreen-shim \
  --no-default-features \
  --features edge \
  --config "target.aarch64-unknown-linux-musl.linker='aarch64-linux-gnu-gcc'"

# Strip and optimize
aarch64-linux-gnu-strip target/aarch64-unknown-linux-musl/release/shim
```

**Output binary size:** ~1.5-2.0 MB (edge feature)

### ARM 32-bit (armv7-unknown-linux-musleabihf)

**Prerequisites:**
```bash
# Ubuntu/Debian
sudo apt-get install gcc-arm-linux-gnueabihf musl-tools

# Add Rust target
rustup target add armv7-unknown-linux-musleabihf
```

**Build:**
```bash
cargo build --release \
  --target armv7-unknown-linux-musleabihf \
  -p evergreen-shim \
  --no-default-features \
  --features edge \
  --config "target.armv7-unknown-linux-musleabihf.linker='arm-linux-gnueabihf-gcc'"
```

## Edge Feature Configuration

The `edge` feature enables only the health-shim, providing minimal binary size for constrained devices:

```toml
[dependencies]
evergreen-shim = { path = "../evergreen-shim", features = ["edge"] }
```

**Features included:**
- `health` - Health check endpoint (readiness/liveness probes)

**Features excluded:**
- All database shims (vault, backup, migration, audit)
- Proxy and TLS shims
- Cache and CDC shims
- All heavy dependencies (reqwest, sqlx, tokio full, etc.)

## Resource Constraints

### Minimum Requirements

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| RAM | 16 MB | 64 MB |
| Storage | 5 MB | 20 MB |
| CPU | Single-core 400MHz | Multi-core 1GHz+ |

### Memory Optimization

For devices with <64 MB RAM:

```bash
# Build with memory optimization
RUSTFLAGS="-C link-arg=-Wl,--gc-sections" \
cargo build --release \
  --target riscv64gc-unknown-linux-musl \
  -p evergreen-shim \
  --no-default-features \
  --features edge
```

### Kernel Configuration

For RISC-V devices, ensure kernel has:
- `CONFIG_CGROUPS=y` (for container resource limits)
- `CONFIG_NAMESPACES=y` (for container isolation)
- `CONFIG_NET=y` (for network access)

## Container Image Alternatives

### Option 1: Scratch (Recommended for Edge)

Smallest image size, no OS overhead:

```dockerfile
FROM scratch
COPY --from=builder /shim /shim
USER 65532:65532
ENTRYPOINT ["/shim"]
```

**Image size:** ~2-3 MB

### Option 2: Alpine Linux

Provides basic shell utilities for debugging:

```dockerfile
FROM alpine:3.21
RUN apk add --no-cache ca-certificates
COPY --from=builder /shim /shim
USER 65532:65532
ENTRYPOINT ["/shim"]
```

**Image size:** ~8-10 MB

### Option 3: Debian Slim

Full glibc compatibility, larger but more stable:

```dockerfile
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /shim /shim
USER 65532:65532
ENTRYPOINT ["/shim"]
```

**Image size:** ~15-20 MB

### Multi-arch Docker Build

Build for multiple architectures:

```bash
# Create buildx builder
docker buildx create --name edge-builder --use

# Build for RISC-V
docker buildx build \
  --platform linux/riscv64 \
  --build-arg FEATURES=edge \
  --target shim-health \
  -t evergreenshim/health:edge-riscv64 \
  --push .

# Build for ARM64
docker buildx build \
  --platform linux/arm64 \
  --build-arg FEATURES=edge \
  --target shim-health \
  -t evergreenshim/health:edge-arm64 \
  --push .
```

## Deployment Examples

### Kubernetes on RISC-V

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: edge-shim
spec:
  replicas: 1
  selector:
    matchLabels:
      app: edge-shim
  template:
    metadata:
      labels:
        app: edge-shim
    spec:
      containers:
      - name: shim
        image: evergreenshim/health:edge-riscv64
        ports:
        - containerPort: 8080
        resources:
          limits:
            memory: "32Mi"
            cpu: "100m"
          requests:
            memory: "16Mi"
            cpu: "50m"
        livenessProbe:
          httpGet:
            path: /healthz
            port: 8080
          initialDelaySeconds: 5
          periodSeconds: 10
        readinessProbe:
          httpGet:
            path: /readyz
            port: 8080
          initialDelaySeconds: 3
          periodSeconds: 5
```

### Docker Compose on Edge Gateway

```yaml
version: '3.8'
services:
  health-shim:
    image: evergreenshim/health:edge-riscv64
    ports:
      - "8080:8080"
    deploy:
      resources:
        limits:
          memory: 32M
          cpus: '0.5'
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "wget", "--spider", "-q", "http://localhost:8080/healthz"]
      interval: 30s
      timeout: 5s
      retries: 3
```

### Systemd Service

```ini
[Unit]
Description=EvergreenShims Health Edge
After=network.target

[Service]
Type=simple
User=shim
ExecStart=/usr/local/bin/shim --mode health
Restart=always
RestartSec=5
MemoryMax=32M
CPUQuota=50%

[Install]
WantedBy=multi-user.target
```

## Troubleshooting

### Common Issues

1. **Segmentation fault on RISC-V**
   - Ensure kernel supports RISC-V GC extensions
   - Verify musl cross-compiler is properly installed

2. **Binary too large**
   - Use `--features edge` instead of `health`
   - Enable LTO in release profile (default)
   - Strip binary after build

3. **Network issues in container**
   - Use `scratch` base image with static binary
   - Ensure `/etc/resolv.conf` is mounted if needed

4. **Memory exhaustion**
   - Set `MemoryMax` in systemd or Kubernetes
   - Reduce Tokio worker threads: `TOKIO_WORKER_THREADS=1`

### Debugging

```bash
# Check binary dependencies
ldd target/riscv64gc-unknown-linux-musl/release/shim
# Should show "not a dynamic executable" for static build

# Check binary size
ls -lh target/riscv64gc-unknown-linux-musl/release/shim

# Test health endpoint
curl -v http://localhost:8080/healthz
```

## CI/CD Integration

The release pipeline automatically builds RISC-V binaries:

```yaml
# .github/workflows/release.yml
- target: riscv64gc-unknown-linux-musl
  arch: riscv64
  features: health
  artifact: shim-riscv64
```

Download from GitHub Releases:
```bash
# Download RISC-V binary
curl -LO https://github.com/WyattAu/EvergreenShims/releases/latest/download/shim-riscv64.gz
gunzip shim-riscv64.gz
chmod +x shim-riscv64
```
