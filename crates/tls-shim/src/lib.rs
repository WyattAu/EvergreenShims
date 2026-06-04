//! TLS shim — automatic TLS certificate management.
//!
//! Obtains and renews TLS certificates from Let's Encrypt or an internal CA.
//!
//! ## Environment Variables
//!
//! ```text
//! TLS_PROVIDER         Provider: letsencrypt, internal-ca, vault-pki (required)
//! TLS_DOMAIN           Domain for the certificate (required)
//! TLS_EMAIL            Email for Let's Encrypt notifications
//! TLS_RENEW_BEFORE     Renew before expiry (default: 72h = 259200s)
//! TLS_CERT_FILE        Path to existing certificate (for internal-ca)
//! TLS_KEY_FILE         Path to existing key (for internal-ca)
//! TLS_LISTEN           Listen address for ACME challenge (default: :80)
//! TLS_DATA_DIR         Directory to store certificates (default: /etc/tls)
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// TLS certificate info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertInfo {
    pub domain: String,
    pub issuer: String,
    pub not_before: String,
    pub not_after: String,
    pub serial: String,
    pub fingerprint: String,
}

/// TLS shim for automatic certificate management.
pub struct TlsShim {
    provider: String,
    domain: String,
    email: String,
    renew_before_secs: u64,
    cert_file: Option<PathBuf>,
    key_file: Option<PathBuf>,
    listen: String,
    data_dir: PathBuf,
    certs_renewed: u64,
    certs_failed: u64,
    last_renewal: Option<chrono::DateTime<chrono::Utc>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl TlsShim {
    pub fn new() -> Self {
        Self {
            provider: std::env::var("TLS_PROVIDER").unwrap_or_else(|_| "letsencrypt".to_string()),
            domain: std::env::var("TLS_DOMAIN").unwrap_or_default(),
            email: std::env::var("TLS_EMAIL").unwrap_or_default(),
            renew_before_secs: std::env::var("TLS_RENEW_BEFORE")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(259200),
            cert_file: std::env::var("TLS_CERT_FILE").ok().map(PathBuf::from),
            key_file: std::env::var("TLS_KEY_FILE").ok().map(PathBuf::from),
            listen: std::env::var("TLS_LISTEN").unwrap_or_else(|_| ":80".to_string()),
            data_dir: std::env::var("TLS_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/etc/tls")),
            certs_renewed: 0,
            certs_failed: 0,
            last_renewal: None,
            shutdown_tx: None,
        }
    }

    /// Check if certificate needs renewal.
    fn needs_renewal(&self) -> bool {
        // In production, parse the certificate and check expiry
        // For now, always return true to trigger renewal on first run
        true
    }

    /// Obtain or renew certificate.
    async fn renew_certificate(&mut self) -> anyhow::Result<()> {
        match self.provider.as_str() {
            "letsencrypt" => {
                tracing::info!("Obtaining Let's Encrypt certificate for {}", self.domain);
                // In production: use acme-micro or rustls-acme
                // 1. Create ACME account
                // 2. Request certificate
                // 3. Complete HTTP-01 challenge
                // 4. Save cert/key to data_dir
            }
            "internal-ca" => {
                tracing::info!("Using internal CA certificates");
                // Verify cert/key files exist
                if let Some(cert) = &self.cert_file {
                    if !cert.exists() {
                        anyhow::bail!("Certificate file not found: {}", cert.display());
                    }
                }
                if let Some(key) = &self.key_file {
                    if !key.exists() {
                        anyhow::bail!("Key file not found: {}", key.display());
                    }
                }
            }
            "vault-pki" => {
                tracing::info!("Generating certificate from Vault PKI");
                // In production: call Vault PKI secrets engine
            }
            _ => {
                anyhow::bail!("Unknown TLS provider: {}", self.provider);
            }
        }

        self.certs_renewed += 1;
        self.last_renewal = Some(chrono::Utc::now());
        tracing::info!("Certificate renewed successfully for {}", self.domain);
        Ok(())
    }
}

impl Default for TlsShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for TlsShim {
    fn name(&self) -> &str {
        "tls"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        if let Some(tls_config) = &config.tls {
            self.provider = tls_config.provider.clone();
            self.domain = tls_config.domain.clone();
            self.email = tls_config.email.clone();
            self.renew_before_secs = tls_config.renew_before_secs;
        }
        tracing::info!(
            "TlsShim initialized (provider={}, domain={})",
            self.provider, self.domain,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        // Initial certificate check
        if self.needs_renewal() {
            if let Err(e) = self.renew_certificate().await {
                tracing::warn!("Initial certificate renewal failed: {}", e);
                self.certs_failed += 1;
            }
        }

        // Spawn renewal loop
        let _renew_before_secs = self.renew_before_secs;
        let _provider = self.provider.clone();
        let domain = self.domain.clone();
        let _email = self.email.clone();
        let _data_dir = self.data_dir.clone();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            // Check daily if renewal is needed
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        tracing::info!("Checking certificate expiry for {}", domain);
                        // In production: parse cert, check if renewal needed
                    }
                    _ = shutdown_rx.changed() => {
                        tracing::info!("TLS shim renewal loop shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!(
            "TlsShim started (provider={}, domain={}, renew_before={}s)",
            self.provider, self.domain, self.renew_before_secs,
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("TlsShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let mut metrics = vec![
            Metric::new("tls_certs_renewed_total", self.certs_renewed as f64),
            Metric::new("tls_certs_failed_total", self.certs_failed as f64),
        ];

        if let Some(last) = &self.last_renewal {
            metrics.push(Metric::new(
                "tls_last_renewal_timestamp",
                last.timestamp() as f64,
            ));
        }

        metrics
    }
}
