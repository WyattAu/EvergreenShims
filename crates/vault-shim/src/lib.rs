//! Vault shim — secrets rotation from HashiCorp Vault or cloud KMS.
//!
//! Reads database credentials from Vault, writes them to a file
//! (e.g., `.pgpass`, `MYSQL_PWD` file), and rotates on a schedule.
//!
//! ## Environment Variables
//!
//! ```text
//! VAULT_ADDR          Vault server URL (default: https://127.0.0.1:8200)
//! VAULT_TOKEN         Vault token (or use AppRole/K8s auth)
//! VAULT_ROLE          Vault role for dynamic credentials
//! VAULT_SECRET        Secret path (e.g., secret/data/postgres/creds)
//! VAULT_KEY           Key within secret (default: password)
//! VAULT_OUTPUT_FILE   File to write rotated credentials
//! VAULT_ROTATION_SECS Rotation interval in seconds (default: 3600)
//! VAULT_MOUNT         Vault mount point (default: secret)
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::fs;
use tokio::sync::watch;

/// Vault response for reading a secret.
#[derive(Debug, Deserialize)]
struct VaultResponse {
    data: VaultSecretData,
}

#[derive(Debug, Deserialize)]
struct VaultSecretData {
    data: HashMap<String, serde_json::Value>,
}

/// Credentials read from Vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// Username for the database.
    pub username: String,
    /// Password for the database.
    pub password: String,
    /// When these credentials were fetched.
    pub fetched_at: String,
    /// When these credentials expire (if lease-based).
    pub expires_at: Option<String>,
}

