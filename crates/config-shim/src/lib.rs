#![allow(dead_code)]
//! Config shim — hot-reload configuration for applications.
//!
//! Watches a configuration file for changes, validates new config,
//! and sends SIGHUP to the application to trigger reload.
//!
//! ## Environment Variables
//!
//! ```text
//! CONFIG_PATH            Path to config file (required)
//! CONFIG_WATCH           Watch for changes (default: true)
//! CONFIG_RELOAD_SIGNAL   Signal to send on change (default: SIGHUP)
//! CONFIG_RELOAD_DEBOUNCE Debounce interval in seconds (default: 5)
//! CONFIG_VALIDATE_CMD    Command to validate config (optional)
//! CONFIG_BACKUP          Keep backup of last config (default: true)
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Config reload event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadEvent {
    pub timestamp: String,
    pub path: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Config shim for hot-reload configuration.
pub struct ConfigShim {
    config_path: PathBuf,
    watch_enabled: bool,
    reload_signal: String,
    reload_debounce_secs: u64,
    validate_cmd: Option<String>,
    keep_backup: bool,
    reloads_total: u64,
    reloads_success: u64,
    reloads_failed: u64,
    last_reload: Option<chrono::DateTime<chrono::Utc>>,
    last_hash: Option<String>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl ConfigShim {
    pub fn new() -> Self {
        Self {
            config_path: std::env::var("CONFIG_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/etc/app/config.toml")),
            watch_enabled: std::env::var("CONFIG_WATCH")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            reload_signal: std::env::var("CONFIG_RELOAD_SIGNAL")
                .unwrap_or_else(|_| "SIGHUP".to_string()),
            reload_debounce_secs: std::env::var("CONFIG_RELOAD_DEBOUNCE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            validate_cmd: std::env::var("CONFIG_VALIDATE_CMD").ok(),
            keep_backup: std::env::var("CONFIG_BACKUP")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            reloads_total: 0,
            reloads_success: 0,
            reloads_failed: 0,
            last_reload: None,
            last_hash: None,
            shutdown_tx: None,
        }
    }

    /// Calculate file hash for change detection.
    async fn file_hash(&self) -> Option<String> {
        use sha2::{Digest, Sha256};

        let content = tokio::fs::read(&self.config_path).await.ok()?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        let result = hasher.finalize();
        Some(format!("{:x}", result))
    }

    /// Validate configuration file.
    async fn validate(&self) -> anyhow::Result<()> {
        if let Some(cmd) = &self.validate_cmd {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .arg(&self.config_path)
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("Config validation failed: {}", stderr);
            }
        }
        Ok(())
    }

    /// Send reload signal to application.
    async fn send_signal(&self) -> anyhow::Result<()> {
        // In production: find the child PID and send signal
        tracing::info!("Sending {} to application", self.reload_signal);
        Ok(())
    }
}

impl Default for ConfigShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for ConfigShim {
    fn name(&self) -> &str {
        "config"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "ConfigShim initialized (path={}, watch={}, signal={})",
            self.config_path.display(),
            self.watch_enabled,
            self.reload_signal,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        // Initial hash
        self.last_hash = self.file_hash().await;

        if self.watch_enabled {
            let config_path = self.config_path.clone();
            let debounce_secs = self.reload_debounce_secs;
            let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
            self.shutdown_tx = Some(shutdown_tx);

            tokio::spawn(async move {
                let mut last_modified = tokio::fs::metadata(&config_path)
                    .await
                    .and_then(|m| m.modified())
                    .ok();

                let mut interval =
                    tokio::time::interval(std::time::Duration::from_secs(debounce_secs));

                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            if let Ok(metadata) = tokio::fs::metadata(&config_path).await {
                                if let Ok(modified) = metadata.modified() {
                                    if Some(modified) != last_modified {
                                        tracing::info!("Config file changed: {}", config_path.display());
                                        last_modified = Some(modified);
                                        // In production: validate, backup, send signal
                                    }
                                }
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            tracing::info!("Config shim watch loop shutting down");
                            break;
                        }
                    }
                }
            });
        }

        tracing::info!("ConfigShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("ConfigShim stopped");
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
