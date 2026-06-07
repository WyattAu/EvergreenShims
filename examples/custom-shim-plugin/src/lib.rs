//! Example plugin implementing the `shim-plugin` vtable interface.
//!
//! Build with `cargo build --release` to produce a shared library, then place
//! it in a plugin directory and load it with `PluginLoader`.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);

/// VTable entry: return the plugin name.
#[unsafe(no_mangle)]
unsafe extern "C" fn plugin_name() -> *const c_char {
    static NAME: &[u8] = b"custom-shim-plugin\0";
    NAME.as_ptr() as *const c_char
}

/// VTable entry: initialize with JSON config.
#[unsafe(no_mangle)]
unsafe extern "C" fn plugin_init(config_json: *const c_char) -> i32 {
    if config_json.is_null() {
        return -1;
    }
    let _config_str = match CStr::from_ptr(config_json).to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };
    // Parse config (demonstration only).
    let _parsed: serde_json::Value = match serde_json::from_str(_config_str) {
        Ok(v) => v,
        Err(_) => return -3,
    };
    0
}

/// VTable entry: start background work.
#[unsafe(no_mangle)]
unsafe extern "C" fn plugin_start() -> i32 {
    STARTED.store(true, Ordering::SeqCst);
    0
}

/// VTable entry: stop gracefully.
#[unsafe(no_mangle)]
unsafe extern "C" fn plugin_stop() -> i32 {
    STARTED.store(false, Ordering::SeqCst);
    0
}

/// VTable entry: return metrics as JSON.
#[unsafe(no_mangle)]
unsafe extern "C" fn plugin_metrics() -> *const c_char {
    let started = STARTED.load(Ordering::SeqCst);
    let metrics = serde_json::json!({
        "started": started,
        "plugin": "custom-shim-plugin",
    });
    let json_str = metrics.to_string();
    // Leak the CString — in a real plugin you'd use a static or arena.
    let c_str = CString::new(json_str).unwrap_or_else(|_| CString::new("error").unwrap());
    c_str.into_raw() as *const c_char
}

/// Entry point called by the plugin loader. Returns a pointer to a static vtable.
///
/// # Safety
/// The returned pointer must outlive the plugin instance.
#[unsafe(no_mangle)]
unsafe extern "C" fn shim_plugin_init() -> *const PluginVTable {
    static VTABLE: PluginVTable = PluginVTable {
        name: plugin_name,
        init: plugin_init,
        start: plugin_start,
        stop: plugin_stop,
        metrics: plugin_metrics,
    };
    &VTABLE
}

/// ABI-stable vtable matching the `shim-plugin` crate definition.
#[repr(C)]
struct PluginVTable {
    name: unsafe extern "C" fn() -> *const c_char,
    init: unsafe extern "C" fn(config_json: *const c_char) -> i32,
    start: unsafe extern "C" fn() -> i32,
    stop: unsafe extern "C" fn() -> i32,
    metrics: unsafe extern "C" fn() -> *const c_char,
}
