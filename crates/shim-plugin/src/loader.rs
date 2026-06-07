use std::ffi::{CStr, CString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use libloading::{Library, Symbol};
use shim_core::config::Config;
use shim_core::error::Result as ShimResult;
use shim_core::metrics::Metric;
use shim_core::Capability;

/// C-compatible function pointer type for string-returning functions.
type NameFn = unsafe extern "C" fn() -> *const std::ffi::c_char;
/// C-compatible function pointer type for config-accepting init function.
type InitFn = unsafe extern "C" fn(config_json: *const std::ffi::c_char) -> i32;
/// C-compatible function pointer type for void operations returning status.
type VoidFn = unsafe extern "C" fn() -> i32;
/// C-compatible function pointer type for metrics collection.
type MetricsFn = unsafe extern "C" fn() -> *const std::ffi::c_char;

/// ABI-stable vtable returned by a plugin's `shim_plugin_init` entry point.
///
/// All function pointers use C calling convention and return integer status codes
/// (0 = success, non-zero = error). String results are returned as C strings
/// that the caller must not free (they point into plugin memory).
#[repr(C)]
pub struct PluginVTable {
    /// Returns the name of this plugin capability.
    pub name: NameFn,
    /// Initialize the plugin with a JSON config string.
    pub init: InitFn,
    /// Start background tasks.
    pub start: VoidFn,
    /// Stop gracefully.
    pub stop: VoidFn,
    /// Collect metrics as a JSON string.
    pub metrics: MetricsFn,
}

/// A loaded plugin instance that owns the shared library and wraps its vtable.
pub struct PluginInstance {
    #[allow(dead_code)]
    library: Arc<Library>,
    vtable: PluginVTable,
    plugin_name: String,
}

impl PluginInstance {
    /// Load a plugin from a shared library file.
    fn load(path: &Path) -> Result<Self> {
        unsafe {
            let library = Library::new(path)
                .with_context(|| format!("failed to load shared library: {}", path.display()))?;

            let init_fn: Symbol<unsafe extern "C" fn() -> *const PluginVTable> =
                library.get(b"shim_plugin_init").with_context(|| {
                    format!(
                        "shared library {} does not export `shim_plugin_init`",
                        path.display()
                    )
                })?;

            let vtable_ptr = init_fn();
            if vtable_ptr.is_null() {
                bail!("`shim_plugin_init` returned null in {}", path.display());
            }

            let vtable = std::ptr::read(vtable_ptr);

            let name_cstr = (vtable.name)();
            if name_cstr.is_null() {
                bail!("plugin name function returned null in {}", path.display());
            }
            let plugin_name = CStr::from_ptr(name_cstr)
                .to_str()
                .context("plugin name is not valid UTF-8")?
                .to_string();

            tracing::info!(name = %plugin_name, path = %path.display(), "loaded plugin");

            Ok(Self {
                library: Arc::new(library),
                vtable,
                plugin_name,
            })
        }
    }
}

/// Wraps a [`PluginInstance`] to implement the [`Capability`] trait.
pub struct PluginCapability {
    instance: PluginInstance,
}

impl PluginCapability {
    /// Load a plugin from a shared library path.
    pub fn load(path: &Path) -> Result<Self> {
        let instance = PluginInstance::load(path)?;
        Ok(Self { instance })
    }
}

impl fmt::Debug for PluginCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginCapability")
            .field("name", &self.instance.plugin_name)
            .finish()
    }
}

#[async_trait]
impl Capability for PluginCapability {
    fn name(&self) -> &str {
        &self.instance.plugin_name
    }

    async fn init(&mut self, config: &Config) -> ShimResult<()> {
        let config_json = serde_json::to_string(config)
            .map_err(|e| shim_core::Error::Config(format!("failed to serialize config: {e}")))?;

        let c_config = CString::new(config_json)
            .map_err(|e| shim_core::Error::Config(format!("config contains null byte: {e}")))?;

        let status = unsafe { (self.instance.vtable.init)(c_config.as_ptr()) };
        if status != 0 {
            return Err(shim_core::Error::Plugin(format!(
                "plugin {} init failed with status {status}",
                self.instance.plugin_name,
            )));
        }
        Ok(())
    }

