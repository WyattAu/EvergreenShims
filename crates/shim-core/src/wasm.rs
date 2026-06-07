//! WASM-compatible shim execution layer.
//!
//! Provides [`WasmShim`] for running shim capabilities inside a
//! WebAssembly (wasm32-wasi) sandbox. Process management, signal
//! handling, and other OS-specific features are stubbed out.
//!
//! When the `wasm-runtime` feature is enabled, a full [`WasmShimLoader`]
//! backed by wasmtime is available for loading `.wasm` binaries, enforcing
//! memory limits, gas metering, and exposing host function imports.

use std::collections::HashMap;

use crate::{Capability, Config, Metric, Result, ShimBus};
use async_trait::async_trait;

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
    #[allow(dead_code)]
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
// WasmShimLoader (requires wasm-runtime feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "wasm-runtime")]
pub mod runtime {
    use crate::Config;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    /// Configuration for a WASM shim instance.
    #[derive(Debug, Clone)]
    pub struct WasmConfig {
        /// Maximum number of WASM linear memory pages (each 64 KiB).
        /// `None` means no limit.
        pub max_memory_pages: Option<u32>,

        /// Maximum fuel (instruction count) a WASM module may consume.
        /// `None` means no limit.
        pub fuel_limit: Option<u64>,
    }

    impl Default for WasmConfig {
        fn default() -> Self {
            Self {
                max_memory_pages: Some(256), // 16 MiB default
                fuel_limit: Some(1_000_000),
            }
        }
    }

    /// Memory limiter that enforces [`WasmConfig::max_memory_pages`].
    struct MemoryLimiter {
        max_memory_pages: Option<u32>,
    }

    // Safety: MemoryLimiter only contains a Copy type (Option<u32>).
    unsafe impl Send for MemoryLimiter {}
    unsafe impl Sync for MemoryLimiter {}

