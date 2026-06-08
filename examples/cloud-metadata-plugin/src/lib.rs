use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static STARTED: AtomicBool = AtomicBool::new(false);
static TOKIO_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static METADATA_URL: OnceLock<String> = OnceLock::new();

fn default_metadata_url() -> String {
    // Default to AWS EC2 metadata endpoint
    "http://169.254.169.254/latest/meta-data/".to_string()
}

fn get_runtime() -> &'static tokio::runtime::Runtime {
    TOKIO_RT.get_or_init(|| {
        tokio::runtime::Runtime::new().expect("failed to create tokio runtime")
    })
}

fn get_metadata_url() -> &'static str {
    METADATA_URL.get_or_init(default_metadata_url).as_str()
}

async fn fetch_metadata(path: &str) -> Result<String, String> {
    let url = format!("{}{}", get_metadata_url(), path);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| format!("failed to create client: {e}"))?;

    let resp = client
        .get(&url)
        .header("X-aws-ec2-metadata-token-ttl-seconds", "21600")
        .send()
        .await
        .map_err(|e| format!("metadata request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("metadata returned status: {}", resp.status()));
    }

    resp.text()
        .await
        .map_err(|e| format!("failed to read response: {e}"))
}

async fn collect_metrics() -> serde_json::Value {
    let mut metrics = serde_json::json!({
        "instance_type": null,
        "instance_id": null,
        "region": null,
        "availability_zone": null,
        "ami_id": null,
        "hostname": null,
        "local_ipv4": null,
    });

    let fields: &[(&str, &str)] = &[
        ("instance_type", "instance-type"),
        ("instance_id", "instance-id"),
        ("ami_id", "ami-id"),
        ("hostname", "hostname"),
        ("local_ipv4", "local-ipv4"),
    ];

    for (key, path) in fields {
        if let Ok(val) = fetch_metadata(path).await {
            if let Some(obj) = metrics.as_object_mut() {
                obj.insert(key.to_string(), serde_json::Value::String(val));
            }
        }
    }

    // Try to get region from placement/availability-zone
    if let Ok(az) = fetch_metadata("placement/availability-zone").await {
        let region = az.trim_end_matches(|c: char| c.is_ascii_alphabetic()).to_string();
        if let Some(obj) = metrics.as_object_mut() {
            obj.insert(
                "availability_zone".to_string(),
                serde_json::Value::String(az.trim().to_string()),
            );
            obj.insert("region".to_string(), serde_json::Value::String(region));
        }
    }

    metrics
}

/// VTable entry: return the plugin name.
#[unsafe(no_mangle)]
unsafe extern "C" fn plugin_name() -> *const c_char {
    static NAME: &[u8] = b"cloud-metadata\0";
    NAME.as_ptr() as *const c_char
}

/// VTable entry: initialize with JSON config.
#[unsafe(no_mangle)]
unsafe extern "C" fn plugin_init(config_json: *const c_char) -> i32 {
    if config_json.is_null() {
        // Use defaults
        return 0;
    }

    let config_str = match CStr::from_ptr(config_json).to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let config: serde_json::Value = match serde_json::from_str(config_str) {
        Ok(v) => v,
        Err(_) => return -3,
    };

    // Extract custom metadata URL from config
    if let Some(url) = config.get("metadata_url").and_then(|v| v.as_str()) {
        let _ = METADATA_URL.set(url.to_string());
    }

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
    if !STARTED.load(Ordering::SeqCst) {
        let c_str = CString::new("[]").unwrap();
        return c_str.into_raw() as *const c_char;
    }

    let metrics = get_runtime().block_on(collect_metrics());

    let result = serde_json::json!([{
        "name": "cloud_metadata",
        "value": 1.0,
        "labels": {
            "instance_type": metrics.get("instance_type").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "instance_id": metrics.get("instance_id").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "region": metrics.get("region").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "availability_zone": metrics.get("availability_zone").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "ami_id": metrics.get("ami_id").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "hostname": metrics.get("hostname").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "local_ipv4": metrics.get("local_ipv4").and_then(|v| v.as_str()).unwrap_or("unknown"),
        },
        "type": "gauge"
    }]);

    let json_str = result.to_string();
    let c_str = CString::new(json_str).unwrap_or_else(|_| CString::new("[]").unwrap());
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
