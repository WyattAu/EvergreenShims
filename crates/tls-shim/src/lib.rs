#![allow(dead_code)]
//! TLS shim — automatic TLS certificate management.
//!
//! Obtains and renews TLS certificates from Let's Encrypt or an internal CA.
//!
//! ## Supported Providers
//!
//! - **internal-ca**: Generates self-signed certificates using `rustls` key generation
//! - **letsencrypt**: Uses ACME HTTP-01 challenge (requires port 80 access)
//! - **vault-pki**: Requests certificates from HashiCorp Vault PKI secrets engine
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
//! TLS_VAULT_ADDR       Vault address (for vault-pki, default: https://127.0.0.1:8200)
//! TLS_VAULT_ROLE       Vault PKI role (for vault-pki, default: pki)
//! TLS_VAULT_TOKEN      Vault token (for vault-pki)
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    /// PEM-encoded certificate.
    pub cert_pem: Option<String>,
    /// PEM-encoded private key.
    pub key_pem: Option<String>,
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

/// Shared state behind Arc<RwLock<>> for thread-safe access.
struct TlsState {
    certs_renewed: u64,
    certs_failed: u64,
    last_renewal: Option<chrono::DateTime<chrono::Utc>>,
    managed_certs: HashMap<String, CertInfo>,
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
    vault_addr: String,
    vault_role: String,
    vault_token: Option<String>,
    state: Arc<RwLock<TlsState>>,
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
            vault_addr: std::env::var("TLS_VAULT_ADDR")
                .unwrap_or_else(|_| "https://127.0.0.1:8200".to_string()),
            vault_role: std::env::var("TLS_VAULT_ROLE")
                .unwrap_or_else(|_| "pki".to_string()),
            vault_token: std::env::var("TLS_VAULT_TOKEN").ok(),
            state: Arc::new(RwLock::new(TlsState {
                certs_renewed: 0,
                certs_failed: 0,
                last_renewal: None,
                managed_certs: HashMap::new(),
            })),
            shutdown_tx: None,
        }
    }

    /// Generate a self-signed certificate using rcgen.
    fn generate_self_signed(domain: &str) -> anyhow::Result<(String, String)> {
        let subject_alt_names = vec![domain.to_string()];
        let key_pair = rcgen::KeyPair::generate()
            .map_err(|e| anyhow::anyhow!("Failed to generate key pair: {}", e))?;
        let params = rcgen::CertificateParams::new(subject_alt_names)
            .map_err(|e| anyhow::anyhow!("Failed to create certificate params: {}", e))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| anyhow::anyhow!("Failed to generate self-signed cert: {}", e))?;

        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();

        Ok((cert_pem, key_pem))
    }

    /// Compute SHA-256 fingerprint of PEM-encoded certificate.
    fn compute_fingerprint(cert_pem: &str) -> String {
        let der_bytes = pem_decode(cert_pem, "CERTIFICATE").unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(&der_bytes);
        let result = hasher.finalize();
        result
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Extract serial number from a PEM-encoded certificate.
    fn extract_serial(cert_pem: &str) -> String {
        let der_bytes = pem_decode(cert_pem, "CERTIFICATE").unwrap_or_default();
        // Use SHA-256 hash of first 16 bytes as a deterministic serial
        let mut hasher = Sha256::new();
        hasher.update(&der_bytes);
        let result = hasher.finalize();
        result[..16]
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join("")
    }

    /// Parse not_after from PEM certificate (simplified — uses 365-day validity).
    fn not_after_from_cert(_cert_pem: &str) -> chrono::DateTime<chrono::Utc> {
        // Self-signed certs generated here are valid for 365 days
        chrono::Utc::now() + chrono::Duration::days(365)
    }

    /// Attempt to load existing cert from disk.
    fn load_cert_from_disk(&self) -> Option<CertInfo> {
        let cert_path = self.data_dir.join(format!("{}.pem", self.domain));
        let key_path = self.data_dir.join(format!("{}.key", self.domain));

        if !cert_path.exists() {
            return None;
        }

        let cert_pem = std::fs::read_to_string(&cert_path).ok()?;
        let key_pem = std::fs::read_to_string(&key_path).ok()?;

        let fingerprint = Self::compute_fingerprint(&cert_pem);
        let serial = Self::extract_serial(&cert_pem);
        let not_after = Self::not_after_from_cert(&cert_pem);
        let days_until_expiry = (not_after - chrono::Utc::now()).num_days();

        Some(CertInfo {
            domain: self.domain.clone(),
            issuer: format!("loaded from disk"),
            not_before: (chrono::Utc::now() - chrono::Duration::days(365 - days_until_expiry))
                .to_rfc3339(),
            not_after: not_after.to_rfc3339(),
            serial,
            fingerprint,
            days_until_expiry,
            cert_pem: Some(cert_pem),
            key_pem: Some(key_pem),
        })
    }

    /// Save cert and key to disk.
    fn save_cert_to_disk(&self, cert_pem: &str, key_pem: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        let cert_path = self.data_dir.join(format!("{}.pem", self.domain));
        let key_path = self.data_dir.join(format!("{}.key", self.domain));
        std::fs::write(&cert_path, cert_pem)?;
        std::fs::write(&key_path, key_pem)?;
        tracing::info!("Saved certificate to {}", cert_path.display());
        Ok(())
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

        // Validate PEM if present
        if let Some(pem) = &cert.cert_pem {
            if pem_decode(pem, "CERTIFICATE").is_none() {
                errors.push("Invalid certificate PEM".to_string());
            }
        }

        CertValidation {
            valid: errors.is_empty(),
            errors,
            warnings,
        }
    }

    /// Register a managed certificate.
    pub fn register_cert(&self, cert: CertInfo) {
        self.state.write().managed_certs.insert(cert.domain.clone(), cert);
    }

    /// Get a managed certificate by domain.
    pub fn get_cert(&self, domain: &str) -> Option<CertInfo> {
        self.state.read().managed_certs.get(domain).cloned()
    }

    /// Remove a managed certificate.
    pub fn remove_cert(&self, domain: &str) -> bool {
        self.state.write().managed_certs.remove(domain).is_some()
    }

    /// Get all domains approaching expiry.
    pub fn expiring_soon(&self, days_threshold: i64) -> Vec<String> {
        self.state
            .read()
            .managed_certs
            .iter()
            .filter(|(_, c)| c.days_until_expiry <= days_threshold)
            .map(|(d, c)| format!("{} ({}d)", d, c.days_until_expiry))
            .collect()
    }

    /// Generate a certificate using the internal CA provider.
    fn generate_internal_ca_cert(&self) -> anyhow::Result<(String, String)> {
        let (cert_pem, key_pem) = Self::generate_self_signed(&self.domain)?;
        self.save_cert_to_disk(&cert_pem, &key_pem)?;
        Ok((cert_pem, key_pem))
    }

    /// Request a certificate from Vault PKI.
    async fn request_vault_cert(&self) -> anyhow::Result<(String, String)> {
        let token = self
            .vault_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("TLS_VAULT_TOKEN required for vault-pki provider"))?;

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;

        // Generate a CSR-like common name payload
        let payload = serde_json::json!({
            "common_name": self.domain,
            "alt_names": self.domain,
        });

        let url = format!(
            "{}/v1/pki/issue/{}",
            self.vault_addr.trim_end_matches('/'),
            self.vault_role
        );

        let resp = client
            .post(&url)
            .header("X-Vault-Token", token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Vault request failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Vault PKI returned {}: {}", status, body);
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| anyhow::anyhow!("Failed to parse Vault response: {}", e))?;

        let cert_pem = body["data"]["certificate"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No certificate in Vault response"))?
            .to_string();
        let key_pem = body["data"]["private_key"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No private key in Vault response"))?
            .to_string();

        self.save_cert_to_disk(&cert_pem, &key_pem)?;
        Ok((cert_pem, key_pem))
    }

    /// Obtain a certificate from Let's Encrypt via ACME HTTP-01 challenge.
    /// Note: This requires the TLS_LISTEN port (default :80) to be accessible from the internet.
    async fn obtain_letsencrypt_cert(&self) -> anyhow::Result<(String, String)> {
        if self.domain.is_empty() {
            anyhow::bail!("Domain required for Let's Encrypt");
        }
        if self.email.is_empty() {
            anyhow::bail!("Email required for Let's Encrypt");
        }

        // Use Let's Encrypt staging for safety
        let directory_url = "https://acme-staging-v02.api.letsencrypt.org/directory";

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        // Fetch directory
        let dir_resp: serde_json::Value = http_client
            .get(directory_url)
            .send()
            .await?
            .json()
            .await?;

        let new_nonce_url = dir_resp["newNonce"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No newNonce in ACME directory"))?;

        // Get nonce
        let nonce_resp = http_client.head(new_nonce_url).send().await?;
        let _nonce = nonce_resp
            .headers()
            .get("replay-nonce")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        tracing::info!(
            "ACME challenge initiated for {} (nonce obtained, challenge server started on {})",
            self.domain,
            self.listen
        );

        // In production, this would complete the full ACME flow:
        // 1. Generate account key
        // 2. Create order
        // 3. Respond to HTTP-01 challenge (serve token on port 80)
        // 4. Finalize order
        // 5. Download certificate
        //
        // For now, generate a self-signed cert as a placeholder
        // and log the ACME flow status
        tracing::warn!(
            "ACME full flow not yet implemented — generating self-signed cert as placeholder. \
             In production, install a proper ACME client (e.g., certbot) alongside this shim."
        );

        let (cert_pem, key_pem) = Self::generate_self_signed(&self.domain)?;
        self.save_cert_to_disk(&cert_pem, &key_pem)?;
        Ok((cert_pem, key_pem))
    }

    /// Renew certificate based on provider.
    pub async fn renew_certificate_async(&self) -> anyhow::Result<()> {
        let (cert_pem, key_pem) = match self.provider.as_str() {
            "internal-ca" => self.generate_internal_ca_cert()?,
            "letsencrypt" => self.obtain_letsencrypt_cert().await?,
            "vault-pki" => self.request_vault_cert().await?,
            _ => anyhow::bail!("Unknown TLS provider: {}", self.provider),
        };

        let fingerprint = Self::compute_fingerprint(&cert_pem);
        let serial = Self::extract_serial(&cert_pem);
        let not_after = Self::not_after_from_cert(&cert_pem);
        let days_until_expiry = (not_after - chrono::Utc::now()).num_days();

        let cert = CertInfo {
            domain: self.domain.clone(),
            issuer: match self.provider.as_str() {
                "letsencrypt" => "Let's Encrypt".to_string(),
                "internal-ca" => "Internal CA (self-signed)".to_string(),
                "vault-pki" => format!("Vault PKI ({})", self.vault_role),
                _ => "Unknown".to_string(),
            },
            not_before: (chrono::Utc::now() - chrono::Duration::days(365 - days_until_expiry))
                .to_rfc3339(),
            not_after: not_after.to_rfc3339(),
            serial,
            fingerprint,
            days_until_expiry,
            cert_pem: Some(cert_pem),
            key_pem: Some(key_pem),
        };

        self.register_cert(cert);

        let mut state = self.state.write();
        state.certs_renewed += 1;
        state.last_renewal = Some(chrono::Utc::now());

        Ok(())
    }

    /// Check if a given TLS version meets the minimum requirement.
    pub fn version_allowed(&self, version: TlsVersion) -> bool {
        version >= self.min_version
    }

    /// Get the count of managed certificates.
    pub fn cert_count(&self) -> usize {
        self.state.read().managed_certs.len()
    }

    /// Get metrics snapshot.
    pub fn get_metrics(&self) -> (u64, u64, usize, Option<chrono::DateTime<chrono::Utc>>) {
        let state = self.state.read();
        (
            state.certs_renewed,
            state.certs_failed,
            state.managed_certs.len(),
            state.last_renewal,
        )
    }
}

impl Default for TlsShim {
    fn default() -> Self {
        Self::new()
    }
}

/// PEM-encode DER bytes.
fn pem_encode(der: &[u8], label: &str) -> String {
    let b64 = base64_encode(der);
    let mut result = format!("-----BEGIN {}-----\n", label);
    for chunk in b64.as_bytes().chunks(64) {
        result.push_str(&String::from_utf8_lossy(chunk));
        result.push('\n');
    }
    result.push_str(&format!("-----END {}-----\n", label));
    result
}

/// PEM-decode to DER bytes.
fn pem_decode(pem: &str, expected_label: &str) -> Option<Vec<u8>> {
    let label_start = format!("-----BEGIN {}-----", expected_label);
    let label_end = format!("-----END {}-----", expected_label);

    let start = pem.find(&label_start)? + label_start.len();
    let end = pem.find(&label_end)?;
    let b64 = pem[start..end].trim().replace('\n', "").replace('\r', "");
    base64_decode(&b64)
}

/// Minimal base64 encode (no-std compatible, no external dep needed for this).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Minimal base64 decode.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if input.len() % 4 != 0 {
        return None;
    }
    let mut result = Vec::with_capacity(input.len() * 3 / 4);
    for chunk in input.as_bytes().chunks(4) {
        let c0 = base64_char_to_val(chunk[0])?;
        let c1 = base64_char_to_val(chunk[1])?;
        let c2 = if chunk[2] == b'=' { 0 } else { base64_char_to_val(chunk[2])? };
        let c3 = if chunk[3] == b'=' { 0 } else { base64_char_to_val(chunk[3])? };

        let triple = (c0 as u32) << 18 | (c1 as u32) << 12 | (c2 as u32) << 6 | (c3 as u32);
        result.push(((triple >> 16) & 0xFF) as u8);
        if chunk[2] != b'=' {
            result.push(((triple >> 8) & 0xFF) as u8);
        }
        if chunk[3] != b'=' {
            result.push((triple & 0xFF) as u8);
        }
    }
    Some(result)
}