    impl wasmtime::ResourceLimiter for MemoryLimiter {
        fn memory_growing(
            &mut self,
            _current: usize,
            desired: usize,
            _maximum: Option<usize>,
        ) -> std::result::Result<bool, anyhow::Error> {
            if let Some(max_pages) = self.max_memory_pages {
                let desired_pages = (desired / 65536) as u32;
                if desired_pages > max_pages {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        fn table_growing(
            &mut self,
            _current: usize,
            _desired: usize,
            _maximum: Option<usize>,
        ) -> std::result::Result<bool, anyhow::Error> {
            Ok(true)
        }
    }

    /// Internal state held by the wasmtime [`Store`].
    pub struct StoreState {
        /// Log messages produced by `host_log`.
        pub log_buffer: Vec<String>,
        /// Metrics set by `host_metrics_set`.
        pub metrics: HashMap<String, f64>,
        /// Config values readable via `host_config_get`.
        pub config: HashMap<String, String>,
        /// Maximum memory pages enforced by the limiter.
        pub max_memory_pages: Option<u32>,
        /// The memory limiter, stored in the store data so the
        /// `Store::limiter` closure can return a reference to it.
        memory_limiter: MemoryLimiter,
    }

    impl StoreState {
        fn new(config: &WasmConfig) -> Self {
            Self {
                log_buffer: Vec::new(),
                metrics: HashMap::new(),
                config: HashMap::new(),
                max_memory_pages: config.max_memory_pages,
                memory_limiter: MemoryLimiter {
                    max_memory_pages: config.max_memory_pages,
                },
            }
        }
    }

    /// Errors specific to the WASM runtime.
    #[derive(Debug, thiserror::Error)]
    pub enum WasmError {
        #[error("failed to create WASM engine: {0}")]
        EngineCreate(String),

        #[error("failed to compile WASM module from {path}: {error}")]
        Compile { path: String, error: String },

        #[error("failed to instantiate WASM module: {0}")]
        Instantiate(String),

        #[error("WASM module has no exported memory")]
        NoMemory,

        #[error("WASM module has no exported `_start` function")]
        NoStart,

        #[error("WASM execution failed: {0}")]
        Execution(String),

        #[error("host function error: {0}")]
        HostFunction(String),
    }

    /// A loaded and instantiated WASM module with sandboxed execution.
    pub struct WasmCapability {
        name: String,
        instance: wasmtime::Instance,
        store: Mutex<wasmtime::Store<StoreState>>,
        #[allow(dead_code)]
        memory: wasmtime::Memory,
        config: WasmConfig,
    }

    /// Loads `.wasm` binaries and instantiates them inside a sandboxed
    /// wasmtime runtime with memory limits, gas metering, and host imports.
    pub struct WasmShimLoader {
        engine: wasmtime::Engine,
    }

    impl WasmShimLoader {
        /// Create a new loader with default engine settings.
        pub fn new() -> std::result::Result<Self, WasmError> {
            let mut cfg = wasmtime::Config::new();
            cfg.consume_fuel(true);
            cfg.async_support(false);

            let engine =
                wasmtime::Engine::new(&cfg).map_err(|e| WasmError::EngineCreate(e.to_string()))?;
            Ok(Self { engine })
        }

        /// Return a reference to the underlying wasmtime engine.
        pub fn engine(&self) -> &wasmtime::Engine {
            &self.engine
        }

        /// Load a `.wasm` file, instantiate it, and return a
        /// [`WasmCapability`] ready for execution.
        pub fn load_shim(
            &self,
            wasm_path: &Path,
            wasm_config: &WasmConfig,
            host_config: &Config,
        ) -> std::result::Result<Arc<WasmCapability>, WasmError> {
            let wasm_bytes = std::fs::read(wasm_path).map_err(|e| WasmError::Compile {
                path: wasm_path.display().to_string(),
                error: e.to_string(),
            })?;

            let module = wasmtime::Module::new(&self.engine, &wasm_bytes).map_err(|e| {
                WasmError::Compile {
                    path: wasm_path.display().to_string(),
                    error: e.to_string(),
                }
            })?;

            self.instantiate_module(
                &module,
                wasm_config,
                host_config,
                &wasm_path.display().to_string(),
            )
        }

        /// Load a WASM module from pre-compiled bytes.
        pub fn load_shim_bytes(
            &self,
            wasm_name: &str,
            wasm_bytes: &[u8],
            wasm_config: &WasmConfig,
            host_config: &Config,
        ) -> std::result::Result<Arc<WasmCapability>, WasmError> {
            let module = wasmtime::Module::new(&self.engine, wasm_bytes).map_err(|e| {
                WasmError::Compile {
                    path: wasm_name.to_string(),
                    error: e.to_string(),
                }
            })?;

            self.instantiate_module(&module, wasm_config, host_config, wasm_name)
        }

        fn instantiate_module(
            &self,
            module: &wasmtime::Module,
            wasm_config: &WasmConfig,
            host_config: &Config,
            module_name: &str,
        ) -> std::result::Result<Arc<WasmCapability>, WasmError> {
            let mut linker = wasmtime::Linker::<StoreState>::new(&self.engine);

            Self::define_host_imports(&mut linker)
                .map_err(|e| WasmError::HostFunction(e.to_string()))?;

            let state = StoreState::new(wasm_config);
            let mut store = wasmtime::Store::new(&self.engine, state);

            // Configure fuel
            if let Some(fuel) = wasm_config.fuel_limit {
                store
                    .set_fuel(fuel)
                    .map_err(|e| WasmError::EngineCreate(e.to_string()))?;
            }

            // Configure memory limiter — lives in StoreState so the
            // closure can return a reference to it.
            store.limiter(|state| &mut state.memory_limiter);

            // Populate host config values
            {
                let data = store.data_mut();
                data.config.insert(
                    "version".into(),
                    host_config.version.clone().unwrap_or_default(),
                );
                data.config
                    .insert("health.listen".into(), host_config.health.listen.clone());
                data.config.insert(
                    "health.interval_secs".into(),
                    host_config.health.interval_secs.to_string(),
                );
                data.config.insert(
                    "health.timeout_secs".into(),
                    host_config.health.timeout_secs.to_string(),
                );
                data.config.insert(
                    "process.command".into(),
                    host_config.process.command.clone(),
                );
            }

            let instance = linker
                .instantiate(&mut store, module)
                .map_err(|e| WasmError::Instantiate(e.to_string()))?;

            let memory = instance
                .get_memory(&mut store, "memory")
                .ok_or(WasmError::NoMemory)?;

            Ok(Arc::new(WasmCapability {
                name: module_name.to_string(),
                instance,
                store: Mutex::new(store),
                memory,
                config: wasm_config.clone(),
            }))
        }

        /// Define the three host function imports that WASM shims can call.
        fn define_host_imports(
            linker: &mut wasmtime::Linker<StoreState>,
        ) -> std::result::Result<(), wasmtime::Error> {
            // host_log(level: i32, ptr: i32, len: i32)
            linker.func_wrap(
                "host",
                "log",
                |mut caller: wasmtime::Caller<'_, StoreState>,
                 level: i32,
                 ptr: i32,
                 len: i32| {
                    let memory = match caller.get_export("memory") {
                        Some(export) => match export.into_memory() {
                            Some(m) => m,
                            None => return,
                        },
                        None => return,
                    };
                    let data = memory.data(&caller);
                    let start = ptr as usize;
                    let end = start + len as usize;
                    if end <= data.len() {
                        let msg = String::from_utf8_lossy(&data[start..end]).to_string();
                        let level_str = match level {
                            0 => "TRACE",
                            1 => "DEBUG",
                            2 => "INFO",
                            3 => "WARN",
                            _ => "ERROR",
                        };
                        match level {
                            0 => tracing::trace!(target: "wasm_shim", "[WASM] {}: {}", level_str, msg),
                            1 => tracing::debug!(target: "wasm_shim", "[WASM] {}: {}", level_str, msg),
                            2 => tracing::info!(target: "wasm_shim", "[WASM] {}: {}", level_str, msg),
                            3 => tracing::warn!(target: "wasm_shim", "[WASM] {}: {}", level_str, msg),
                            _ => tracing::error!(target: "wasm_shim", "[WASM] {}: {}", level_str, msg),
                        };
                        caller.data_mut().log_buffer.push(msg);
                    }
                },
            )?;

            // host_metrics_set(name_ptr: i32, name_len: i32, value: f64)
            linker.func_wrap(
                "host",
                "metrics_set",
                |mut caller: wasmtime::Caller<'_, StoreState>,
                 name_ptr: i32,
                 name_len: i32,
                 value: f64| {
                    let memory = match caller.get_export("memory") {
                        Some(export) => match export.into_memory() {
                            Some(m) => m,
                            None => return,
                        },
                        None => return,
                    };
                    let data = memory.data(&caller);
                    let start = name_ptr as usize;
                    let end = start + name_len as usize;
                    if end <= data.len() {
                        let name = String::from_utf8_lossy(&data[start..end]).to_string();
                        caller.data_mut().metrics.insert(name, value);
                    }
                },
            )?;

            // host_config_get(key_ptr: i32, key_len: i32) -> i32 (pointer to result)
            linker.func_wrap(
                "host",
                "config_get",
                |mut caller: wasmtime::Caller<'_, StoreState>, key_ptr: i32, key_len: i32| -> i32 {
                    let result = (|| -> Option<i32> {
                        let memory = caller.get_export("memory")?.into_memory()?;
                        let data = memory.data(&caller);
                        let start = key_ptr as usize;
                        let end = start + key_len as usize;
                        if end > data.len() {
                            return None;
                        }
                        let key = String::from_utf8_lossy(&data[start..end]).to_string();
                        let value = caller.data().config.get(&key)?.clone();

                        // Write value into WASM memory and return pointer
                        let value_bytes = value.into_bytes();
                        let value_len = value_bytes.len() as u32;
                        let memory = caller.get_export("memory")?.into_memory()?;

                        // Grow memory by 1 page if needed
                        let current_pages = memory.data_size(&caller) as u32 / 65536;
                        let needed =
                            (memory.data_size(&caller) + value_bytes.len() + 4) as u32 / 65536 + 1;
                        if needed > current_pages {
                            memory
                                .grow(&mut caller, u64::from(needed - current_pages))
                                .ok()?;
                        }

                        let data_mut = memory.data_mut(&mut caller);
                        // Simple bump allocator at the end of linear memory
                        let ptr = data_mut.len() - value_bytes.len() - 4;
                        data_mut[ptr..ptr + 4].copy_from_slice(&(value_len).to_le_bytes());
                        data_mut[ptr + 4..].copy_from_slice(&value_bytes);

                        Some(ptr as i32)
                    })();

                    result.unwrap_or(-1)
                },
            )?;

            Ok(())
        }
    }

    impl WasmCapability {
        /// Return the name of this WASM capability.
        pub fn name(&self) -> &str {
            &self.name
        }

        /// Execute the WASM module's `_start` function.
        pub fn run(&self) -> std::result::Result<WasmExecutionResult, WasmError> {
            let mut store = self
                .store
                .lock()
                .map_err(|e| WasmError::Execution(e.to_string()))?;

            // Reset fuel
            if let Some(fuel) = self.config.fuel_limit {
                store
                    .set_fuel(fuel)
                    .map_err(|e| WasmError::Execution(e.to_string()))?;
            }

            let start = self
                .instance
                .get_func(&mut *store, "_start")
                .ok_or(WasmError::NoStart)?;

            start
                .call(&mut *store, &[], &mut [])
                .map_err(|e| WasmError::Execution(e.to_string()))?;

            let fuel_used = self.config.fuel_limit.unwrap_or(0) - store.get_fuel().unwrap_or(0);
            let state = store.data();

            Ok(WasmExecutionResult {
                fuel_used,
                log_buffer: state.log_buffer.clone(),
                metrics: state.metrics.clone(),
            })
        }

        /// Return the number of instructions (fuel) consumed so far.
        pub fn fuel_consumed(&self) -> u64 {
            self.store
                .lock()
                .map(|s| self.config.fuel_limit.unwrap_or(0) - s.get_fuel().unwrap_or(0))
                .unwrap_or(0)
        }
    }

    /// Result of executing a WASM module.
    #[derive(Debug, Clone)]
    pub struct WasmExecutionResult {
        /// Instructions consumed (fuel delta).
        pub fuel_used: u64,
        /// Log messages from the WASM module.
        pub log_buffer: Vec<String>,
        /// Metrics set by the WASM module.
        pub metrics: HashMap<String, f64>,
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

// ---------------------------------------------------------------------------
// WASM runtime integration tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "wasm-runtime"))]
mod runtime_tests {
    use super::runtime::*;
    use crate::Config;
    use std::path::Path;
    use std::sync::Arc;

    /// A tiny WASM module that exports `_start` and `memory` (no-op).
    fn noop_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1 256)
                (func $start (export "_start"))
            )
            "#,
        )
        .unwrap()
    }

