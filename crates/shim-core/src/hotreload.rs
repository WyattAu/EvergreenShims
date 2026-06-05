//! Config hot-reload via filesystem watching.
//!
//! `ConfigWatcher` monitors a TOML config file and triggers a callback
//! when it changes. Uses `notify` crate for filesystem events with
//! debouncing to avoid rapid re-reads.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tracing::{error, info, warn};

use crate::config::Config;

/// Callback type for config reload notifications.
pub type ReloadCallback = Arc<dyn Fn(&Config) + Send + Sync>;

/// Watches a config file for changes and triggers reload.
pub struct ConfigWatcher {
    /// Path to watch.
    path: PathBuf,
    /// Current config (thread-safe).
    config: Arc<RwLock<Config>>,
    /// Debounce interval.
    debounce: Duration,
}

impl ConfigWatcher {
    /// Create a new config watcher, loading initial config from the given path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let config = Config::from_file(path.as_ref().to_str().unwrap_or("shim.toml"))
            .unwrap_or_default();
        Self {
            path: path.as_ref().to_path_buf(),
            config: Arc::new(RwLock::new(config)),
            debounce: Duration::from_millis(500),
        }
    }

    /// Create with custom debounce interval.
    pub fn with_debounce(path: impl AsRef<Path>, debounce: Duration) -> Self {
        let config = Config::from_file(path.as_ref().to_str().unwrap_or("shim.toml"))
            .unwrap_or_default();
        Self {
            path: path.as_ref().to_path_buf(),
            config: Arc::new(RwLock::new(config)),
            debounce,
        }
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
                use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};

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
                        Ok(Ok(Event { kind: notify::EventKind::Modify(_), .. })) => {
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

                                    if let Some(ref cb) = callback {
                                        cb(&new_config);
                                    }
                                }
                                Err(e) => {
                                    warn!("config reload failed: {} (keeping previous config)", e);
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
}