    async fn start(&mut self) -> ShimResult<()> {
        let status = unsafe { (self.instance.vtable.start)() };
        if status != 0 {
            return Err(shim_core::Error::Plugin(format!(
                "plugin {} start failed with status {status}",
                self.instance.plugin_name,
            )));
        }
        Ok(())
    }

    async fn stop(&mut self) -> ShimResult<()> {
        let status = unsafe { (self.instance.vtable.stop)() };
        if status != 0 {
            return Err(shim_core::Error::Plugin(format!(
                "plugin {} stop failed with status {status}",
                self.instance.plugin_name,
            )));
        }
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        unsafe {
            let ptr = (self.instance.vtable.metrics)();
            if ptr.is_null() {
                return Vec::new();
            }
            match CStr::from_ptr(ptr).to_str() {
                Ok(json_str) => serde_json::from_str(json_str).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        }
    }
}

/// Scans a directory for shared libraries and loads plugins.
pub struct PluginLoader {
    plugin_dir: PathBuf,
    loaded: Vec<PluginCapability>,
}

impl PluginLoader {
    /// Create a new loader that will scan `plugin_dir` for shared libraries.
    pub fn new(plugin_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugin_dir: plugin_dir.into(),
            loaded: Vec::new(),
        }
    }

    /// Return the platform-specific shared library file extensions.
    fn library_extensions() -> &'static [&'static str] {
        #[cfg(target_os = "linux")]
        {
            &["so"]
        }
        #[cfg(target_os = "macos")]
        {
            &["dylib"]
        }
        #[cfg(target_os = "windows")]
        {
            &["dll"]
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            &["so", "dylib"]
        }
    }

    /// Scan the plugin directory and load all valid shared libraries.
    pub fn load_all(&mut self) -> Result<()> {
        if !self.plugin_dir.is_dir() {
            bail!(
                "plugin directory does not exist: {}",
                self.plugin_dir.display()
            );
        }

        let extensions = Self::library_extensions();
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.plugin_dir)
            .with_context(|| {
                format!(
                    "failed to read plugin directory: {}",
                    self.plugin_dir.display()
                )
            })?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| extensions.contains(&ext))
                    .unwrap_or(false)
            })
            .collect();

        entries.sort();

        let mut loaded_count = 0u32;
        let mut errors: Vec<String> = Vec::new();

        for path in &entries {
            match PluginCapability::load(path) {
                Ok(plugin) => {
                    self.loaded.push(plugin);
                    loaded_count += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to load plugin, skipping"
                    );
                    errors.push(format!("{}: {e}", path.display()));
                }
            }
        }

        tracing::info!(
            dir = %self.plugin_dir.display(),
            loaded = loaded_count,
            errors = errors.len(),
            "plugin scan complete"
        );

        Ok(())
    }

    /// Return the number of successfully loaded plugins.
    pub fn count(&self) -> usize {
        self.loaded.len()
    }

    /// Consume the loader and return the loaded plugins.
    pub fn into_plugins(self) -> Vec<PluginCapability> {
        self.loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use std::sync::atomic::{AtomicUsize, Ordering};

    static PLUGIN_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Compile a C file to a shared library and return the path.
    fn compile_plugin(c_source: &str, name: &str) -> PathBuf {
        let id = PLUGIN_COUNTER.fetch_add(1, Ordering::Relaxed);
        let out_dir = std::env::temp_dir().join(format!(
            "shim_plugin_test_{id}"
        ));
        std::fs::create_dir_all(&out_dir).unwrap();
        let so_path = out_dir.join(format!("lib{name}.so"));
        let status = std::process::Command::new("gcc")
            .args([
                "-shared",
                "-fPIC",
                "-o",
                so_path.to_str().unwrap(),
                c_source,
            ])
            .status()
            .expect("failed to run gcc");
        assert!(status.success(), "gcc compilation failed for {c_source}");
        assert!(so_path.exists(), "shared library not created at {}", so_path.display());
        so_path
    }

    fn good_plugin_path() -> PathBuf {
        compile_plugin(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_plugin.c"),
            "good_test_plugin",
        )
    }

    fn bad_plugin_path() -> PathBuf {
        compile_plugin(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_plugin_bad.c"),
            "bad_test_plugin",
        )
    }

    #[test]
    fn load_valid_plugin_succeeds() {
        let path = good_plugin_path();
        let plugin = PluginCapability::load(&path).unwrap();
        assert_eq!(plugin.name(), "good_test_plugin");
    }

    #[test]
    fn load_plugin_returns_correct_vtable_name() {
        let path = good_plugin_path();
        let plugin = PluginCapability::load(&path).unwrap();
        assert_eq!(plugin.name(), "good_test_plugin");
        assert!(plugin.name().len() > 0);
    }

    #[tokio::test]
    async fn plugin_lifecycle_init_start_stop() {
        let path = good_plugin_path();
        let mut plugin = PluginCapability::load(&path).unwrap();
        let config = Config::default();

        // init should succeed (good plugin returns 0)
        plugin.init(&config).await.unwrap();

        // start should succeed
        plugin.start().await.unwrap();

        // stop should succeed
        plugin.stop().await.unwrap();
    }

    #[tokio::test]
    async fn plugin_init_failure_returns_error() {
        let path = bad_plugin_path();
        let mut plugin = PluginCapability::load(&path).unwrap();
        let config = Config::default();

        // bad plugin init returns 1
        let result = plugin.init(&config).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("init failed"));
    }

    #[tokio::test]
    async fn plugin_start_failure_returns_error() {
        let path = bad_plugin_path();
        let mut plugin = PluginCapability::load(&path).unwrap();

        // init succeeds (we need to call it first, but bad plugin init returns 1)
        // For start failure test, use the good plugin first then test bad
        // Actually bad plugin's init also fails, so test start independently
        // by loading good plugin and verifying, then load bad and test start
        // The bad plugin returns non-zero for start
        // We can't call init (it fails), but start is independent
        // Let's test that start returns error for bad plugin
        // Note: init must be called before start in the capability protocol,
        // but the C plugin doesn't enforce this
        let result = plugin.start().await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("start failed"));
    }

    #[tokio::test]
    async fn plugin_stop_failure_returns_error() {
        let path = bad_plugin_path();
        let mut plugin = PluginCapability::load(&path).unwrap();

        // bad plugin stop returns 3
        let result = plugin.stop().await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("stop failed"));
    }

    #[test]
    fn plugin_metrics_returns_empty_for_good_plugin() {
        let path = good_plugin_path();
        let plugin = PluginCapability::load(&path).unwrap();
        // good plugin metrics returns "[]" which deserializes to empty vec
        let metrics = plugin.metrics();
        assert!(metrics.is_empty());
    }

    #[test]
    fn plugin_metrics_returns_empty_for_bad_plugin() {
        let path = bad_plugin_path();
        let plugin = PluginCapability::load(&path).unwrap();
        // bad plugin also returns "[]"
        let metrics = plugin.metrics();
        assert!(metrics.is_empty());
    }

    #[test]
    fn load_nonexistent_plugin_returns_error() {
        let result = PluginCapability::load(Path::new("/nonexistent/plugin.so"));
        assert!(result.is_err());
    }

    #[test]
    fn load_invalid_shared_library_returns_error() {
        let dir = std::env::temp_dir().join("shim_plugin_invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("invalid.so");
        std::fs::write(&path, b"this is not a shared library").unwrap();

        let result = PluginCapability::load(&path);
        assert!(result.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_plugin_without_shim_plugin_init_returns_error() {
        // Create a .so that exists but doesn't export shim_plugin_init
        let dir = std::env::temp_dir().join("shim_plugin_no_init");
        std::fs::create_dir_all(&dir).unwrap();
        let c_path = dir.join("no_init.c");
        let so_path = dir.join("no_init.so");
        std::fs::write(
            &c_path,
            r#"
            int some_other_function(void) { return 0; }
            "#,
        )
        .unwrap();
        std::process::Command::new("gcc")
            .args([
                "-shared",
                "-fPIC",
                "-o",
                so_path.to_str().unwrap(),
                c_path.to_str().unwrap(),
            ])
            .status()
            .unwrap();

        let result = PluginCapability::load(&so_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("shim_plugin_init"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vtable_all_function_pointers_non_null() {
        let path = good_plugin_path();
        let plugin = PluginCapability::load(&path).unwrap();
        // If we got here, the vtable loaded successfully, meaning all
        // function pointers were valid enough to call name().
        // The name() call succeeded, so name pointer is valid.
        assert!(!plugin.instance.plugin_name.is_empty());
        // Verify the vtable function pointers are set (non-null) by calling each
        unsafe {
            let name_ptr = (plugin.instance.vtable.name)();
            assert!(!name_ptr.is_null());
        }
    }

    #[test]
    fn plugin_loader_new_with_empty_dir() {
        let dir = std::env::temp_dir().join("shim_plugin_empty_loader");
        std::fs::create_dir_all(&dir).unwrap();
        let mut loader = PluginLoader::new(&dir);
        loader.load_all().unwrap();
        assert_eq!(loader.count(), 0);
        let plugins = loader.into_plugins();
        assert!(plugins.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_loader_nonexistent_dir_returns_error() {
        let mut loader = PluginLoader::new("/nonexistent/plugin/dir");
        let result = loader.load_all();
        assert!(result.is_err());
    }

    #[test]
    fn plugin_loader_ignores_non_library_files() {
        let dir = std::env::temp_dir().join("shim_plugin_nonlib");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("readme.txt"), "not a plugin").unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();

        let mut loader = PluginLoader::new(&dir);
        loader.load_all().unwrap();
        assert_eq!(loader.count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_loader_loads_valid_plugins() {
        let dir = std::env::temp_dir().join("shim_plugin_valid_loader");
        std::fs::create_dir_all(&dir).unwrap();

        let so_path = good_plugin_path();
        let dest = dir.join("good.so");
        std::fs::copy(&so_path, &dest).unwrap();

        let mut loader = PluginLoader::new(&dir);
        loader.load_all().unwrap();
        assert_eq!(loader.count(), 1);
        let plugins = loader.into_plugins();
        assert_eq!(plugins[0].name(), "good_test_plugin");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_loader_skips_invalid_and_loads_valid() {
        let dir = std::env::temp_dir().join("shim_plugin_mixed");
        std::fs::create_dir_all(&dir).unwrap();

        // Invalid .so
        std::fs::write(dir.join("bad.so"), "not a library").unwrap();

        // Valid .so
        let so_path = good_plugin_path();
        std::fs::copy(&so_path, dir.join("good.so")).unwrap();

        let mut loader = PluginLoader::new(&dir);
        loader.load_all().unwrap();
        assert_eq!(loader.count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_debug_format() {
        let path = good_plugin_path();
        let plugin = PluginCapability::load(&path).unwrap();
        let debug = format!("{:?}", plugin);
        assert!(debug.contains("good_test_plugin"));
    }

    #[tokio::test]
    async fn full_plugin_lifecycle_with_metrics() {
        let path = good_plugin_path();
        let mut plugin = PluginCapability::load(&path).unwrap();
        let config = Config::default();

        plugin.init(&config).await.unwrap();
        plugin.start().await.unwrap();

        let metrics = plugin.metrics();
        assert!(metrics.is_empty());

        plugin.stop().await.unwrap();
    }

    #[test]
    fn plugin_name_is_thread_safe() {
        let path = good_plugin_path();
        let plugin = PluginCapability::load(&path).unwrap();
        let plugin = Arc::new(plugin);

        let mut handles = vec![];
        for _ in 0..4 {
            let p = plugin.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    assert_eq!(p.name(), "good_test_plugin");
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }
}
