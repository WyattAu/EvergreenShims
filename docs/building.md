# Building

## Target Environment

All builds produce statically linked binaries via musl. The target container is `scratch` (zero base image).

## Prerequisites

```bash
# Ubuntu/Debian
apt-get install -y musl-tools

# Cross-compile aarch64
apt-get install -y gcc-aarch64-linux-gnu binutils-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-musl
```

## Build Commands

### Feature-Prescribed Binaries

```bash
# Health only (~300KB)
cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --no-default-features --features health

# Database shim (~1MB)
cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --no-default-features --features db-shim

# Full shim (~3MB)
cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --no-default-features --features full
```

### Cross-Compilation (aarch64)

```bash
cargo build --release --target aarch64-unknown-linux-musl \
    -p evergreen-shim --no-default-features --features health \
    --config "target.aarch64-unknown-linux-musl.linker='aarch64-linux-gnu-gcc'"
```

### Post-Build

```bash
strip target/x86_64-unknown-linux-musl/release/shim
gzip target/x86_64-unknown-linux-musl/release/shim
```

## Feature Flags

| Feature | Composition | Approximate Size |
|---------|-------------|------------------|
| `health` | health-shim only | ~300KB |
| `db-shim` | health + vault + backup + migration + audit | ~1MB |
| `proxy-combo` | health + audit + tls | ~700KB |
| `ha-shim` | health + failover + replication | ~800KB |
| `cache-shim-combo` | health + cache + replication | ~700KB |
| `infra` | health + vault + backup + migration + audit + config + scheduler + queue + alerting | ~2MB |
| `full` | all 27 shims | ~3MB |

## Docker Build

Multi-stage Dockerfile at `Dockerfile.shim-image`:

```dockerfile
FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl \
    -p evergreen-shim --no-default-features --features ${FEATURES:-health}

FROM scratch
COPY --from=builder /src/target/x86_64-unknown-linux-musl/release/shim /shim
USER 65532
ENTRYPOINT ["/shim"]
```

Build targets: `shim-health`, `shim-db`, `shim-cache`, `shim-full`.

## Notes

- `reqwest` requires `aws-lc-rs` for musl (ring-free configuration)
- `rustls` is used for TLS (no OpenSSL dependency)
- aarch64 `full`/`infra` builds excluded from CI due to `aws-lc-rs` cross-compile limitation
- All binaries are stripped before packaging
