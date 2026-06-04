# Building for Scratch Images

EvergreenShims targets `scratch` Docker images via musl static linking.

## Prerequisites

Install the musl toolchain:

```bash
# Ubuntu/Debian
apt-get install musl-tools

# macOS
brew install musl-cross

# Or use the Docker-based build
```

## Build Commands

### Health-shim only (~300KB)
```bash
cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --features health
```

### Database shim (~1MB)
```bash
cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --features db-shim
```

### Full shim (~3MB)
```bash
cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --features full
```

### Cross-architecture builds
```bash
# aarch64 (ARM64)
rustup target add aarch64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl \
    -p evergreen-shim --features full
```

## CI/CD Build

The recommended approach is to build in Docker with the musl toolchain pre-installed:

```dockerfile
FROM rust:1.75 as builder
RUN apt-get update && apt-get install -y musl-tools
RUN rustup target add x86_64-unknown-linux-musl
WORKDIR /src
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --features full

FROM scratch
COPY --from=builder /src/target/x86_64-unknown-linux-musl/release/shim /shim
ENTRYPOINT ["/shim"]
```

## Feature Flags

| Feature | Description | Size |
|---------|-------------|------|
| `health` | Health probes + metrics only | ~300KB |
| `db-shim` | health + vault + backup + migration + audit | ~1MB |
| `proxy-combo` | health + audit + tls | ~700KB |
| `ha-shim` | health + failover + replication | ~800KB |
| `full` | Everything | ~3MB |

## Notes

- `reqwest` (HTTP client) requires a C compiler for the `ring` crate
- Musl builds without `http` feature skip HTTP health checks (TCP/exec only)
- For HTTP health checks in scratch images, use the `http` feature with a musl-compatible build environment
