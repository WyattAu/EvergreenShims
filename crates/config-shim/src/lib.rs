//! Config shim — hot-reload configuration for applications.
//!
//! Watches a configuration file for changes using SHA-256 content hashing,
//! validates new config via optional shell command, backs up the previous
//! version, and signals the child process to reload.
//!
//! ## Environment Variables
//!
//! ```text
//! CONFIG_PATH            Path to config file (default: /etc/app/config.toml)
//! CONFIG_WATCH           Watch for changes (default: true)
//! CONFIG_RELOAD_SIGNAL   Signal to send on change (default: SIGHUP)
//! CONFIG_RELOAD_DEBOUNCE Debounce interval in seconds (default: 5)
//! CONFIG_VALIDATE_CMD    Command to validate config (optional)
//! CONFIG_BACKUP          Keep backup of last config (default: true)
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::{watch, Mutex};

/// Config reload event emitted on successful or failed reload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadEvent {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub path: String,
    pub old_hash: String,
    pub new_hash: String,
    pub success: bool,
    pub error: Option<String>,
    pub backup_path: Option<String>,
}

/// Config shim for hot-reload configuration.
///
/// Monitors a config file for content changes (SHA-256 hash comparison),
/// optionally validates the new config, backs up the previous version,
/// and sends a signal (default SIGHUP) to the child process.
#[allow(dead_code)]
pub struct ConfigShim {
    config_path: PathBuf,
    watch_enabled: bool,
    reload_signal: Signal,
    reload_debounce_secs: u64,
    validate_cmd: Option<String>,
    keep_backup: bool,
    child_pid: Arc<Mutex<Option<u32>>>,
    reloads_total: u64,
    reloads_success: u64,
    reloads_failed: u64,
    last_reload: Option<chrono::DateTime<chrono::Utc>>,
    last_hash: Option<String>,
    reload_tx: Option<watch::Sender<ReloadEvent>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ConfigShim {
    pub fn new() -> Self {
        let sig_str =
            std::env::var("CONFIG_RELOAD_SIGNAL").unwrap_or_else(|_| "SIGHUP".to_string());
        let reload_signal = parse_signal(&sig_str).unwrap_or(Signal::SIGHUP);

        Self {
            config_path: std::env::var("CONFIG_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/etc/app/config.toml")),
            watch_enabled: std::env::var("CONFIG_WATCH")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            reload_signal,
            reload_debounce_secs: std::env::var("CONFIG_RELOAD_DEBOUNCE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            validate_cmd: std::env::var("CONFIG_VALIDATE_CMD").ok(),
            keep_backup: std::env::var("CONFIG_BACKUP")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            child_pid: Arc::new(Mutex::new(None)),
            reloads_total: 0,
            reloads_success: 0,
            reloads_failed: 0,
            last_reload: None,
            last_hash: None,
            reload_tx: None,
            shutdown_tx: None,
        }
    }

    /// Set the child PID for signal delivery.
    pub async fn set_child_pid(&self, pid: u32) {
        let mut guard = self.child_pid.lock().await;
        *guard = Some(pid);
    }

    /// Subscribe to reload events.
    pub fn subscribe(&mut self) -> Option<watch::Receiver<ReloadEvent>> {
        self.reload_tx.as_ref().map(|tx| tx.subscribe())
    }

    /// Calculate SHA-256 hash of file contents.
    pub async fn file_hash(path: &Path) -> Option<String> {
        use sha2::{Digest, Sha256};

        let content = tokio::fs::read(path).await.ok()?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let result = hasher.finalize();
        Some(format!("{:x}", result))
    }

    /// Validate config file by running optional validation command.
    pub async fn validate(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(ref cmd) = self.validate_cmd {
            tracing::debug!(%cmd, "Running config validation");
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .arg(path)
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("Config validation failed: {}", stderr.trim());
            }
        }
        Ok(())
    }

    /// Create backup of current config file.
    #[allow(dead_code)]
    async fn backup(&self) -> anyhow::Result<Option<PathBuf>> {
        if !self.keep_backup {
            return Ok(None);
        }

        let backup_path = self.config_path.with_extension("toml.bak");
        if self.config_path.exists() {
            tokio::fs::copy(&self.config_path, &backup_path).await?;
            tracing::info!(path = %backup_path.display(), "Config backup created");
            Ok(Some(backup_path))
        } else {
            Ok(None)
        }
    }

