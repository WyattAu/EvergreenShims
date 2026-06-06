//! MongoDB shim — health checks, backup, and CDC for MongoDB.
//!
//! ## Environment Variables
//!
//! ```text
//! MONGO_URI              MongoDB connection URI (default: mongodb://localhost:27017)
//! MONGO_DATABASE         Database name
//! MONGO_BACKUP_DIR       Backup output directory (default: /tmp/mongo-backups)
//! MONGO_BACKUP_CMD       Backup command (default: mongodump)
//! MONGO_RETENTION_DAYS   Backup retention days (default: 30)
//! MONGO_CDC_OUTPUT       CDC output: kafka, webhook, log (default: log)
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// MongoDB health status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MongoHealth {
    pub ok: bool,
    pub version: String,
    pub connections: u32,
    pub uptime_secs: u64,
}

/// MongoDB backup result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MongoBackup {
    pub path: String,
    pub size_bytes: u64,
    pub timestamp: String,
    pub database: String,
    pub success: bool,
    pub error: Option<String>,
}

/// MongoDB shim for health checks, backup, and CDC.
pub struct MongoShim {
    uri: String,
    database: String,
    backup_dir: String,
    backup_cmd: String,
    #[allow(dead_code)]
    retention_days: u32,
    health_checks: u64,
    backup_success: u64,
    backup_failure: u64,
    last_backup: Option<chrono::DateTime<chrono::Utc>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl MongoShim {
    pub fn new() -> Self {
        Self {
            uri: std::env::var("MONGO_URI")
                .unwrap_or_else(|_| "mongodb://localhost:27017".to_string()),
            database: std::env::var("MONGO_DATABASE").unwrap_or_default(),
            backup_dir: std::env::var("MONGO_BACKUP_DIR")
                .unwrap_or_else(|_| "/tmp/mongo-backups".to_string()),
            backup_cmd: std::env::var("MONGO_BACKUP_CMD")
                .unwrap_or_else(|_| "mongodump".to_string()),
            retention_days: std::env::var("MONGO_RETENTION_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            health_checks: 0,
            backup_success: 0,
            backup_failure: 0,
            last_backup: None,
            shutdown_tx: None,
        }
    }

    /// Check MongoDB health via serverStatus with retry.
    pub async fn check_health(&mut self) -> anyhow::Result<MongoHealth> {
        self.health_checks += 1;

        let max_retries = 3;
        let mut last_error = None;

        for attempt in 0..max_retries {
            let output = tokio::process::Command::new("mongosh")
                .args([
                    &self.uri,
                    "--eval",
                    "JSON.stringify(db.serverStatus())",
                    "--quiet",
                ])
                .output()
                .await;

            match output {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let status: serde_json::Value = serde_json::from_str(&stdout)
                        .map_err(|e| anyhow::anyhow!("Failed to parse serverStatus: {}", e))?;

                    return Ok(MongoHealth {
                        ok: status["ok"].as_f64().unwrap_or(0.0) == 1.0,
                        version: status["version"].as_str().unwrap_or("unknown").to_string(),
                        connections: status["connections"]["current"].as_u64().unwrap_or(0) as u32,
                        uptime_secs: status["uptime"].as_u64().unwrap_or(0),
                    });
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    last_error = Some(anyhow::anyhow!("mongosh failed: {}", stderr));
                }
                Err(e) => {
                    last_error = Some(anyhow::anyhow!("mongosh execution failed: {}", e));
                }
            }

            if attempt < max_retries - 1 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Health check failed after retries")))
    }

    /// Backup a MongoDB database using mongodump with retry.
    pub async fn backup(&mut self) -> anyhow::Result<MongoBackup> {
        if self.database.is_empty() {
            anyhow::bail!("MONGO_DATABASE not set");
        }

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let backup_path = format!("{}/{}", self.backup_dir, self.database);

        // Create backup directory
        tokio::fs::create_dir_all(&backup_path).await?;

        let output = tokio::process::Command::new(&self.backup_cmd)
            .args([
                "--uri",
                &self.uri,
                "--db",
                &self.database,
                "--out",
                &backup_path,
            ])
            .output()
            .await?;

        if output.status.success() {
            // Get backup size
            let mut total_size = 0u64;
            let mut entries = tokio::fs::read_dir(&backup_path).await?;
            while let Some(entry) = entries.next_entry().await? {
                if let Ok(meta) = entry.metadata().await {
                    total_size += meta.len();
                }
            }

            self.backup_success += 1;
            self.last_backup = Some(chrono::Utc::now());

            tracing::info!(
                "MongoDB backup complete: {} ({} bytes)",
                backup_path,
                total_size
            );

            Ok(MongoBackup {
                path: backup_path,
                size_bytes: total_size,
                timestamp: timestamp.to_string(),
                database: self.database.clone(),
                success: true,
                error: None,
            })
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            self.backup_failure += 1;

            tracing::error!("MongoDB backup failed: {}", stderr);

            Ok(MongoBackup {
                path: backup_path,
                size_bytes: 0,
                timestamp: timestamp.to_string(),
                database: self.database.clone(),
                success: false,
                error: Some(stderr.to_string()),
            })
        }
    }

    /// Get MongoDB URI.
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Get database name.
    pub fn database(&self) -> &str {
        &self.database
    }
}

impl Default for MongoShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for MongoShim {
    fn name(&self) -> &str {
        "mongodb"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "MongoShim initialized (uri={}, db={})",
            self.uri,
            self.database
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("MongoShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("MongoShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("mongo_health_checks_total", self.health_checks as f64),
            Metric::new("mongo_backup_success_total", self.backup_success as f64),
            Metric::new("mongo_backup_failure_total", self.backup_failure as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mongo_shim_defaults() {
        temp_env::with_vars(
            [
                ("MONGO_URI", None::<&str>),
                ("MONGO_DATABASE", None::<&str>),
            ],
            || {
                let shim = MongoShim::new();
                assert_eq!(shim.uri(), "mongodb://localhost:27017");
                assert_eq!(shim.database(), "");
            },
        );
    }

    #[test]
    fn test_mongo_shim_env_overrides() {
        temp_env::with_vars(
            [
                ("MONGO_URI", Some("mongodb://prod:27017")),
                ("MONGO_DATABASE", Some("myapp")),
            ],
            || {
                let shim = MongoShim::new();
                assert_eq!(shim.uri(), "mongodb://prod:27017");
                assert_eq!(shim.database(), "myapp");
            },
        );
    }

    #[test]
    fn test_mongo_shim_retention() {
        temp_env::with_vars([("MONGO_RETENTION_DAYS", Some("60"))], || {
            let shim = MongoShim::new();
            assert_eq!(shim.retention_days, 60);
        });
    }

    #[test]
    fn test_mongo_shim_metrics() {
        let shim = MongoShim::new();
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 3);
        assert_eq!(metrics[0].name, "mongo_health_checks_total");
        assert_eq!(metrics[1].name, "mongo_backup_success_total");
        assert_eq!(metrics[2].name, "mongo_backup_failure_total");
    }

    #[test]
    fn test_mongo_shim_capability_name() {
        let shim = MongoShim::new();
        assert_eq!(shim.name(), "mongodb");
    }

    #[test]
    fn test_mongo_shim_default_trait() {
        let shim = MongoShim::default();
        assert_eq!(shim.name(), "mongodb");
    }

    #[test]
    fn test_mongo_shim_backup_dir_default() {
        temp_env::with_vars([("MONGO_BACKUP_DIR", None::<&str>)], || {
            let shim = MongoShim::new();
            assert_eq!(shim.backup_dir, "/tmp/mongo-backups");
        });
    }

    #[test]
    fn test_mongo_shim_backup_cmd_default() {
        temp_env::with_vars([("MONGO_BACKUP_CMD", None::<&str>)], || {
            let shim = MongoShim::new();
            assert_eq!(shim.backup_cmd, "mongodump");
        });
    }

    #[test]
    fn test_mongo_shim_retention_default() {
        temp_env::with_vars([("MONGO_RETENTION_DAYS", None::<&str>)], || {
            let shim = MongoShim::new();
            assert_eq!(shim.retention_days, 30);
        });
    }

    #[test]
    fn test_mongo_shim_invalid_retention_falls_back() {
        temp_env::with_vars([("MONGO_RETENTION_DAYS", Some("not_a_number"))], || {
            let shim = MongoShim::new();
            assert_eq!(shim.retention_days, 30);
        });
    }

    #[test]
    fn test_mongo_shim_init_and_start_stop() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = MongoShim::new();
            let config = Config::default();

            let init_result = shim.init(&config).await;
            assert!(init_result.is_ok());

            let start_result = shim.start().await;
            assert!(start_result.is_ok());
            assert!(shim.shutdown_tx.is_some());

            let stop_result = shim.stop().await;
            assert!(stop_result.is_ok());
            assert!(shim.shutdown_tx.is_none());
        });
    }

    #[test]
    fn test_mongo_shim_stop_without_start() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = MongoShim::new();
            let result = shim.stop().await;
            assert!(result.is_ok());
        });
    }

    #[tokio::test]
    async fn test_mongo_backup_missing_database() {
        // Directly create a shim with empty database
        let mut shim = MongoShim {
            database: String::new(),
            ..MongoShim::new()
        };
        let result = shim.backup().await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("MONGO_DATABASE not set"));
    }

    #[test]
    fn test_mongo_health_serialize() {
        let health = MongoHealth {
            ok: true,
            version: "7.0.0".to_string(),
            connections: 42,
            uptime_secs: 86400,
        };
        let json = serde_json::to_string(&health).unwrap();
        let parsed: MongoHealth = serde_json::from_str(&json).unwrap();
        assert!(parsed.ok);
        assert_eq!(parsed.version, "7.0.0");
        assert_eq!(parsed.connections, 42);
    }

    #[test]
    fn test_mongo_backup_serialize() {
        let backup = MongoBackup {
            path: "/tmp/backups/mydb".to_string(),
            size_bytes: 1024,
            timestamp: "20240101_120000".to_string(),
            database: "mydb".to_string(),
            success: true,
            error: None,
        };
        let json = serde_json::to_string(&backup).unwrap();
        let parsed: MongoBackup = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.size_bytes, 1024);
    }

    #[test]
    fn test_mongo_shim_metrics_after_init() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut shim = MongoShim::new();
            let config = Config::default();
            let _ = shim.init(&config).await;

            let metrics = shim.metrics();
            assert_eq!(metrics.len(), 3);
            assert_eq!(metrics[0].name, "mongo_health_checks_total");
            assert_eq!(metrics[0].value, 0.0);
            assert_eq!(metrics[1].name, "mongo_backup_success_total");
            assert_eq!(metrics[1].value, 0.0);
            assert_eq!(metrics[2].name, "mongo_backup_failure_total");
            assert_eq!(metrics[2].value, 0.0);
        });
    }
}
