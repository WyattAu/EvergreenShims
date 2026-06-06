//! WASM-compatible shim execution layer.
//!
//! Provides [`WasmShim`] for running shim capabilities inside a
//! WebAssembly (wasm32-wasi) sandbox. Process management, signal
//! handling, and other OS-specific features are stubbed out.

use std::collections::HashMap;

use crate::{Capability, Config, Metric, Result, ShimBus};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// WASM Capability trait
// ---------------------------------------------------------------------------

/// Extension trait that adds WASM-specific lifecycle hooks to [`Capability`].
#[async_trait]
pub trait WasmCapability: Capability {
    /// Called when the WASM module is being instantiated.
    /// Use this for any pre-init setup that only makes sense inside a sandbox.
    async fn wasm_init(&mut self) -> Result<()> {
        Ok(())
    }

    /// Return a snapshot of the sandbox memory usage in bytes.
    /// Defaults to 0 when the host cannot provide this information.
    fn memory_usage_bytes(&self) -> u64 {
        0
    }

    /// Return a snapshot of the sandbox CPU time consumed in microseconds.
    /// Defaults to 0 when the host cannot provide this information.
    fn cpu_time_us(&self) -> u64 {
        0
    }
}

// ---------------------------------------------------------------------------
// WasmShim
// ---------------------------------------------------------------------------

/// A wrapper that executes a [`Capability`] inside a WASM sandbox.
pub struct WasmShim {
    name: String,
    inner: Box<dyn WasmCapability>,
    config: Option<Config>,
    bus: Option<ShimBus>,
    metrics: Vec<Metric>,
    counters: HashMap<String, u64>,
}

impl WasmShim {
    /// Create a new WASM shim wrapping the given capability.
    pub fn new(name: impl Into<String>, inner: Box<dyn WasmCapability>) -> Self {
        Self {
            name: name.into(),
            inner,
            config: None,
            bus: None,
            metrics: Vec::new(),
            counters: HashMap::new(),
        }
    }

    /// Return the memory usage of the inner capability.
    pub fn memory_usage(&self) -> u64 {
        self.inner.memory_usage_bytes()
    }

    /// Return the CPU time consumed by the inner capability.
    pub fn cpu_time(&self) -> u64 {
        self.inner.cpu_time_us()
    }

    /// Increment a named counter (useful for WASM-side bookkeeping).
    pub fn increment_counter(&mut self, key: &str) {
        *self.counters.entry(key.to_string()).or_insert(0) += 1;
    }

    /// Read the current value of a named counter.
    pub fn counter(&self, key: &str) -> u64 {
        self.counters.get(key).copied().unwrap_or(0)
    }
}

#[async_trait]
impl Capability for WasmShim {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        self.config = Some(config.clone());
        self.inner.wasm_init().await?;
        self.inner.init(config).await
    }

    fn set_bus(&mut self, bus: ShimBus) {
        self.bus = Some(bus.clone());
        self.inner.set_bus(bus);
    }

    async fn start(&mut self) -> Result<()> {
        self.inner.start().await
    }

    async fn stop(&mut self) -> Result<()> {
        self.inner.stop().await
    }

    fn metrics(&self) -> Vec<Metric> {
        let mut m = self.inner.metrics();
        m.push(Metric::new(
            "wasm_memory_bytes",
            self.inner.memory_usage_bytes() as f64,
        ));
        m.push(Metric::new(
            "wasm_cpu_time_us",
            self.inner.cpu_time_us() as f64,
        ));
        for (k, v) in &self.counters {
            m.push(Metric::new(&format!("wasm_counter_{}", k), *v as f64));
        }
        m
    }
}

// ---------------------------------------------------------------------------
// Stub module – OS-specific code compiled away in WASM builds
// ---------------------------------------------------------------------------

/// Stub for process management. No-ops in WASM.
pub mod process_stub {
    /// Stub process handle. Does nothing.
    pub struct StubProcess;