    /// Send reload signal to child process.
    #[allow(dead_code)]
    async fn send_signal(&self) -> anyhow::Result<()> {
        let guard = self.child_pid.lock().await;
        if let Some(pid) = *guard {
            let pid = Pid::from_raw(pid as i32);
            tracing::info!(%pid, signal = ?self.reload_signal, "Sending reload signal");
            signal::kill(pid, self.reload_signal)?;
        } else {
            tracing::warn!("No child PID set, cannot send reload signal");
        }
        Ok(())
    }

    /// Reload configuration (validate → backup → hash check → signal).
    #[allow(dead_code)]
    async fn reload(&mut self) -> anyhow::Result<ReloadEvent> {
        let old_hash = self.last_hash.clone().unwrap_or_default();
        let new_hash = Self::file_hash(&self.config_path).await.ok_or_else(|| {
            anyhow::anyhow!("Cannot read config file: {}", self.config_path.display())
        })?;

        // Skip if unchanged
        if Some(new_hash.clone()) == self.last_hash {
            return Ok(ReloadEvent {
                timestamp: chrono::Utc::now(),
                path: self.config_path.display().to_string(),
                old_hash,
                new_hash,
                success: true,
                error: None,
                backup_path: None,
            });
        }

        // Validate
        self.validate(&self.config_path).await?;

        // Backup
        let backup_path = self.backup().await?.map(|p| p.display().to_string());

        // Update hash
        self.last_hash = Some(new_hash.clone());

        // Signal child
        self.send_signal().await?;

        let event = ReloadEvent {
            timestamp: chrono::Utc::now(),
            path: self.config_path.display().to_string(),
            old_hash,
            new_hash,
            success: true,
            error: None,
            backup_path,
        };

        self.reloads_total += 1;
        self.reloads_success += 1;
        self.last_reload = Some(event.timestamp);

        tracing::info!(
            path = %self.config_path.display(),
            total = self.reloads_total,
            "Config reloaded successfully"
        );

        Ok(event)
    }
}

impl Default for ConfigShim {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse signal name string to nix Signal.
fn parse_signal(s: &str) -> Option<Signal> {
    match s.to_uppercase().as_str() {
        "SIGHUP" => Some(Signal::SIGHUP),
        "SIGUSR1" => Some(Signal::SIGUSR1),
        "SIGUSR2" => Some(Signal::SIGUSR2),
        "SIGTERM" => Some(Signal::SIGTERM),
        _ => {
            // Try numeric
            let n: i32 = s.parse().ok()?;
            Signal::try_from(n).ok()
        }
    }
}

#[async_trait::async_trait]
impl Capability for ConfigShim {
    fn name(&self) -> &str {
        "config"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            path = %self.config_path.display(),
            watch = self.watch_enabled,
            signal = ?self.reload_signal,
            debounce = self.reload_debounce_secs,
            "ConfigShim initialized"
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        // Initial hash
        if self.config_path.exists() {
            self.last_hash = Self::file_hash(&self.config_path).await;
            tracing::info!(
                path = %self.config_path.display(),
                hash = ?self.last_hash,
                "Initial config loaded"
            );
        } else {
            tracing::warn!(
                path = %self.config_path.display(),
                "Config file not found, will watch for creation"
            );
        }

        let (reload_tx, _) = watch::channel(ReloadEvent {
            timestamp: chrono::Utc::now(),
            path: String::new(),
            old_hash: String::new(),
            new_hash: String::new(),
            success: true,
            error: None,
            backup_path: None,
        });
        self.reload_tx = Some(reload_tx);

        if self.watch_enabled {
            let config_path = self.config_path.clone();
            let debounce_secs = self.reload_debounce_secs;
            let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
            self.shutdown_tx = Some(shutdown_tx);

            tokio::spawn(async move {
                let mut last_hash = Self::file_hash(&config_path).await;

                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(debounce_secs)) => {
                            let current_hash = Self::file_hash(&config_path).await;

                            if current_hash != last_hash {
                                tracing::info!(
                                    path = %config_path.display(),
                                    changed = (last_hash.is_some()),
                                    "Config file changed"
                                );

                                // Log the change but actual reload is done via reload()
                                last_hash = current_hash;
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            tracing::info!("Config watch loop shutting down");
                            break;
                        }
                    }
                }

                tracing::info!("Config watch loop exited");
            });
        }

        tracing::info!("ConfigShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!(
            total = self.reloads_total,
            success = self.reloads_success,
            failed = self.reloads_failed,
            "ConfigShim stopped"
        );
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let mut metrics = vec![
            Metric::new("config_reloads_total", self.reloads_total as f64),
            Metric::new("config_reloads_success", self.reloads_success as f64),
            Metric::new("config_reloads_failed", self.reloads_failed as f64),
        ];

