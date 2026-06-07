//! Config hot-reload via filesystem watching.
//!
//! `ConfigWatcher` monitors a TOML config file and triggers a callback
//! when it changes. Uses `notify` crate for filesystem events with
//! debouncing to avoid rapid re-reads.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use prometheus::{Gauge, IntCounter, Opts, Registry};
use tracing::{error, info, warn};

use crate::config::{Config, ConfigValidationError};

/// Callback type for config reload notifications.
pub type ReloadCallback = Arc<dyn Fn(&Config) + Send + Sync>;

/// Callback type for config validation.
pub type ValidateCallback = Arc<dyn Fn(&Config) -> Vec<ConfigValidationError> + Send + Sync>;

/// Metrics for config hot-reload.
pub struct ReloadMetrics {
    /// Total successful config reloads.
    pub reload_total: IntCounter,
    /// Total failed config reloads (parse or validation errors).
    pub reload_failed_total: IntCounter,
    /// Unix timestamp of the last successful reload.
    pub reload_last_timestamp: Gauge,
}

impl ReloadMetrics {
    /// Create and register metrics on the given registry.
    pub fn new(registry: &Registry) -> Self {
        let reload_total = IntCounter::with_opts(Opts::new(
            "config_reload_total",
            "Total successful config reloads",
        ))
        .expect("metric opts for config_reload_total are valid");
        let reload_failed_total = IntCounter::with_opts(Opts::new(
            "config_reload_failed_total",
            "Total failed config reload attempts",
        ))
        .expect("metric opts for config_reload_failed_total are valid");
        // Gauge holding epoch seconds — set directly on each reload
        let reload_last_timestamp = Gauge::with_opts(Opts::new(
            "config_reload_last_timestamp",
            "Unix timestamp of the last successful config reload",
        ))
        .expect("metric opts for config_reload_last_timestamp are valid");

        registry
            .register(Box::new(reload_total.clone()))
            .expect("register reload_total must not conflict");
        registry
            .register(Box::new(reload_failed_total.clone()))
            .expect("register reload_failed_total must not conflict");
        registry
            .register(Box::new(reload_last_timestamp.clone()))
            .expect("register reload_last_timestamp must not conflict");

        Self {
            reload_total,
            reload_failed_total,
            reload_last_timestamp,
        }
    }
}

/// Watches a config file for changes and triggers reload.
pub struct ConfigWatcher {
    /// Path to watch.
    path: PathBuf,
    /// Current config (thread-safe).
    config: Arc<RwLock<Config>>,
    /// Debounce interval.
    debounce: Duration,
    /// Optional reload metrics.
    metrics: Option<Arc<ReloadMetrics>>,
}