    impl StubProcess {
        /// No-op start.
        pub fn start(&self) -> std::io::Result<()> {
            Ok(())
        }

        /// No-op stop.
        pub fn stop(&self) -> std::io::Result<()> {
            Ok(())
        }

        /// Always reports as not running.
        pub fn is_running(&self) -> bool {
            false
        }
    }
}

/// Stub for signal handling. No-ops in WASM.
pub mod signal_stub {
    /// Stub signal handler. Does nothing.
    pub struct StubSignalHandler;

    impl StubSignalHandler {
        /// No-op wait.
        pub async fn wait(&self) -> std::io::Result<()> {
            std::future::pending().await
        }

        /// No-op shutdown check.
        pub fn should_shutdown(&self) -> bool {
            false
        }
    }
}

/// Stub for process spawning. Always fails in WASM.
pub mod spawn_stub {
    use std::process::Command;

    /// Attempt to spawn a process. Always returns an error in WASM.
    pub fn spawn(_cmd: &mut Command) -> std::io::Result<std::process::Child> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "process spawning is not supported in WASM",
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct DummyCapability {
        started: bool,
        stopped: bool,
    }

    impl DummyCapability {
        fn new() -> Self {
            Self {
                started: false,
                stopped: false,
            }
        }
    }

    #[async_trait]
    impl Capability for DummyCapability {
        fn name(&self) -> &str {
            "dummy"
        }

        async fn init(&mut self, _config: &Config) -> Result<()> {
            Ok(())
        }

        async fn start(&mut self) -> Result<()> {
            self.started = true;
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.stopped = true;
            Ok(())
        }

        fn metrics(&self) -> Vec<Metric> {
            vec![]
        }
    }

    #[async_trait]
    impl WasmCapability for DummyCapability {
        async fn wasm_init(&mut self) -> Result<()> {
            Ok(())
        }

        fn memory_usage_bytes(&self) -> u64 {
            1024
        }

        fn cpu_time_us(&self) -> u64 {
            500
        }
    }

    #[tokio::test]
    async fn wasm_shim_name() {
        let shim = WasmShim::new("test", Box::new(DummyCapability::new()));
        assert_eq!(shim.name(), "test");
    }

    #[tokio::test]
    async fn wasm_shim_lifecycle() {
        let mut shim = WasmShim::new("test", Box::new(DummyCapability::new()));
        let config = Config::default();
        shim.init(&config).await.unwrap();
        shim.start().await.unwrap();
        shim.stop().await.unwrap();
    }

    #[tokio::test]
    async fn wasm_shim_metrics_include_sandbox_info() {
        let shim = WasmShim::new("test", Box::new(DummyCapability::new()));
        let metrics = shim.metrics();
        assert!(metrics.iter().any(|m| m.name == "wasm_memory_bytes"));
        assert!(metrics.iter().any(|m| m.name == "wasm_cpu_time_us"));
    }

    #[tokio::test]
    async fn wasm_shim_counters() {
        let mut shim = WasmShim::new("test", Box::new(DummyCapability::new()));
        assert_eq!(shim.counter("foo"), 0);
        shim.increment_counter("foo");
        shim.increment_counter("foo");
        assert_eq!(shim.counter("foo"), 2);

        let metrics = shim.metrics();
        assert!(metrics
            .iter()
            .any(|m| m.name == "wasm_counter_foo" && (m.value - 2.0).abs() < f64::EPSILON));
    }

    #[test]
    fn stub_process_always_not_running() {
        let p = process_stub::StubProcess;
        assert!(!p.is_running());
    }

    #[test]
    fn stub_signal_handler_not_shutdown() {
        let h = signal_stub::StubSignalHandler;
        assert!(!h.should_shutdown());
    }

    #[test]
    fn stub_spawn_fails() {
        let err = spawn_stub::spawn(&mut Command::new("echo")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    }
}