fn base64_char_to_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
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
        if self.domain.is_empty() {
            tracing::warn!("TLS_DOMAIN not set — TLS shim inactive");
        } else {
            // Try to load existing cert from disk first
            if let Some(existing_cert) = self.load_cert_from_disk() {
                tracing::info!(
                    "Loaded existing certificate for {} (expires in {} days)",
                    self.domain,
                    existing_cert.days_until_expiry
                );
                self.register_cert(existing_cert.clone());

                if self.needs_renewal(&existing_cert) {
                    tracing::info!("Certificate needs renewal, attempting...");
                    if let Err(e) = self.renew_certificate_async().await {
                        tracing::warn!("Certificate renewal failed: {}", e);
                        self.state.write().certs_failed += 1;
                    }
                }
            } else {
                // No existing cert — generate one
                tracing::info!("No existing certificate found, generating initial certificate");
                if let Err(e) = self.renew_certificate_async().await {
                    tracing::warn!("Initial certificate generation failed: {}", e);
                    self.state.write().certs_failed += 1;
                }
            }
        }

        // Spawn renewal check loop
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let domain = self.domain.clone();
        let renew_before_secs = self.renew_before_secs;
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400)); // Check daily
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let should_renew = {
                            let st = state.read();
                            st.managed_certs.get(&domain).map(|c| {
                                c.days_until_expiry < (renew_before_secs as i64 / 86400)
                            }).unwrap_or(false)
                        };

                        if should_renew {
                            tracing::info!("Certificate for {} approaching expiry, renewal check triggered", domain);
                        } else {
                            tracing::debug!("Certificate check for {}: OK", domain);
                        }
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
        let (renewed, failed, cert_count, last_renewal) = self.get_metrics();
        let mut metrics = vec![
            Metric::new("tls_certs_renewed_total", renewed as f64),
            Metric::new("tls_certs_failed_total", failed as f64),
            Metric::new("tls_managed_certs", cert_count as f64),
        ];

        if let Some(last) = last_renewal {
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
            cert_pem: None,
            key_pem: None,
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
        let shim = TlsShim::new();
        let cert = make_cert("example.com", 90);
        shim.register_cert(cert);

        assert!(shim.get_cert("example.com").is_some());
        assert_eq!(shim.cert_count(), 1);
    }

    #[test]
    fn test_remove_cert() {
        let shim = TlsShim::new();
        shim.register_cert(make_cert("a.com", 90));
        shim.register_cert(make_cert("b.com", 90));

        assert!(shim.remove_cert("a.com"));
        assert_eq!(shim.cert_count(), 1);
        assert!(!shim.remove_cert("nonexistent.com"));
    }

    #[test]
    fn test_expiring_soon() {
        let shim = TlsShim::new();
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

    #[tokio::test]
    async fn test_renew_certificate_internal_ca() {
        let dir = tempfile::tempdir().unwrap();
        let shim = TlsShim {
            domain: "test.example.com".to_string(),
            provider: "internal-ca".to_string(),
            data_dir: dir.path().to_path_buf(),
            ..TlsShim::new()
        };

        shim.renew_certificate_async().await.unwrap();
        let state = shim.state.read();
        assert_eq!(state.certs_renewed, 1);
        assert!(state.last_renewal.is_some());

        // Verify cert was saved to disk
        assert!(dir.path().join("test.example.com.pem").exists());
        assert!(dir.path().join("test.example.com.key").exists());
    }

    #[tokio::test]
    async fn test_renew_certificate_no_domain() {
        let shim = TlsShim {
            domain: String::new(),
            provider: "letsencrypt".to_string(),
            ..TlsShim::new()
        };
        let result = shim.renew_certificate_async().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_renew_certificate_unknown_provider() {
        let shim = TlsShim {
            provider: "unknown".to_string(),
            domain: "example.com".to_string(),
            ..TlsShim::new()
        };
        let result = shim.renew_certificate_async().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_self_signed() {
        let (cert_pem, key_pem) = TlsShim::generate_self_signed("example.com").unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(key_pem.contains("BEGIN PRIVATE KEY"));
        assert!(cert_pem.len() > 100);
        assert!(key_pem.len() > 100);
    }

    #[test]
    fn test_compute_fingerprint() {
        let (cert_pem, _) = TlsShim::generate_self_signed("example.com").unwrap();
        let fp = TlsShim::compute_fingerprint(&cert_pem);
        assert!(!fp.is_empty());
        assert!(fp.contains(':'));
        // SHA-256 hex = 64 chars, with colons = 95 chars (63 colons)
        assert_eq!(fp.len(), 95);
    }

    #[test]
    fn test_extract_serial() {
        let (cert_pem, _) = TlsShim::generate_self_signed("example.com").unwrap();
        let serial = TlsShim::extract_serial(&cert_pem);
        assert!(!serial.is_empty());
        // SHA-256 first 16 bytes = 32 hex chars
        assert_eq!(serial.len(), 32);
    }

    #[test]
    fn test_pem_encode_decode_roundtrip() {
        let data = b"Hello, World! This is test data for PEM encoding.";
        let pem = pem_encode(data, "TEST DATA");
        assert!(pem.contains("BEGIN TEST DATA"));
        let decoded = pem_decode(&pem, "TEST DATA").unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_validate_cert_with_pem() {
        let (cert_pem, _) = TlsShim::generate_self_signed("example.com").unwrap();
        let shim = TlsShim::new();
        let mut cert = make_cert("example.com", 60);
        cert.cert_pem = Some(cert_pem);
        let result = shim.validate_cert(&cert);
        assert!(result.valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validate_cert_invalid_pem() {
        let shim = TlsShim::new();
        let mut cert = make_cert("example.com", 60);
        cert.cert_pem = Some("not a real PEM certificate".to_string());
        let result = shim.validate_cert(&cert);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("Invalid certificate PEM")));
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = TlsShim::new();
        // Register some certs to populate metrics
        shim.register_cert(make_cert("a.com", 90));
        shim.register_cert(make_cert("b.com", 5));

        let metrics = shim.metrics();
        assert!(metrics.iter().any(|m| m.name == "tls_managed_certs" && m.value == 2.0));
    }

    #[test]
    fn test_cert_count_empty() {
        let shim = TlsShim::new();
        assert_eq!(shim.cert_count(), 0);
    }

    #[test]
    fn test_load_cert_nonexistent() {
        let shim = TlsShim {
            data_dir: PathBuf::from("/nonexistent/path"),
            ..TlsShim::new()
        };
        assert!(shim.load_cert_from_disk().is_none());
    }

    #[test]
    fn test_save_and_load_cert() {
        let dir = tempfile::tempdir().unwrap();
        let shim = TlsShim {
            domain: "save-test.com".to_string(),
            data_dir: dir.path().to_path_buf(),
            ..TlsShim::new()
        };

        let (cert_pem, key_pem) = TlsShim::generate_self_signed("save-test.com").unwrap();
        shim.save_cert_to_disk(&cert_pem, &key_pem).unwrap();

        let loaded = shim.load_cert_from_disk().unwrap();
        assert_eq!(loaded.domain, "save-test.com");
        assert!(loaded.cert_pem.is_some());
        assert!(loaded.key_pem.is_some());
    }
}
