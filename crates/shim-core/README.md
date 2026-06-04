# EvergreenShims

Rust-native shims for building self-managing container images.

## Quick Start

```rust
use shim_core::{Capability, Config, Result};

// Your shim implements the Capability trait
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

## Documentation

- [Architecture](docs/architecture.md)
- [Testing](docs/testing.md)
- [Roadmap](docs/roadmap.md)

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](../LICENSE) for details.
