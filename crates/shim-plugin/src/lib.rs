//! Runtime plugin loader for custom shim capabilities.
//!
//! This crate provides an ABI-stable plugin interface that allows loading
//! custom shim capabilities from shared libraries (`.so`, `.dylib`, `.dll`)
//! at runtime.
//!
//! Enable the `plugin` feature to use the runtime loader.

#[cfg(feature = "plugin")]
pub mod loader;

#[cfg(feature = "plugin")]
pub use loader::{PluginCapability, PluginLoader, PluginVTable};
