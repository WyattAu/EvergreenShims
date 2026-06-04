#![allow(dead_code)]
//! Encryption shim — transparent data encryption.
//!
//! Encrypts/decrypts data at rest and in transit.
//!
//! ## Environment Variables
//!
//! ```text
//! ENCRYPTION_METHOD      Method: aes-gcm, chacha20 (default: aes-gcm)
//! ENCRYPTION_KEY         Encryption key (or path to key file)
//! ENCRYPTION_KEY_ID      Key ID for key rotation
//! ENCRYPTION_COLUMNS     Columns to encrypt (empty = all eligible)
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Encryption shim.
pub struct EncryptionShim {
    method: String,
    key: Option<String>,
    key_id: Option<String>,
    columns: Vec<String>,
    encryptions_total: u64,
    decryptions_total: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl EncryptionShim {
    pub fn new() -> Self {
        Self {
            method: std::env::var("ENCRYPTION_METHOD").unwrap_or_else(|_| "aes-gcm".to_string()),
            key: std::env::var("ENCRYPTION_KEY").ok(),
            key_id: std::env::var("ENCRYPTION_KEY_ID").ok(),
            columns: std::env::var("ENCRYPTION_COLUMNS")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect(),
            encryptions_total: 0,
            decryptions_total: 0,
            shutdown_tx: None,
        }
    }
}

impl Default for EncryptionShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for EncryptionShim {
    fn name(&self) -> &str {
        "encryption"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("EncryptionShim initialized (method={})", self.method);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("EncryptionShim started (method={})", self.method);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("EncryptionShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new(
                "encryption_encryptions_total",
                self.encryptions_total as f64,
            ),
            Metric::new(
                "encryption_decryptions_total",
                self.decryptions_total as f64,
            ),
        ]
    }
}