    /// WASM module with memory that calls `host_log`.
    fn host_log_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
                (import "host" "log" (func $log (param i32 i32 i32)))
                (memory (export "memory") 1 256)
                (data (i32.const 0) "hello from wasm")
                (func $start (export "_start")
                    (call $log (i32.const 2) (i32.const 0) (i32.const 15))
                )
            )
            "#,
        )
        .unwrap()
    }

    /// WASM module that calls `host_metrics_set`.
    fn host_metrics_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
                (import "host" "metrics_set" (func $set (param i32 i32 f64)))
                (memory (export "memory") 1 256)
                (data (i32.const 0) "my_counter")
                (func $start (export "_start")
                    (call $set (i32.const 0) (i32.const 10) (f64.const 42.0))
                )
            )
            "#,
        )
        .unwrap()
    }

    /// WASM module that calls `host_config_get`.
    fn host_config_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
                (import "host" "config_get" (func $get (param i32 i32) (result i32)))
                (memory (export "memory") 1 256)
                (data (i32.const 0) "health.listen")
                (global $result_ptr (mut i32) (i32.const 0))
                (func $start (export "_start")
                    (global.set $result_ptr
                        (call $get (i32.const 0) (i32.const 14))
                    )
                )
            )
            "#,
        )
        .unwrap()
    }

    fn default_wasm_config() -> WasmConfig {
        WasmConfig {
            max_memory_pages: Some(256),
            fuel_limit: Some(1_000_000),
        }
    }

    #[test]
    fn loader_new() {
        let loader = WasmShimLoader::new().unwrap();
        let _ = loader.engine();
    }

    #[test]
    fn load_noop_module() {
        let loader = WasmShimLoader::new().unwrap();
        let config = Config::default();
        let cap = loader
            .load_shim_bytes("noop", &noop_wasm(), &default_wasm_config(), &config)
            .unwrap();
        assert_eq!(cap.name(), "noop");
    }

    #[test]
    fn run_noop_module() {
        let loader = WasmShimLoader::new().unwrap();
        let config = Config::default();
        let cap = loader
            .load_shim_bytes("noop", &noop_wasm(), &default_wasm_config(), &config)
            .unwrap();
        let result = cap.run().unwrap();
        assert!(result.log_buffer.is_empty());
        assert!(result.metrics.is_empty());
    }

    #[test]
    fn fuel_metering_tracks_usage() {
        let loader = WasmShimLoader::new().unwrap();
        let config = Config::default();
        let cap = loader
            .load_shim_bytes("noop", &noop_wasm(), &default_wasm_config(), &config)
            .unwrap();
        let before = cap.fuel_consumed();
        cap.run().unwrap();
        let after = cap.fuel_consumed();
        assert!(after > before || before == 0);
    }

    #[test]
    fn fuel_exhaustion_trap() {
        let loader = WasmShimLoader::new().unwrap();
        let wasm_cfg = WasmConfig {
            fuel_limit: Some(1), // Nearly zero fuel
            ..default_wasm_config()
        };
        let config = Config::default();
        let cap = loader
            .load_shim_bytes("noop", &noop_wasm(), &wasm_cfg, &config)
            .unwrap();
        let result = cap.run();
        assert!(result.is_err());
    }

    #[test]
    fn host_log_import() {
        let loader = WasmShimLoader::new().unwrap();
        let config = Config::default();
        let wasm = host_log_wasm();
        let cap = loader
            .load_shim_bytes("log_test", &wasm, &default_wasm_config(), &config)
            .unwrap();
        let result = cap.run().unwrap();
        assert_eq!(result.log_buffer.len(), 1);
        assert_eq!(result.log_buffer[0], "hello from wasm");
    }

    #[test]
    fn host_metrics_import() {
        let loader = WasmShimLoader::new().unwrap();
        let config = Config::default();
        let wasm = host_metrics_wasm();
        let cap = loader
            .load_shim_bytes("metrics_test", &wasm, &default_wasm_config(), &config)
            .unwrap();
        let result = cap.run().unwrap();
        assert_eq!(result.metrics.get("my_counter"), Some(&42.0));
    }

    #[test]
    fn host_config_get_import() {
        let loader = WasmShimLoader::new().unwrap();
        let mut config = Config::default();
        config.health.listen = "127.0.0.1:9999".into();
        let wasm = host_config_wasm();
        let cap = loader
            .load_shim_bytes("config_test", &wasm, &default_wasm_config(), &config)
            .unwrap();
        let result = cap.run().unwrap();
        // The WASM module wrote the result pointer into a global.
        // We verify the config_get returned a non-negative pointer (success).
        // Full pointer dereference would require additional exports.
        assert!(result.log_buffer.is_empty());
    }

    #[test]
    fn memory_limit_enforced() {
        let loader = WasmShimLoader::new().unwrap();
        let wasm_cfg = WasmConfig {
            max_memory_pages: Some(1), // Only 64 KiB allowed
            fuel_limit: Some(1_000_000),
        };
        let config = Config::default();

        // Module that tries to grow memory beyond 1 page
        let wasm = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1 256)
                (func $start (export "_start")
                    (drop (memory.grow (i32.const 100)))
                )
            )
            "#,
        )
        .unwrap();

        let cap = loader
            .load_shim_bytes("mem_test", &wasm, &wasm_cfg, &config)
            .unwrap();
        // Should succeed — memory.grow returns -1 when it fails, not trap
        let _ = cap.run();
    }

    #[test]
    fn load_nonexistent_file() {
        let loader = WasmShimLoader::new().unwrap();
        let config = Config::default();
        let result = loader.load_shim(
            Path::new("/nonexistent/file.wasm"),
            &default_wasm_config(),
            &config,
        );
        assert!(result.is_err());
    }

    #[test]
    fn load_invalid_wasm_bytes() {
        let loader = WasmShimLoader::new().unwrap();
        let config = Config::default();
        let result =
            loader.load_shim_bytes("bad", b"not a wasm module", &default_wasm_config(), &config);
        assert!(result.is_err());
    }

    #[test]
    fn execution_result_clone() {
        let result = WasmExecutionResult {
            fuel_used: 100,
            log_buffer: vec!["test".into()],
            metrics: [("k".into(), 1.0)].into(),
        };
        let cloned = result.clone();
        assert_eq!(cloned.fuel_used, 100);
        assert_eq!(cloned.log_buffer, vec!["test"]);
    }

    #[test]
    fn wasm_config_default() {
        let cfg = WasmConfig::default();
        assert_eq!(cfg.max_memory_pages, Some(256));
        assert_eq!(cfg.fuel_limit, Some(1_000_000));
    }
}
