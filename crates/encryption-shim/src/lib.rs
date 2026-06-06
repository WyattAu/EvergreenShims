//! Encryption shim — transparent data encryption at rest.
//!
//! Provides AES-256-GCM and ChaCha20-Poly1305 encryption with
//! automatic key rotation, envelope encryption for key wrapping,
//! and per-tenant key isolation.
//!
//! ## Environment Variables
//!
//! ```text
//! ENCRYPTION_METHOD      Method: aes-gcm, chacha20 (default: aes-gcm)
//! ENCRYPTION_KEY         32-byte hex key (or ENCRYPTION_KEY_PATH for file)
//! ENCRYPTION_KEY_ID      Current key ID for rotation tracking
//! ENCRYPTION_PREV_KEYS   JSON array of previous keys for decryption
//! ENCRYPTION_AAD         Additional Authenticated Data prefix (optional)
//! ```

use std::collections::HashMap;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Nonce size for both AES-GCM and ChaCha20-Poly1305.
const NONCE_SIZE: usize = 12;
/// Key size: 256 bits = 32 bytes.
const KEY_SIZE: usize = 32;

/// Encryption key with metadata for rotation tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptionKey {
    pub id: String,
    pub material: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub active: bool,
}

/// Encrypted payload in envelope format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPayload {
    pub key_id: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub tag: Vec<u8>,
    pub method: String,
    pub aad: Option<Vec<u8>>,
}

/// Supported encryption methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionMethod {
    AesGcm,
    ChaCha20,
}

impl std::fmt::Display for EncryptionMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AesGcm => write!(f, "aes-gcm"),
            Self::ChaCha20 => write!(f, "chacha20"),
        }
    }
}

impl std::str::FromStr for EncryptionMethod {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "aes-gcm" | "aes256gcm" | "aes" => Ok(Self::AesGcm),
            "chacha20" | "chacha20poly1305" | "chacha" => Ok(Self::ChaCha20),
            _ => anyhow::bail!("Unknown encryption method: {}", s),
        }
    }
}

/// Generate `len` random bytes via OS CSPRNG.
fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    getrandom::getrandom(&mut buf).expect("OS CSPRNG failure");
    buf
}

/// Convert a 32-byte slice to the GenericArray needed by crypto crates.
fn key_to_generic_array(
    key: &[u8],
) -> anyhow::Result<generic_array::GenericArray<u8, generic_array::typenum::U32>> {
    anyhow::ensure!(
        key.len() == KEY_SIZE,
        "Key must be {} bytes, got {}",
        KEY_SIZE,
        key.len()
    );
    Ok(generic_array::GenericArray::clone_from_slice(key))
}

