# API Stability

## Stability Policy

Starting from v1.0.0, EvergreenShims follows semantic versioning with these guarantees:

- **Major version (X.0.0):** Breaking changes to public API. No migration path guaranteed.
- **Minor version (0.X.0):** New functionality, backward-compatible. Existing code continues to work.
- **Patch version (0.0.X):** Bug fixes, backward-compatible. No API changes.

## Public API Surface

### shim-core (Foundation)

| Item | Type | Stability | Breaking Change |
|------|------|-----------|-----------------|
| `Capability` trait | Trait | Stable | Method signature change |
| `Config` struct | Struct | Stable | Field removal/renaming |
| `Metric` struct | Struct | Stable | Field change |
| `ShimBus` struct | Struct | Stable | Method removal |
| `EventType` enum | Enum | Stable | Variant removal |
| `Severity` enum | Enum | Stable | Variant removal |
| `Result` type | Type alias | Stable | Error type change |

### Shim Crates

Each shim crate exposes:

| Item | Type | Stability |
|------|------|-----------|
| `ShimName::new()` | Constructor | Stable |
| `Capability::name()` | Method | Stable |
| `Capability::init()` | Method | Stable |
| `Capability::start()` | Method | Stable |
| `Capability::stop()` | Method | Stable |
| `Capability::metrics()` | Method | Stable |
| `*::from_env()` | Constructor | Stable |

### Configuration

| Config Type | Stability |
|-------------|-----------|
| Environment variable names | Stable |
| Environment variable defaults | Stable |
| TOML config schema | Stable |
| Behavior with missing config | Stable |

## Unstable API

Items marked `#[doc(hidden)]` or `#[unstable]` are not covered by stability guarantees. They may change between any versions.

## Migration Guide

When breaking changes are introduced in a major version:

1. New functionality is added first (minor version)
2. Deprecated items are marked with `#[deprecated]` (minor version)
3. Breaking change removes deprecated items (major version)
4. Migration guide is published with the major release

## Deprecation Policy

Items are deprecated for at least one minor version before removal:

```rust
#[deprecated(since = "1.1.0", note = "Use new_method() instead")]
pub fn old_method() { ... }
```
