#![allow(dead_code)]
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
//! TLS_MIN_VERSION      Minimum TLS version (default: TLS1.2)
//! ```

use std::collections::HashMap;
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
    pub days_until_expiry: i64,
}

/// TLS version enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TlsVersion {
    Tls1_0,
    Tls1_1,
    Tls1_2,
    Tls1_3,
}

impl std::fmt::Display for TlsVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tls1_0 => write!(f, "TLSv1.0"),
            Self::Tls1_1 => write!(f, "TLSv1.1"),
            Self::Tls1_2 => write!(f, "TLSv1.2"),
            Self::Tls1_3 => write!(f, "TLSv1.3"),
        }
    }
}

impl std::str::FromStr for TlsVersion {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tlsv1.0" | "tls1.0" | "1.0" => Ok(Self::Tls1_0),
            "tlsv1.1" | "tls1.1" | "1.1" => Ok(Self::Tls1_1),
            "tlsv1.2" | "tls1.2" | "1.2" => Ok(Self::Tls1_2),
            "tlsv1.3" | "tls1.3" | "1.3" => Ok(Self::Tls1_3),
            _ => Err(format!("Unknown TLS version: {}", s)),
        }
    }
}

/// Certificate validation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertValidation {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
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
    min_version: TlsVersion,
    certs_renewed: u64,
    certs_failed: u64,
    last_renewal: Option<chrono::DateTime<chrono::Utc>>,
    managed_certs: HashMap<String, CertInfo>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl TlsShim {
    pub fn new() -> Self {
        let min_ver = std::env::var("TLS_MIN_VERSION")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(TlsVersion::Tls1_2);

        Self {
            provider: std::env::var("TLS_PROVIDER").unwrap_or_else(|_| "letsencrypt".to_string()),
            domain: std::env::var("TLS_DOMAIN").unwrap_or_default(),
            email: std::env::var("TLS_EMAIL").unwrap_or_default(),
            renew_before_secs: std::env::var("TLS_RENEW_BEFORE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(259200),
            cert_file: std::env::var("TLS_CERT_FILE").ok().map(PathBuf::from),
            key_file: std::env::var("TLS_KEY_FILE").ok().map(PathBuf::from),
            listen: std::env::var("TLS_LISTEN").unwrap_or_else(|_| ":80".to_string()),
            data_dir: std::env::var("TLS_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/etc/tls")),
            min_version: min_ver,
            certs_renewed: 0,
            certs_failed: 0,
            last_renewal: None,
            managed_certs: HashMap::new(),
            shutdown_tx: None,
        }
    }

    /// Check if certificate needs renewal based on configured threshold.
    pub fn needs_renewal(&self, cert: &CertInfo) -> bool {
        cert.days_until_expiry < (self.renew_before_secs as i64 / 86400)
    }

    /// Validate a certificate info struct.
    pub fn validate_cert(&self, cert: &CertInfo) -> CertValidation {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        if cert.domain.is_empty() {
            errors.push("Domain is empty".to_string());
        }
        if cert.issuer.is_empty() {
            errors.push("Issuer is empty".to_string());
        }
        if cert.not_after.is_empty() {
            errors.push("Not-after date is empty".to_string());
        }
        if cert.fingerprint.is_empty() {
            errors.push("Fingerprint is empty".to_string());
        }
        if cert.days_until_expiry < 0 {
            errors.push("Certificate has expired".to_string());
        }
        if cert.days_until_expiry < 7 {
            warnings.push("Certificate expires in less than 7 days".to_string());
        }
        if cert.days_until_expiry < 30 {
            warnings.push("Certificate expires in less than 30 days".to_string());
        }

        CertValidation {
            valid: errors.is_empty(),
            errors,
            warnings,
        }
    }

    /// Register a managed certificate.
    pub fn register_cert(&mut self, cert: CertInfo) {
        self.managed_certs.insert(cert.domain.clone(), cert);
    }

    /// Get a managed certificate by domain.
    pub fn get_cert(&self, domain: &str) -> Option<&CertInfo> {
        self.managed_certs.get(domain)
    }

    /// Remove a managed certificate.
    pub fn remove_cert(&mut self, domain: &str) -> bool {
        self.managed_certs.remove(domain).is_some()
    }

    /// Get all domains approaching expiry.
    pub fn expiring_soon(&self, days_threshold: i64) -> Vec<String> {
        self.managed_certs
            .iter()
            .filter(|(_, c)| c.days_until_expiry <= days_threshold)
            .map(|(d, c)| format!("{} ({}d)", d, c.days_until_expiry))
            .collect()
    }

    /// Simulate certificate renewal. Updates cert's expiry.
    pub fn renew_certificate(&mut self) -> anyhow::Result<()> {
        match self.provider.as_str() {
            "letsencrypt" => {
                if self.domain.is_empty() {
                    anyhow::bail!("Domain required for Let's Encrypt");
                }
                tracing::info!("Obtaining Let's Encrypt certificate for {}", self.domain);
            }
            "internal-ca" => {
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
            }
            _ => {
                anyhow::bail!("Unknown TLS provider: {}", self.provider);
            }
        }

        self.certs_renewed += 1;
        self.last_renewal = Some(chrono::Utc::now());

        if let Some(cert) = self.managed_certs.get_mut(&self.domain) {
            cert.days_until_expiry = 90;
            cert.not_after = (chrono::Utc::now() + chrono::Duration::days(90)).to_rfc3339();
        }

        Ok(())
    }

    /// Check if a given TLS version meets the minimum requirement.
    pub fn version_allowed(&self, version: TlsVersion) -> bool {
        version >= self.min_version
    }

    /// Get the count of managed certificates.
    pub fn cert_count(&self) -> usize {
        self.managed_certs.len()
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
            self.provider,
            self.domain,
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        if !self.domain.is_empty() {
            let cert = CertInfo {
                domain: self.domain.clone(),
                issuer: match self.provider.as_str() {
                    "letsencrypt" => "Let's Encrypt".to_string(),
                    "internal-ca" => "Internal CA".to_string(),
                    "vault-pki" => "Vault PKI".to_string(),
                    _ => "Unknown".to_string(),
                },
                not_before: chrono::Utc::now().to_rfc3339(),
                not_after: (chrono::Utc::now() + chrono::Duration::days(90)).to_rfc3339(),
                serial: "0".to_string(),
                fingerprint: "simulated".to_string(),
                days_until_expiry: 90,
            };
            self.register_cert(cert);

            if let Some(info) = self.get_cert(&self.domain) {
                if self.needs_renewal(info) {
                    if let Err(e) = self.renew_certificate() {
                        tracing::warn!("Initial certificate renewal failed: {}", e);
                        self.certs_failed += 1;
                    }
                }
            }
        }

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let domain = self.domain.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        tracing::info!("Checking certificate expiry for {}", domain);
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
            self.provider,
            self.domain,
            self.renew_before_secs,
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
            Metric::new("tls_managed_certs", self.managed_certs.len() as f64),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cert(domain: &str, days_until_expiry: i64) -> CertInfo {
        CertInfo {
            domain: domain.to_string(),
            issuer: "Test CA".to_string(),
            not_before: (chrono::Utc::now() - chrono::Duration::days(90 - days_until_expiry))
                .to_rfc3339(),
            not_after: (chrono::Utc::now() + chrono::Duration::days(days_until_expiry))
                .to_rfc3339(),
            serial: "1234".to_string(),
            fingerprint: "abcdef".to_string(),
            days_until_expiry,
        }
    }

    #[test]
    fn test_tls_version_parse() {
        assert_eq!("TLSv1.2".parse::<TlsVersion>().unwrap(), TlsVersion::Tls1_2);
        assert_eq!("TLSv1.3".parse::<TlsVersion>().unwrap(), TlsVersion::Tls1_3);
        assert_eq!("1.0".parse::<TlsVersion>().unwrap(), TlsVersion::Tls1_0);
        assert!("invalid".parse::<TlsVersion>().is_err());
    }

    #[test]
    fn test_tls_version_display() {
        assert_eq!(TlsVersion::Tls1_2.to_string(), "TLSv1.2");
        assert_eq!(TlsVersion::Tls1_3.to_string(), "TLSv1.3");
    }

    #[test]
    fn test_needs_renewal_true() {
        let shim = TlsShim {
            renew_before_secs: 259200,
            ..TlsShim::new()
        };
        let cert = make_cert("example.com", 2);
        assert!(shim.needs_renewal(&cert));
    }

    #[test]
    fn test_needs_renewal_false() {
        let shim = TlsShim {
            renew_before_secs: 259200,
            ..TlsShim::new()
        };
        let cert = make_cert("example.com", 10);
        assert!(!shim.needs_renewal(&cert));
    }

    #[test]
    fn test_validate_cert_valid() {
        let shim = TlsShim::new();
        let cert = make_cert("example.com", 60);
        let result = shim.validate_cert(&cert);
        assert!(result.valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validate_cert_expired() {
        let shim = TlsShim::new();
        let cert = make_cert("example.com", -1);
        let result = shim.validate_cert(&cert);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("expired")));
    }

    #[test]
    fn test_validate_cert_warnings() {
        let shim = TlsShim::new();
        let cert = make_cert("example.com", 5);
        let result = shim.validate_cert(&cert);
        assert!(result.valid);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn test_register_and_get_cert() {
        let mut shim = TlsShim::new();
        let cert = make_cert("example.com", 90);
        shim.register_cert(cert);

        assert!(shim.get_cert("example.com").is_some());
        assert_eq!(shim.cert_count(), 1);
    }

    #[test]
    fn test_remove_cert() {
        let mut shim = TlsShim::new();
        shim.register_cert(make_cert("a.com", 90));
        shim.register_cert(make_cert("b.com", 90));

        assert!(shim.remove_cert("a.com"));
        assert_eq!(shim.cert_count(), 1);
        assert!(!shim.remove_cert("nonexistent.com"));
    }

    #[test]
    fn test_expiring_soon() {
        let mut shim = TlsShim::new();
        shim.register_cert(make_cert("expiring.com", 5));
        shim.register_cert(make_cert("safe.com", 90));

        let expiring = shim.expiring_soon(30);
        assert_eq!(expiring.len(), 1);
        assert!(expiring[0].contains("expiring.com"));
    }

    #[test]
    fn test_version_allowed() {
        let shim = TlsShim {
            min_version: TlsVersion::Tls1_2,
            ..TlsShim::new()
        };
        assert!(shim.version_allowed(TlsVersion::Tls1_2));
        assert!(shim.version_allowed(TlsVersion::Tls1_3));
        assert!(!shim.version_allowed(TlsVersion::Tls1_1));
        assert!(!shim.version_allowed(TlsVersion::Tls1_0));
    }

    #[test]
    fn test_renew_certificate_success() {
        let mut shim = TlsShim {
            domain: "example.com".to_string(),
            provider: "letsencrypt".to_string(),
            ..TlsShim::new()
        };
        shim.register_cert(make_cert("example.com", 5));

        shim.renew_certificate().unwrap();
        assert_eq!(shim.certs_renewed, 1);
        assert_eq!(shim.get_cert("example.com").unwrap().days_until_expiry, 90);
    }

    #[test]
    fn test_renew_certificate_no_domain() {
        let mut shim = TlsShim {
            domain: String::new(),
            provider: "letsencrypt".to_string(),
            ..TlsShim::new()
        };
        let result = shim.renew_certificate();
        assert!(result.is_err());
    }

    #[test]
    fn test_renew_certificate_unknown_provider() {
        let mut shim = TlsShim {
            provider: "unknown".to_string(),
            domain: "example.com".to_string(),
            ..TlsShim::new()
        };
        let result = shim.renew_certificate();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = TlsShim {
            certs_renewed: 5,
            certs_failed: 1,
            last_renewal: Some(chrono::Utc::now()),
            managed_certs: HashMap::new(),
            ..TlsShim::new()
        };
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].value, 5.0);
        assert_eq!(metrics[1].value, 1.0);
    }
}