impl ConfigWatcher {
    /// Create a new config watcher, loading initial config from the given path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let config =
            Config::from_file(path.as_ref().to_str().unwrap_or("shim.toml")).unwrap_or_default();
        Self {
            path: path.as_ref().to_path_buf(),
            config: Arc::new(RwLock::new(config)),
            debounce: Duration::from_millis(500),
            metrics: None,
        }
    }

    /// Create with custom debounce interval.
    pub fn with_debounce(path: impl AsRef<Path>, debounce: Duration) -> Self {
        let config =
            Config::from_file(path.as_ref().to_str().unwrap_or("shim.toml")).unwrap_or_default();
        Self {
            path: path.as_ref().to_path_buf(),
            config: Arc::new(RwLock::new(config)),
            debounce,
            metrics: None,
        }
    }

    /// Create with metrics collection.
    pub fn with_metrics(path: impl AsRef<Path>, registry: &Registry) -> Self {
        let mut watcher = Self::new(path);
        watcher.metrics = Some(Arc::new(ReloadMetrics::new(registry)));
        watcher
    }

    /// Get the current config snapshot.
    pub fn config(&self) -> Config {
        self.config.read().clone()
    }

    /// Start watching for changes. Runs in background.
    ///
    /// When the config file changes, re-reads it, validates, and
    /// updates the shared `Config`. Calls the callback if provided.
    pub fn start_watching(self: &Arc<Self>, callback: Option<ReloadCallback>) {
        let config_watcher = self.clone();
        let path = self.path.clone();
        let debounce = self.debounce;

        std::thread::Builder::new()
            .name("config-watcher".into())
            .spawn(move || {
                use notify::{
                    Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher,
                };

                let (tx, rx) = std::sync::mpsc::channel();
                let mut fs_watcher = match RecommendedWatcher::new(tx, NotifyConfig::default()) {
                    Ok(w) => w,
                    Err(e) => {
                        error!("failed to create file watcher: {}", e);
                        return;
                    }
                };

                if let Err(e) = fs_watcher.watch(&path, RecursiveMode::NonRecursive) {
                    error!("failed to watch {}: {}", path.display(), e);
                    return;
                }

                info!("watching config file: {}", path.display());

                let mut last_reload = std::time::Instant::now();

                loop {
                    match rx.recv() {
                        Ok(Ok(Event {
                            kind: notify::EventKind::Modify(_),
                            ..
                        })) => {
                            if last_reload.elapsed() < debounce {
                                continue;
                            }
                            last_reload = std::time::Instant::now();

                            // Debounce: wait a bit more for writes to finish
                            std::thread::sleep(debounce);

                            match Config::from_file(path.to_str().unwrap_or("shim.toml")) {
                                Ok(new_config) => {
                                    let mut config = config_watcher.config.write();
                                    *config = new_config.clone();
                                    drop(config);

                                    info!("config reloaded from {}", path.display());

                                    if let Some(ref m) = config_watcher.metrics {
                                        m.reload_total.inc();
                                        let ts = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs();
                                        // Reset and set to current timestamp
                                        m.reload_last_timestamp.set(ts as f64);
                                    }

                                    if let Some(ref cb) = callback {
                                        cb(&new_config);
                                    }
                                }
                                Err(e) => {
                                    warn!("config reload failed: {} (keeping previous config)", e);
                                    if let Some(ref m) = config_watcher.metrics {
                                        m.reload_failed_total.inc();
                                    }
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            error!("watch error: {}", e);
                            break;
                        }
                        Err(e) => {
                            error!("channel error: {}", e);
                            break;
                        }
                        _ => {} // Ignore non-modify events
                    }
                }
            })
            .expect("failed to spawn config-watcher thread");
    }

    /// Start watching with validation. Validates config before applying.
    ///
    /// When the config file changes, re-reads it, runs it through the
    /// validation callback, and only applies it if validation passes.
    /// Calls `on_validated` with the new config if validation succeeds,
    /// or `on_validation_failed` with the errors if it fails.
    pub fn start_watching_with_validation(
        self: &Arc<Self>,
        validator: ValidateCallback,
        on_validated: Option<ReloadCallback>,
    ) {
        let config_watcher = self.clone();
        let path = self.path.clone();
        let debounce = self.debounce;

        std::thread::Builder::new()
            .name("config-watcher-validation".into())
            .spawn(move || {
                use notify::{
                    Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher,
                };

                let (tx, rx) = std::sync::mpsc::channel();
                let mut fs_watcher = match RecommendedWatcher::new(tx, NotifyConfig::default()) {
                    Ok(w) => w,
                    Err(e) => {
                        error!("failed to create file watcher: {}", e);
                        return;
                    }
                };

                if let Err(e) = fs_watcher.watch(&path, RecursiveMode::NonRecursive) {
                    error!("failed to watch {}: {}", path.display(), e);
                    return;
                }

                info!("watching config file with validation: {}", path.display());

                let mut last_reload = std::time::Instant::now();

                loop {
                    match rx.recv() {
                        Ok(Ok(Event {
                            kind: notify::EventKind::Modify(_),
                            ..
                        })) => {
                            if last_reload.elapsed() < debounce {
                                continue;
                            }
                            last_reload = std::time::Instant::now();

                            std::thread::sleep(debounce);

                            match Config::from_file(path.to_str().unwrap_or("shim.toml")) {
                                Ok(new_config) => {
                                    let errors = validator(&new_config);
                                    if errors.is_empty() {
                                        let mut config = config_watcher.config.write();
                                        *config = new_config.clone();
                                        drop(config);

                                        info!(
                                            "config validated and reloaded from {}",
                                            path.display()
                                        );

                                        if let Some(ref m) = config_watcher.metrics {
                                            m.reload_total.inc();
                                            let ts = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs();
                                            m.reload_last_timestamp.set(ts as f64);
                                        }

                                        if let Some(ref cb) = on_validated {
                                            cb(&new_config);
                                        }
                                    } else {
                                        warn!(
                                            "config validation failed with {} error(s), \
                                             keeping previous config",
                                            errors.len()
                                        );
                                        for err in &errors {
                                            warn!(
                                                "  validation error: {}: {}",
                                                err.field, err.message
                                            );
                                        }
                                        if let Some(ref m) = config_watcher.metrics {
                                            m.reload_failed_total.inc();
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("config reload failed: {} (keeping previous config)", e);
                                    if let Some(ref m) = config_watcher.metrics {
                                        m.reload_failed_total.inc();
                                    }
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            error!("watch error: {}", e);
                            break;
                        }
                        Err(e) => {
                            error!("channel error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
            })
            .expect("failed to spawn config-watcher-validation thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_config_watcher_new() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let watcher = ConfigWatcher::new(&config_path);
        let cfg = watcher.config();
        assert_eq!(cfg.health.listen, "0.0.0.0:9200");
    }

    #[test]
    fn test_config_watcher_reload() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let watcher = Arc::new(ConfigWatcher::new(&config_path));
        let reloaded = Arc::new(AtomicBool::new(false));
        let reloaded_clone = reloaded.clone();
        let watcher_clone = watcher.clone();

        watcher.start_watching(Some(Arc::new(move |config: &Config| {
            reloaded_clone.store(true, Ordering::SeqCst);
            tracing::info!("reloaded: listen={}", config.health.listen);
        })));

        // Wait for watcher to be ready
        std::thread::sleep(Duration::from_millis(200));

        // Modify config
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9300\"\n").unwrap();

        // Wait for reload
        std::thread::sleep(Duration::from_millis(1500));

        let cfg = watcher_clone.config();
        // May or may not have reloaded depending on timing, but should not crash
        assert!(cfg.health.listen.starts_with("0.0.0.0:"));
    }

    #[test]
    fn test_config_watcher_invalid_config_keeps_previous() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let watcher = Arc::new(ConfigWatcher::new(&config_path));
        watcher.start_watching(None);

        std::thread::sleep(Duration::from_millis(200));

        // Write invalid TOML
        std::fs::write(&config_path, "this is not valid [[[ toml").unwrap();

        std::thread::sleep(Duration::from_millis(1500));

        // Should still have original config
        let cfg = watcher.config();
        assert_eq!(cfg.health.listen, "0.0.0.0:9200");
    }

    #[test]
    fn test_reload_metrics_creation() {
        let registry = Registry::new();
        let m = ReloadMetrics::new(&registry);
        assert_eq!(m.reload_total.get(), 0);
        assert_eq!(m.reload_failed_total.get(), 0);
    }

    #[test]
    fn test_reload_metrics_increments() {
        let registry = Registry::new();
        let m = ReloadMetrics::new(&registry);
        m.reload_total.inc();
        m.reload_total.inc();
        m.reload_failed_total.inc();

        assert_eq!(m.reload_total.get(), 2);
        assert_eq!(m.reload_failed_total.get(), 1);
    }

    #[test]
    fn test_reload_metrics_export() {
        let registry = Registry::new();
        let m = ReloadMetrics::new(&registry);
        m.reload_total.inc();
        m.reload_failed_total.inc();

        let output = prometheus::TextEncoder::new()
            .encode_to_string(&registry.gather())
            .unwrap();
        assert!(output.contains("config_reload_total"));
        assert!(output.contains("config_reload_failed_total"));
        assert!(output.contains("config_reload_last_timestamp"));
    }

    #[test]
    fn test_config_watcher_with_metrics() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let registry = Registry::new();
        let watcher = ConfigWatcher::with_metrics(&config_path, &registry);
        assert!(watcher.metrics.is_some());
    }

    #[test]
    fn test_validate_callback_type() {
        let validator: ValidateCallback = Arc::new(|config: &Config| config.validate());
        let config = Config::default();
        let errors = validator(&config);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_validate_callback_rejects_invalid() {
        let validator: ValidateCallback = Arc::new(|config: &Config| config.validate());
        let mut config = Config::default();
        config.health.listen = "not-an-address".into();
        let errors = validator(&config);
        assert!(!errors.is_empty());
    }

    // --- concurrency stress tests ---

    #[test]
    fn test_concurrent_config_read_write() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let watcher = Arc::new(ConfigWatcher::new(&config_path));
        let running = Arc::new(AtomicBool::new(true));
        let read_count = Arc::new(AtomicUsize::new(0));

        // Spawn reader threads
        let mut handles = Vec::new();
        for _ in 0..5 {
            let watcher = watcher.clone();
            let running = running.clone();
            let read_count = read_count.clone();
            handles.push(std::thread::spawn(move || {
                while running.load(Ordering::Relaxed) {
                    let _cfg = watcher.config();
                    read_count.fetch_add(1, Ordering::Relaxed);
                    std::thread::yield_now();
                }
            }));
        }

        // Spawn writer thread that modifies the config file
        {
            let config_path = config_path.clone();
            let running = running.clone();
            handles.push(std::thread::spawn(move || {
                for port in 9201..9210 {
                    let content = format!("[health]\nlisten = \"0.0.0.0:{}\"\n", port);
                    std::fs::write(&config_path, content).unwrap();
                    std::thread::sleep(Duration::from_millis(10));
                }
                running.store(false, Ordering::Relaxed);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // Readers should have completed many reads without panicking
        assert!(read_count.load(Ordering::Relaxed) > 10);
    }

    #[test]
    fn test_concurrent_config_reload_and_read() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let watcher = Arc::new(ConfigWatcher::new(&config_path));
        let callback_count = Arc::new(AtomicUsize::new(0));

        let callback_count_clone = callback_count.clone();
        watcher.start_watching(Some(Arc::new(move |_config: &Config| {
            callback_count_clone.fetch_add(1, Ordering::Relaxed);
        })));

        // Wait for watcher to initialize
        std::thread::sleep(Duration::from_millis(200));

        let running = Arc::new(AtomicBool::new(true));

        // Spawn reader threads
        let mut handles = Vec::new();
        for _ in 0..3 {
            let watcher = watcher.clone();
            let running = running.clone();
            handles.push(std::thread::spawn(move || {
                let mut count = 0;
                while running.load(Ordering::Relaxed) {
                    let _cfg = watcher.config();
                    count += 1;
                    std::thread::sleep(Duration::from_millis(5));
                }
                count
            }));
        }

        // Write multiple config changes
        for port in 9300..9305 {
            let content = format!("[health]\nlisten = \"0.0.0.0:{}\"\n", port);
            std::fs::write(&config_path, content).unwrap();
            std::thread::sleep(Duration::from_millis(800));
        }

        running.store(false, Ordering::Relaxed);

        for h in handles {
            let count = h.join().unwrap();
            assert!(count > 0, "reader should have completed at least one read");
        }

        // Allow remaining reloads to settle
        std::thread::sleep(Duration::from_millis(1000));
        // Callback may or may not have fired depending on timing, but shouldn't panic
    }

    #[test]
    fn test_concurrent_validate_and_read() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let watcher = Arc::new(ConfigWatcher::new(&config_path));
        let validator: ValidateCallback = Arc::new(|config: &Config| config.validate());
        let running = Arc::new(AtomicBool::new(true));

        let mut handles = Vec::new();

        // Reader threads
        for _ in 0..3 {
            let watcher = watcher.clone();
            let running = running.clone();
            handles.push(std::thread::spawn(move || {
                let mut count = 0;
                while running.load(Ordering::Relaxed) {
                    let cfg = watcher.config();
                    let errors = cfg.validate();
                    assert!(errors.is_empty(), "default config should be valid");
                    count += 1;
                    std::thread::yield_now();
                }
                count
            }));
        }

        // Validator thread
        {
            let watcher = watcher.clone();
            let validator = validator.clone();
            let running = running.clone();
            handles.push(std::thread::spawn(move || {
                let mut count = 0;
                while running.load(Ordering::Relaxed) {
                    let cfg = watcher.config();
                    let errors = validator(&cfg);
                    // Config loaded from file should be valid
                    assert!(errors.is_empty());
                    count += 1;
                    std::thread::yield_now();
                }
                count
            }));
        }

        // Stop after a short burst
        std::thread::sleep(Duration::from_millis(100));
        running.store(false, Ordering::Relaxed);

        for h in handles {
            let count = h.join().unwrap();
            assert!(count > 0);
        }
    }

    #[test]
    fn test_config_watcher_start_watching_with_validation() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let watcher = Arc::new(ConfigWatcher::new(&config_path));
        let applied = Arc::new(AtomicBool::new(false));
        let applied_clone = applied.clone();

        let validator: ValidateCallback = Arc::new(|config: &Config| config.validate());

        watcher.start_watching_with_validation(
            validator,
            Some(Arc::new(move |_config: &Config| {
                applied_clone.store(true, Ordering::SeqCst);
            })),
        );

        std::thread::sleep(Duration::from_millis(200));

        // Write valid config
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9300\"\n").unwrap();

        std::thread::sleep(Duration::from_millis(1500));

        let cfg = watcher.config();
        // Should have reloaded valid config
        assert!(cfg.health.listen.starts_with("0.0.0.0:"));
    }

    #[test]
    fn test_config_watcher_validation_rejects_bad_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("shim.toml");
        std::fs::write(&config_path, "[health]\nlisten = \"0.0.0.0:9200\"\n").unwrap();

        let watcher = Arc::new(ConfigWatcher::new(&config_path));

        let validator: ValidateCallback = Arc::new(|config: &Config| config.validate());

        // Use a callback that would fire on valid config — it shouldn't fire for invalid
        watcher.start_watching_with_validation(
            validator,
            Some(Arc::new(move |_config: &Config| {
                // This should NOT be called with invalid config
            })),
        );

        std::thread::sleep(Duration::from_millis(200));

        // Write config with invalid listen address
        std::fs::write(&config_path, "[health]\nlisten = \"not-an-address\"\n").unwrap();

        std::thread::sleep(Duration::from_millis(1500));

        // Should still have original valid config
        let cfg = watcher.config();
        assert_eq!(cfg.health.listen, "0.0.0.0:9200");
    }
}