/// Vault shim for automatic secrets rotation.
pub struct VaultShim {
    vault_addr: String,
    vault_token: String,
    vault_role: String,
    vault_secret: String,
    vault_key: String,
    vault_mount: String,
    output_file: Option<PathBuf>,
    rotation_secs: u64,
    http_client: Client,
    last_rotation: Option<chrono::DateTime<chrono::Utc>>,
    rotation_success: u64,
    rotation_failure: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl VaultShim {
    /// Create a new vault shim.
    pub fn new() -> Self {
        Self {
            vault_addr: std::env::var("VAULT_ADDR")
                .unwrap_or_else(|_| "https://127.0.0.1:8200".to_string()),
            vault_token: std::env::var("VAULT_TOKEN")
                .unwrap_or_default(),
            vault_role: std::env::var("VAULT_ROLE")
                .unwrap_or_default(),
            vault_secret: std::env::var("VAULT_SECRET")
                .unwrap_or_default(),
            vault_key: std::env::var("VAULT_KEY")
                .unwrap_or_else(|_| "password".to_string()),
            vault_mount: std::env::var("VAULT_MOUNT")
                .unwrap_or_else(|_| "secret".to_string()),
            output_file: std::env::var("VAULT_OUTPUT_FILE")
                .ok()
                .map(PathBuf::from),
            rotation_secs: std::env::var("VAULT_ROTATION_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3600),
            http_client: Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap_or_default(),
            last_rotation: None,
            rotation_success: 0,
            rotation_failure: 0,
            shutdown_tx: None,
        }
    }

    /// Read a secret from Vault.
    pub async fn read_secret(&self, path: &str) -> anyhow::Result<Credentials> {
        let url = format!(
            "{}/v1/{}/{}",
            self.vault_addr, self.vault_mount, path
        );

        let response = self
            .http_client
            .get(&url)
            .header("X-Vault-Token", &self.vault_token)
            .send()
            .await?
            .error_for_status()?
            .json::<VaultResponse>()
            .await?;

        let username = response
            .data
            .data
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("postgres")
            .to_string();

        let password = response
            .data
            .data
            .get(&self.vault_key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(Credentials {
            username,
            password,
            fetched_at: chrono::Utc::now().to_rfc3339(),
            expires_at: None,
        })
    }

    /// Generate dynamic credentials via Vault's database secrets engine.
    pub async fn generate_dynamic(&self) -> anyhow::Result<Credentials> {
        let url = format!(
            "{}/v1/database/creds/{}",
            self.vault_addr, self.vault_role
        );

        let response = self
            .http_client
            .post(&url)
            .header("X-Vault-Token", &self.vault_token)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

        let lease = response.get("lease_duration").and_then(|v| v.as_u64()).unwrap_or(3600);
        let data = response.get("data").unwrap_or(&serde_json::Value::Null);

        let username = data
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let password = data
            .get("password")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let expires_at = chrono::Utc::now()
            + chrono::Duration::seconds(lease as i64);

        Ok(Credentials {
            username,
            password,
            fetched_at: chrono::Utc::now().to_rfc3339(),
            expires_at: Some(expires_at.to_rfc3339()),
        })
    }

    /// Write credentials to output file.
    pub async fn write_credentials(&self, creds: &Credentials) -> anyhow::Result<()> {
        if let Some(path) = &self.output_file {
            let content = format!("{}:{}\n", creds.username, creds.password);
            fs::write(path, &content).await?;
            tracing::info!("Wrote credentials to {}", path.display());
        }
        Ok(())
    }

    /// Perform a single rotation cycle.
    async fn rotate(&mut self) -> anyhow::Result<()> {
        let creds = if !self.vault_role.is_empty() {
            // Use dynamic credentials (database secrets engine)
            self.generate_dynamic().await?
        } else if !self.vault_secret.is_empty() {
            // Read static secret
            self.read_secret(&self.vault_secret).await?
        } else {
            return Err(anyhow::anyhow!("No VAULT_SECRET or VAULT_ROLE configured"));
        };

        self.write_credentials(&creds).await?;
        self.last_rotation = Some(chrono::Utc::now());
        self.rotation_success += 1;
        tracing::info!("Secret rotation successful (user={})", creds.username);
        Ok(())
    }
}

impl Default for VaultShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for VaultShim {
    fn name(&self) -> &str {
        "vault"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        // Override defaults with config
        if let Some(vault_config) = &config.vault {
            self.vault_addr = vault_config.addr.clone();
            self.vault_role = vault_config.role.clone();
            self.vault_secret = vault_config.secret.clone();
            self.rotation_secs = vault_config.rotation_secs;
        }
        tracing::info!(
            "VaultShim initialized (addr={}, secret={}, role={}, rotation={}s)",
            self.vault_addr,
            self.vault_secret,
            self.vault_role,
            self.rotation_secs,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        // Initial rotation
        if let Err(e) = self.rotate().await {
            tracing::warn!("Initial secret rotation failed: {}", e);
            self.rotation_failure += 1;
        }

        // Spawn rotation loop
        let rotation_secs = self.rotation_secs;
        let vault_addr = self.vault_addr.clone();
        let vault_token = self.vault_token.clone();
        let vault_role = self.vault_role.clone();
        let vault_secret = self.vault_secret.clone();
        let vault_key = self.vault_key.clone();
        let vault_mount = self.vault_mount.clone();
        let output_file = self.output_file.clone();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            let client = Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap_or_default();

            let mut shim = VaultShim {
                vault_addr,
                vault_token,
                vault_role,
                vault_secret,
                vault_key,
                vault_mount,
                output_file,
                rotation_secs,
                http_client: client,
                last_rotation: None,
                rotation_success: 0,
                rotation_failure: 0,
                shutdown_tx: None,
            };

            let mut interval = tokio::time::interval(
                std::time::Duration::from_secs(rotation_secs),
            );

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = shim.rotate().await {
                            tracing::error!("Secret rotation failed: {}", e);
                            shim.rotation_failure += 1;
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Vault shim rotation loop shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!("VaultShim started (rotation every {}s)", rotation_secs);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("VaultShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let mut metrics = vec![
            Metric::new("vault_rotation_success_total", self.rotation_success as f64),
            Metric::new("vault_rotation_failure_total", self.rotation_failure as f64),
        ];

        if let Some(last) = &self.last_rotation {
            metrics.push(Metric::new(
                "vault_rotation_last_success_timestamp",
                last.timestamp() as f64,
            ));
        }

        metrics
    }
}