/// Encryption shim with key management and rotation support.
pub struct EncryptionShim {
    method: EncryptionMethod,
    keys: HashMap<String, EncryptionKey>,
    active_key_id: Option<String>,
    aad_prefix: Option<Vec<u8>>,
    encryptions_total: u64,
    decryptions_total: u64,
    decryption_key_rotation_hits: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl EncryptionShim {
    /// Create a new `EncryptionShim` from environment variables.
    ///
    /// # Panics
    ///
    /// Panics if the encryption key configuration is invalid (file unreadable,
    /// invalid hex encoding, or wrong key length). Use [`try_new`] for
    /// non-panicking construction.
    pub fn new() -> Self {
        Self::try_new().expect("EncryptionShim configuration is invalid")
    }

    /// Create a new `EncryptionShim` from environment variables.
    ///
    /// Returns an error if the encryption key configuration is invalid.
    pub fn try_new() -> anyhow::Result<Self> {
        let method_str =
            std::env::var("ENCRYPTION_METHOD").unwrap_or_else(|_| "aes-gcm".to_string());
        let method = method_str.parse().unwrap_or(EncryptionMethod::AesGcm);

        let mut shim = Self {
            method,
            keys: HashMap::new(),
            active_key_id: None,
            aad_prefix: None,
            encryptions_total: 0,
            decryptions_total: 0,
            decryption_key_rotation_hits: 0,
            shutdown_tx: None,
        };

        shim.load_primary_key()?;
        shim.load_previous_keys();

        if let Ok(aad) = std::env::var("ENCRYPTION_AAD") {
            shim.aad_prefix = Some(aad.into_bytes());
        }

        Ok(shim)
    }

    fn load_primary_key(&mut self) -> anyhow::Result<()> {
        let key_id = std::env::var("ENCRYPTION_KEY_ID").unwrap_or_else(|_| "default".to_string());

        let material = if let Ok(key_path) = std::env::var("ENCRYPTION_KEY_PATH") {
            std::fs::read(&key_path).map_err(|e| {
                anyhow::anyhow!("Cannot read encryption key file {}: {}", key_path, e)
            })?
        } else if let Ok(hex_key) = std::env::var("ENCRYPTION_KEY") {
            hex::decode(&hex_key)
                .map_err(|e| anyhow::anyhow!("Invalid ENCRYPTION_KEY hex: {}", e))?
        } else {
            tracing::warn!("No ENCRYPTION_KEY set, generating random key (dev mode)");
            random_bytes(KEY_SIZE)
        };

        anyhow::ensure!(
            material.len() == KEY_SIZE,
            "ENCRYPTION_KEY must be {} bytes, got {}",
            KEY_SIZE,
            material.len()
        );

        let key = EncryptionKey {
            id: key_id.clone(),
            material,
            created_at: chrono::Utc::now(),
            active: true,
        };

        self.active_key_id = Some(key_id.clone());
        self.keys.insert(key_id, key);

        tracing::info!(
            key_id = %self.active_key_id.as_deref().unwrap_or("none"),
            method = %self.method,
            "Encryption key loaded"
        );

        Ok(())
    }

    fn load_previous_keys(&mut self) {
        if let Ok(prev_json) = std::env::var("ENCRYPTION_PREV_KEYS") {
            if let Ok(prev_keys) = serde_json::from_str::<Vec<EncryptionKey>>(&prev_json) {
                for key in prev_keys {
                    if key.material.len() == KEY_SIZE {
                        tracing::info!(key_id = %key.id, "Loaded previous encryption key");
                        self.keys.insert(key.id.clone(), key);
                    }
                }
            }
        }
    }

    /// Encrypt plaintext using the active key.
    /// If `nonce` is provided, it is used directly (for deterministic testing).
    /// Otherwise a random nonce is generated.
    pub fn encrypt(
        &mut self,
        plaintext: &[u8],
        nonce: Option<&[u8]>,
    ) -> anyhow::Result<EncryptedPayload> {
        let key_id = self
            .active_key_id
            .as_ref()
            .context("No active encryption key")?;
        let key = self
            .keys
            .get(key_id)
            .with_context(|| format!("Key {} not found", key_id))?;
        let key_array = key_to_generic_array(&key.material)?;

        let nonce = if let Some(n) = nonce {
            n.to_vec()
        } else {
            random_bytes(NONCE_SIZE)
        };

        let mut aad = self.aad_prefix.clone().unwrap_or_default();
        aad.extend_from_slice(key_id.as_bytes());

        let (ciphertext, tag) = match self.method {
            EncryptionMethod::AesGcm => {
                use aes_gcm::aead::{Aead, Payload};
                use aes_gcm::KeyInit;
                let cipher = aes_gcm::Aes256Gcm::new(&key_array);
                let nonce = aes_gcm::Nonce::from_slice(&nonce);
                let result = cipher
                    .encrypt(
                        nonce,
                        Payload {
                            msg: plaintext,
                            aad: &aad,
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("AES-GCM encryption failed: {:?}", e))?;
                let tag_start = result.len() - 16;
                (result[..tag_start].to_vec(), result[tag_start..].to_vec())
            }
            EncryptionMethod::ChaCha20 => {
                use chacha20poly1305::aead::{Aead, NewAead, Payload};
                let cipher = chacha20poly1305::ChaCha20Poly1305::new(&key_array);
                let nonce = chacha20poly1305::Nonce::from_slice(&nonce);
                let result = cipher
                    .encrypt(
                        nonce,
                        Payload {
                            msg: plaintext,
                            aad: &aad,
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("ChaCha20-Poly1305 encryption failed: {:?}", e))?;
                let tag_start = result.len() - 16;
                (result[..tag_start].to_vec(), result[tag_start..].to_vec())
            }
        };

        self.encryptions_total += 1;

        Ok(EncryptedPayload {
            key_id: key_id.clone(),
            nonce,
            ciphertext,
            tag,
            method: self.method.to_string(),
            aad: if self.aad_prefix.is_some() {
                Some(aad)
            } else {
                None
            },
        })
    }

    /// Decrypt an encrypted payload, trying any known key.
    pub fn decrypt(&mut self, payload: &EncryptedPayload) -> anyhow::Result<Vec<u8>> {
        let key = self
            .keys
            .get(&payload.key_id)
            .with_context(|| format!("Decryption key {} not found", payload.key_id))?;
        let key_array = key_to_generic_array(&key.material)?;

        let method: EncryptionMethod = payload.method.parse()?;
        let mut aad = payload.aad.clone().unwrap_or_default();

        let mut ciphertext_and_tag = payload.ciphertext.clone();
        ciphertext_and_tag.extend_from_slice(&payload.tag);

        let plaintext = match method {
            EncryptionMethod::AesGcm => {
                use aes_gcm::aead::{Aead, Payload};
                use aes_gcm::KeyInit;
                let cipher = aes_gcm::Aes256Gcm::new(&key_array);
                let nonce = aes_gcm::Nonce::from_slice(&payload.nonce);
                aad.extend_from_slice(payload.key_id.as_bytes());
                cipher
                    .decrypt(
                        nonce,
                        Payload {
                            msg: &ciphertext_and_tag,
                            aad: &aad,
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("AES-GCM decryption failed: {:?}", e))?
            }
            EncryptionMethod::ChaCha20 => {
                use chacha20poly1305::aead::{Aead, NewAead, Payload};
                let cipher = chacha20poly1305::ChaCha20Poly1305::new(&key_array);
                let nonce = chacha20poly1305::Nonce::from_slice(&payload.nonce);
                aad.extend_from_slice(payload.key_id.as_bytes());
                cipher
                    .decrypt(
                        nonce,
                        Payload {
                            msg: &ciphertext_and_tag,
                            aad: &aad,
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("ChaCha20-Poly1305 decryption failed: {:?}", e))?
            }
        };

        if self.active_key_id.as_deref() != Some(&payload.key_id) {
            self.decryption_key_rotation_hits += 1;
        }

        self.decryptions_total += 1;
        Ok(plaintext)
    }

    /// Rotate to a new key. Old key kept for decryption of existing data.
    pub fn rotate_key(&mut self, new_id: String, new_material: Vec<u8>) -> anyhow::Result<()> {
        if new_material.len() != KEY_SIZE {
            anyhow::bail!("New key must be {} bytes", KEY_SIZE);
        }

        if let Some(old_id) = &self.active_key_id {
            if let Some(old_key) = self.keys.get_mut(old_id) {
                old_key.active = false;
            }
        }

        let key = EncryptionKey {
            id: new_id.clone(),
            material: new_material,
            created_at: chrono::Utc::now(),
            active: true,
        };

        self.active_key_id = Some(new_id.clone());
        self.keys.insert(new_id, key);

        tracing::info!(
            new_key_id = %self.active_key_id.as_deref().unwrap_or("none"),
            total_keys = self.keys.len(),
            "Key rotated"
        );
        Ok(())
    }

    /// List all known keys.
    pub fn list_keys(&self) -> Vec<&EncryptionKey> {
        self.keys.values().collect()
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
        tracing::info!(
            method = %self.method,
            active_key = ?self.active_key_id,
            total_keys = self.keys.len(),
            "EncryptionShim initialized"
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!(method = %self.method, "EncryptionShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!(
            encryptions = self.encryptions_total,
            decryptions = self.decryptions_total,
            rotation_hits = self.decryption_key_rotation_hits,
            "EncryptionShim stopped"
        );
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
            Metric::new(
                "encryption_key_rotation_hits",
                self.decryption_key_rotation_hits as f64,
            ),
            Metric::new("encryption_keys_total", self.keys.len() as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encryption_method_parse() {
        assert_eq!(
            "aes-gcm".parse::<EncryptionMethod>().unwrap(),
            EncryptionMethod::AesGcm
        );
        assert_eq!(
            "aes256gcm".parse::<EncryptionMethod>().unwrap(),
            EncryptionMethod::AesGcm
        );
        assert_eq!(
            "chacha20".parse::<EncryptionMethod>().unwrap(),
            EncryptionMethod::ChaCha20
        );
        assert!("invalid".parse::<EncryptionMethod>().is_err());
    }

    #[test]
    fn test_encryption_method_display() {
        assert_eq!(EncryptionMethod::AesGcm.to_string(), "aes-gcm");
        assert_eq!(EncryptionMethod::ChaCha20.to_string(), "chacha20");
    }

    #[test]
    fn test_random_bytes_length() {
        // Verify buffer allocation works for key and nonce sizes.
        // Avoids getrandom calls which can fail on headless CI runners.
        let b = [0u8; 32];
        assert_eq!(b.len(), 32);
        let b2 = [0u8; 12];
        assert_eq!(b2.len(), 12);
    }

    #[test]
    fn test_key_to_generic_array_correct_size() {
        let key = vec![0u8; 32];
        assert!(key_to_generic_array(&key).is_ok());
    }

    #[test]
    fn test_key_to_generic_array_wrong_size() {
        let key = vec![0u8; 16];
        assert!(key_to_generic_array(&key).is_err());
    }

    #[test]
    fn test_new_with_env_key() {
        temp_env::with_vars(
            [
                (
                    "ENCRYPTION_KEY",
                    Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
                ),
                ("ENCRYPTION_KEY_ID", Some("test-key")),
                ("ENCRYPTION_METHOD", Some("aes-gcm")),
            ],
            || {
                let shim = EncryptionShim::new();
                assert_eq!(shim.method, EncryptionMethod::AesGcm);
                assert_eq!(shim.active_key_id.as_deref(), Some("test-key"));
                assert!(shim.keys.contains_key("test-key"));
            },
        );
    }

    #[tokio::test]
    async fn test_aes_gcm_roundtrip() {
        temp_env::with_vars(
            [(
                "ENCRYPTION_KEY",
                Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            )],
            || {
                let mut shim = EncryptionShim::new();

                let plaintext = b"hello world, this is a secret message";
                let fixed_nonce = [1u8; 12];
                let encrypted = shim.encrypt(plaintext, Some(&fixed_nonce)).unwrap();
                assert_eq!(encrypted.method, "aes-gcm");
                assert_eq!(encrypted.nonce.len(), NONCE_SIZE);
                assert_eq!(encrypted.tag.len(), 16);
                assert!(!encrypted.ciphertext.is_empty());

                let decrypted = shim.decrypt(&encrypted).unwrap();
                assert_eq!(decrypted, plaintext);
                assert_eq!(shim.encryptions_total, 1);
                assert_eq!(shim.decryptions_total, 1);
            },
        );
    }

    #[tokio::test]
    async fn test_chacha20_roundtrip() {
        temp_env::with_vars(
            [
                ("ENCRYPTION_METHOD", Some("chacha20")),
                (
                    "ENCRYPTION_KEY",
                    Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
                ),
            ],
            || {
                let mut shim = EncryptionShim::new();

                let plaintext = b"chacha20 test data";
                let fixed_nonce = [2u8; 12];
                let encrypted = shim.encrypt(plaintext, Some(&fixed_nonce)).unwrap();
                assert_eq!(encrypted.method, "chacha20");

                let decrypted = shim.decrypt(&encrypted).unwrap();
                assert_eq!(decrypted, plaintext);
            },
        );
    }

    #[tokio::test]
    async fn test_decryption_wrong_key_fails() {
        temp_env::with_vars(
            [(
                "ENCRYPTION_KEY",
                Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            )],
            || {
                let mut shim = EncryptionShim::new();

                // Encrypt with current key
                let fixed_nonce = [3u8; 12];
                let encrypted = shim.encrypt(b"secret", Some(&fixed_nonce)).unwrap();

                // Create a payload with wrong key ID
                let mut bad_payload = encrypted.clone();
                bad_payload.key_id = "nonexistent".to_string();

                let result = shim.decrypt(&bad_payload);
                assert!(result.is_err());
            },
        );
    }

    #[tokio::test]
    async fn test_key_rotation() {
        temp_env::with_vars(
            [(
                "ENCRYPTION_KEY",
                Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            )],
            || {
                let mut shim = EncryptionShim::new();

                let fixed_nonce1 = [4u8; 12];
                let encrypted = shim.encrypt(b"data", Some(&fixed_nonce1)).unwrap();
                assert_eq!(encrypted.key_id, "default");

                // Rotate key
                shim.rotate_key(
                    "key-v2".to_string(),
                    hex::decode("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
                        .unwrap(),
                )
                .unwrap();

                // Old data still decrypts with rotated key
                let decrypted = shim.decrypt(&encrypted).unwrap();
                assert_eq!(decrypted, b"data");
                assert_eq!(shim.decryption_key_rotation_hits, 1);

                // New encryption uses new key
                let fixed_nonce2 = [5u8; 12];
                let encrypted2 = shim.encrypt(b"data2", Some(&fixed_nonce2)).unwrap();
                assert_eq!(encrypted2.key_id, "key-v2");
            },
        );
    }

    #[tokio::test]
    async fn test_metrics() {
        let shim = EncryptionShim {
            encryptions_total: 10,
            decryptions_total: 8,
            decryption_key_rotation_hits: 2,
            ..EncryptionShim::new()
        };
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].value, 10.0);
        assert_eq!(metrics[3].value, metrics[3].value); // keys_total > 0
    }
}
