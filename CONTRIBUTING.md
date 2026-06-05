# Contributing

## Prerequisites

- Rust 1.75+ (stable)
- Docker (for integration tests)
- PostgreSQL, MariaDB, Redis clients (for DB-specific tests)

## Development Workflow

```bash
git clone https://github.com/WyattAu/EvergreenShims.git
cd EvergreenShims
./scripts/install-hooks.sh    # Enable pre-commit checks
cargo build --workspace
cargo test --workspace
```

### Pre-Commit Hook

The pre-commit hook enforces:

1. `cargo fmt --all -- --check` (auto-formats on failure)
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace --lib` (unit tests only)

## Adding a Shim

1. Create crate:

```bash
cargo new crates/my-shim --lib
```

2. Implement `Capability` trait:

```rust
use shim_core::{Capability, Config, Metric, Result, ShimBus};

pub struct MyShim;

impl Capability for MyShim {
    fn name(&self) -> &str { "my-shim" }
    fn init(&mut self, _config: &Config) -> Result<()> { Ok(()) }
    fn start(&mut self) -> Result<()> { Ok(()) }
    fn stop(&mut self) -> Result<()> { Ok(()) }
    fn metrics(&self) -> Vec<Metric> { vec![] }
    fn set_bus(&mut self, _bus: ShimBus) {}
}
```

3. Add feature flag to `crates/evergreen-shim/Cargo.toml`:

```toml
[features]
default = ["health"]
my-shim = ["dep:my-shim"]

[dependencies]
my-shim = { path = "../my-shim", optional = true }
```

4. Wire into binary (`crates/evergreen-shim/src/main.rs`):

```rust
#[cfg(feature = "my-shim")]
capabilities.push(Box::new(my_shim::MyShim));
```

5. Add to the appropriate preset feature set in `Cargo.toml`.

## Commit Convention

[Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
feat(vault-shim): add dynamic credential rotation
fix(failover-shim): correct Patroni check interval parsing
docs(building): update aarch64 cross-compile instructions
test(integration): add health-to-failover event chain test
chore(ci): add security audit job
```

## Code Style

- `cargo fmt --all` for formatting
- `cargo clippy --workspace -- -D warnings` for linting
- Document all public API items with `///` doc comments
- No `unwrap()` in library code -- use `anyhow::Result` with context

## Pull Requests

- Reference related issues (`Fixes #123`)
- Include tests for new features
- Update documentation if API changes
- One feature per PR
- All CI checks must pass before merge

## Testing

```bash
# Unit tests (fast, no dependencies)
cargo test --workspace

# Integration tests (requires Docker)
docker compose -f tests/docker-compose.yml up -d
cargo test --workspace --lib
docker compose -f tests/docker-compose.yml down -v
```

## Release Process

1. Update version in workspace `Cargo.toml`
2. Commit: `chore: bump version to vX.Y.Z`
3. Tag: `git tag vX.Y.Z`
4. Push: `git push origin main --tags`
5. GitHub Actions builds binaries, creates Docker images, publishes release
