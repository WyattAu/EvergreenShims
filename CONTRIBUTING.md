# Contributing to EvergreenShims

Thank you for your interest in contributing! This document provides guidelines and information about contributing.

## Code of Conduct

Please read our [Code of Conduct](CODE_OF_CONDUCT.md) before contributing.

## How to Contribute

### Reporting Bugs

1. Check if the bug has already been reported in [GitHub Issues](https://github.com/WyattAu/EvergreenShims/issues)
2. If not, create a new issue with:
   - Clear title and description
   - Steps to reproduce
   - Expected vs actual behavior
   - Environment details (OS, Rust version, database version)

### Suggesting Features

1. Check [GitHub Issues](https://github.com/WyattAu/EvergreenShims/issues) for existing feature requests
2. Create a new issue with:
   - Clear title and description
   - Use case (why this feature is needed)
   - Proposed implementation (if any)

### Submitting Changes

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Make your changes
4. Write or update tests
5. Ensure all tests pass (`cargo test --workspace`)
6. Commit your changes (`git commit -m 'Add my feature'`)
7. Push to the branch (`git push origin feature/my-feature`)
8. Create a Pull Request

## Development Setup

### Prerequisites

- Rust 1.75+
- Docker (for integration tests)
- PostgreSQL client (for backup/migration tests)
- MariaDB client (for failover tests)

### Clone and Build

```bash
git clone https://github.com/WyattAu/EvergreenShims.git
cd EvergreenShims
cargo build --workspace
```

### Run Tests

```bash
# Unit tests
cargo test --workspace

# Integration tests
docker compose -f tests/docker-compose.yml up -d
cargo test --workspace --features integration
docker compose -f tests/docker-compose.yml down -v

# Chaos tests (requires root)
sudo cargo test --workspace --features chaos
```

## Code Style

### Rust

- Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
- Use `rustfmt` for formatting
- Use `clippy` for linting
- Write documentation for public APIs

### Commit Messages

- Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/)
- Examples:
  - `feat: add vault-shim for secrets rotation`
  - `fix: handle network timeout in failover-shim`
  - `docs: update README with usage examples`
  - `test: add integration tests for MariaDB failover`

### Pull Requests

- Write clear title and description
- Reference related issues (`Fixes #123`)
- Include tests for new features
- Update documentation if needed
- Keep PRs focused (one feature per PR)

## Architecture

See [docs/architecture.md](docs/architecture.md) for the overall architecture.

### Adding a New Shim

1. Create a new crate in `crates/`:
   ```bash
   cargo new crates/my-shim --lib
   ```

2. Implement the `Capability` trait:
   ```rust
   use shim_core::Capability;

   pub struct MyShim {
       config: Option<MyConfig>,
   }

   impl Capability for MyShim {
       fn name(&self) -> &str {
           "my-shim"
       }

       fn init(&mut self, config: &Config) -> Result<()> {
           // Initialize from config
           Ok(())
       }

       fn start(&mut self) -> Result<()> {
           // Start background tasks
           Ok(())
       }

       fn stop(&mut self) -> Result<()> {
           // Stop gracefully
           Ok(())
       }

       fn metrics(&self) -> Vec<Metric> {
           // Return metrics
           vec![]
       }
   }
   ```

3. Add feature flag to `crates/evergreen-shim/Cargo.toml`:
   ```toml
   [features]
   my-shim = ["dep:my-shim"]

   [dependencies]
   my-shim = { path = "../my-shim", optional = true }
   ```

4. Wire into main binary:
   ```rust
   #[cfg(feature = "my-shim")]
   capabilities.push(Box::new(MyShim::new()));
   ```

5. Write tests and documentation

## Testing

See [docs/testing.md](docs/testing.md) for the full testing strategy.

### Test Structure

```
tests/
├── integration/          # Integration tests
│   ├── postgres.rs
│   ├── mariadb.rs
│   └── redis.rs
├── chaos/                # Chaos tests
│   ├── network_partition.rs
│   ├── process_crash.rs
│   └── disk_full.rs
├── vectors/              # Test vectors
│   ├── failover_mariadb.toml
│   └── backup_postgres.toml
└── docker-compose.yml    # Test infrastructure
```

### Writing Tests

```rust
#[tokio::test]
async fn test_my_shim() {
    // Arrange
    let shim = MyShim::new();
    let config = Config::default();
    
    // Act
    shim.init(&config).unwrap();
    shim.start().unwrap();
    
    // Assert
    assert!(shim.is_healthy());
    
    // Cleanup
    shim.stop().unwrap();
}
```

## Documentation

- Write clear, concise documentation
- Include examples for all features
- Keep documentation up-to-date with code changes
- Use [Rustdoc](https://doc.rust-lang.org/rustdoc/) for API documentation

## Release Process

1. Update version in `Cargo.toml`
2. Update `CHANGELOG.md`
3. Create a release commit
4. Tag the release (`git tag v1.0.0`)
5. Push to GitHub (`git push origin main --tags`)
6. GitHub Actions will build and publish binaries

## Questions?

If you have questions, feel free to:
- Open a [GitHub Issue](https://github.com/WyattAu/EvergreenShims/issues)
- Start a [Discussion](https://github.com/WyattAu/EvergreenShims/discussions)

Thank you for contributing!