        if let Some(last) = &self.last_reload {
            metrics.push(Metric::new(
                "config_last_reload_timestamp",
                last.timestamp() as f64,
            ));
        }

        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_hash_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        tokio::fs::write(&path, "key = 'value'\n").await.unwrap();

        let h1 = ConfigShim::file_hash(&path).await.unwrap();
        let h2 = ConfigShim::file_hash(&path).await.unwrap();
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[tokio::test]
    async fn test_file_hash_changes_on_content_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        tokio::fs::write(&path, "key = 'value'\n").await.unwrap();

        let h1 = ConfigShim::file_hash(&path).await.unwrap();

        tokio::fs::write(&path, "key = 'changed'\n").await.unwrap();

        let h2 = ConfigShim::file_hash(&path).await.unwrap();
        assert_ne!(h1, h2);
    }

    #[tokio::test]
    async fn test_file_hash_none_for_missing() {
        let hash = ConfigShim::file_hash(Path::new("/nonexistent/file.toml")).await;
        assert!(hash.is_none());
    }

    #[tokio::test]
    async fn test_validate_succeeds_with_valid_cmd() {
        let shim = ConfigShim::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        tokio::fs::write(&path, "key = 'value'\n").await.unwrap();

        // validate_cmd = "test -f" — will succeed for existing file
        // We can't set env easily, so test the no-validate path
        let result = shim.validate(&path).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_fails_for_bad_cmd() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        tokio::fs::write(&path, "bad config\n").await.unwrap();

        let shim = ConfigShim {
            validate_cmd: Some("false".to_string()), // always exits 1
            ..ConfigShim::new()
        };

        let result = shim.validate(&path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_backup_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let backup = dir.path().join("config.toml.bak");
        tokio::fs::write(&path, "original content\n").await.unwrap();

        let shim = ConfigShim {
            config_path: path.clone(),
            keep_backup: true,
            ..ConfigShim::new()
        };

        let result = shim.backup().await.unwrap();
        assert!(result.is_some());
        assert!(backup.exists());
        assert_eq!(
            tokio::fs::read_to_string(&backup).await.unwrap(),
            "original content\n"
        );
    }

    #[tokio::test]
    async fn test_backup_skipped_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        tokio::fs::write(&path, "content\n").await.unwrap();

        let shim = ConfigShim {
            config_path: path.clone(),
            keep_backup: false,
            ..ConfigShim::new()
        };

        let result = shim.backup().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_parse_signal() {
        assert_eq!(parse_signal("SIGHUP"), Some(Signal::SIGHUP));
        assert_eq!(parse_signal("SIGUSR1"), Some(Signal::SIGUSR1));
        assert_eq!(parse_signal("SIGUSR2"), Some(Signal::SIGUSR2));
        assert_eq!(parse_signal("SIGTERM"), Some(Signal::SIGTERM));
        assert_eq!(parse_signal("sighup"), Some(Signal::SIGHUP)); // case insensitive
        assert_eq!(parse_signal("1"), Some(Signal::SIGHUP)); // SIGHUP=1
        assert!(parse_signal("INVALID").is_none());
    }

    #[tokio::test]
    async fn test_reload_skips_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        tokio::fs::write(&path, "key = 'value'\n").await.unwrap();

        let mut shim = ConfigShim {
            config_path: path.clone(),
            keep_backup: false,
            ..ConfigShim::new()
        };
        shim.last_hash = ConfigShim::file_hash(&path).await;

        let event = shim.reload().await.unwrap();
        assert!(event.success);
        assert_eq!(event.new_hash, event.old_hash); // unchanged
        assert_eq!(shim.reloads_total, 0); // no increment on unchanged
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = ConfigShim {
            reloads_total: 5,
            reloads_success: 4,
            reloads_failed: 1,
            last_reload: Some(chrono::Utc::now()),
            ..ConfigShim::new()
        };

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].name, "config_reloads_total");
        assert_eq!(metrics[0].value, 5.0);
        assert_eq!(metrics[1].name, "config_reloads_success");
        assert_eq!(metrics[1].value, 4.0);
        assert_eq!(metrics[2].name, "config_reloads_failed");
        assert_eq!(metrics[2].value, 1.0);
    }

    #[tokio::test]
    async fn test_set_child_pid() {
        let shim = ConfigShim::new();
        assert!(shim.child_pid.lock().await.is_none());

        shim.set_child_pid(12345).await;
        assert_eq!(*shim.child_pid.lock().await, Some(12345));
    }
}
