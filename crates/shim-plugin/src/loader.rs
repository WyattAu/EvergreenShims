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
