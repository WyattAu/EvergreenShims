//! Auth shim — authentication/authorization layer.
//!
//! Provides authentication and authorization for database connections
//! with token validation, API key management, and role-based access.
//!
//! ## Environment Variables
//!
//! ```text
//! AUTH_METHOD            Method: password, certificate, ldap, oauth (default: password)
//! AUTH_LDAP_URL          LDAP server URL
//! AUTH_LDAP_BASE         LDAP search base
//! AUTH_OAUTH_ISSUER      OAuth2 issuer URL
//! AUTH_OAUTH_AUDIENCE    OAuth2 audience
//! AUTH_TOKEN_EXPIRY_SECS Token expiry in seconds (default: 3600)
//! AUTH_MAX_FAILED_LOGINS Max failed login attempts before lockout (default: 5)
//! AUTH_LOCKOUT_SECS      Lockout duration in seconds (default: 300)
//! AUTH_HMAC_KEY          HMAC signing key (hex-encoded). If unset, a random key is generated.
//! ```

use std::collections::HashMap;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// User roles for role-based access control.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Role {
    Admin,
    ReadWrite,
    ReadOnly,
    Denied,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admin => write!(f, "admin"),
            Self::ReadWrite => write!(f, "readwrite"),
            Self::ReadOnly => write!(f, "readonly"),
            Self::Denied => write!(f, "denied"),
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "admin" => Ok(Self::Admin),
            "readwrite" | "rw" => Ok(Self::ReadWrite),
            "readonly" | "ro" => Ok(Self::ReadOnly),
            "denied" => Ok(Self::Denied),
            _ => Err(format!("Unknown role: {}", s)),
        }
    }
}

/// A token with metadata for validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub token_id: String,
    pub user: String,
    pub role: Role,
    pub issued_at: String,
    pub expires_at: String,
    pub source_ip: Option<String>,
}

/// An API key with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub key_id: String,
    pub name: String,
    pub key_hash: String,
    pub role: Role,
    pub created_at: String,
    pub last_used: Option<String>,
    pub revoked: bool,
}

/// Authentication result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResult {
    pub authenticated: bool,
    pub user: Option<String>,
    pub role: Option<Role>,
    pub reason: Option<String>,
}

/// Internal storage for a token with its HMAC verification hash.
#[derive(Debug, Clone)]
struct StoredToken {
    token_hash: String,
    user: String,
    role: Role,
    #[allow(dead_code)]
    issued_at: String,
    expires_at: String,
    #[allow(dead_code)]
    source_ip: Option<String>,
}

/// Auth shim.
pub struct AuthShim {
    method: String,
    #[allow(dead_code)]
    ldap_url: Option<String>,
    #[allow(dead_code)]
    ldap_base: Option<String>,
    #[allow(dead_code)]
    oauth_issuer: Option<String>,
    #[allow(dead_code)]
    oauth_audience: Option<String>,
    token_expiry_secs: u64,
    max_failed_logins: u32,
    lockout_secs: u64,
    auth_success: u64,
    auth_failure: u64,
    tokens: HashMap<String, StoredToken>,
    api_keys: HashMap<String, ApiKey>,
    failed_attempts: HashMap<String, u32>,
    locked_users: HashMap<String, chrono::DateTime<chrono::Utc>>,
    hmac_key: Vec<u8>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl AuthShim {
    pub fn new() -> Self {
        let hmac_key = std::env::var("AUTH_HMAC_KEY")
            .ok()
            .and_then(|k| hex_decode(&k))
            .unwrap_or_else(|| Uuid::new_v4().as_bytes().to_vec());
        Self {
            method: std::env::var("AUTH_METHOD").unwrap_or_else(|_| "password".to_string()),
            ldap_url: std::env::var("AUTH_LDAP_URL").ok(),
            ldap_base: std::env::var("AUTH_LDAP_BASE").ok(),
            oauth_issuer: std::env::var("AUTH_OAUTH_ISSUER").ok(),
            oauth_audience: std::env::var("AUTH_OAUTH_AUDIENCE").ok(),
            token_expiry_secs: std::env::var("AUTH_TOKEN_EXPIRY_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3600),
            max_failed_logins: std::env::var("AUTH_MAX_FAILED_LOGINS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            lockout_secs: std::env::var("AUTH_LOCKOUT_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            auth_success: 0,
            auth_failure: 0,
            tokens: HashMap::new(),
            api_keys: HashMap::new(),
            failed_attempts: HashMap::new(),
            locked_users: HashMap::new(),
            hmac_key,
            shutdown_tx: None,
        }
    }

    fn hmac_hash(&self, data: &[u8]) -> String {
        let mut mac =
            HmacSha256::new_from_slice(&self.hmac_key).expect("HMAC can take key of any size");
        mac.update(data);
        hex_encode(&mac.finalize().into_bytes())
    }

    /// Generate a new token for a user. Returns the token value (`{id}.{secret}`).
    pub fn create_token(&mut self, user: &str, role: Role, source_ip: Option<&str>) -> String {
        let id = Uuid::new_v4().to_string();
        let secret = Uuid::new_v4().to_string();
        let token_value = format!("{}.{}", id, secret);
        let now = chrono::Utc::now();
        let token_hash = self.hmac_hash(token_value.as_bytes());
        let stored = StoredToken {
            token_hash,
            user: user.to_string(),
            role,
            issued_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::seconds(self.token_expiry_secs as i64))
                .to_rfc3339(),
            source_ip: source_ip.map(|s| s.to_string()),
        };
        self.tokens.insert(id, stored);
        token_value
    }

    /// Validate a token. Returns AuthResult with the outcome.
    pub fn validate_token(&mut self, token: &str) -> AuthResult {
        let (id, _rest) = match token.split_once('.') {
            Some(parts) => parts,
            None => {
                self.auth_failure += 1;
                return AuthResult {
                    authenticated: false,
                    user: None,
                    role: None,
                    reason: Some("Invalid token format".to_string()),
                };
            }
        };

        let stored = match self.tokens.get(id) {
            Some(t) => t.clone(),
            None => {
                self.auth_failure += 1;
                return AuthResult {
                    authenticated: false,
                    user: None,
                    role: None,
                    reason: Some("Token not found".to_string()),
                };
            }
        };

        let expected_hash = match hex_decode(&stored.token_hash) {
            Some(h) => h,
            None => {
                self.auth_failure += 1;
                return AuthResult {
                    authenticated: false,
                    user: None,
                    role: None,
                    reason: Some("Internal error".to_string()),
                };
            }
        };
        let mut mac = match HmacSha256::new_from_slice(&self.hmac_key) {
            Ok(m) => m,
            Err(_) => {
                self.auth_failure += 1;
                return AuthResult {
                    authenticated: false,
                    user: None,
                    role: None,
                    reason: Some("Internal error".to_string()),
                };
            }
        };
        mac.update(token.as_bytes());
        if mac.verify_slice(&expected_hash).is_err() {
            self.auth_failure += 1;
            return AuthResult {
                authenticated: false,
                user: None,
                role: None,
                reason: Some("Token verification failed".to_string()),
            };
        }

        let now = chrono::Utc::now();
        let expires = stored
            .expires_at
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap_or(now);
        if now > expires {
            self.auth_failure += 1;
            return AuthResult {
                authenticated: false,
                user: Some(stored.user.clone()),
                role: None,
                reason: Some("Token expired".to_string()),
            };
        }

        if stored.role == Role::Denied {
            self.auth_failure += 1;
            return AuthResult {
                authenticated: false,
                user: Some(stored.user),
                role: Some(Role::Denied),
                reason: Some("User role is denied".to_string()),
            };
        }

        self.auth_success += 1;
        AuthResult {
            authenticated: true,
            user: Some(stored.user),
            role: Some(stored.role),
            reason: None,
        }
    }

    /// Revoke a token by its ID prefix.
    pub fn revoke_token(&mut self, token_id: &str) -> bool {
        self.tokens.remove(token_id).is_some()
    }

    /// Register an API key.
    pub fn register_api_key(&mut self, key: ApiKey) {
        self.api_keys.insert(key.key_id.clone(), key);
    }

    /// Hash an API key for storage using HMAC-SHA256.
    pub fn hash_api_key(&self, key: &str) -> String {
        self.hmac_hash(key.as_bytes())
    }

    /// Validate an API key by ID and provided key value using constant-time HMAC comparison.
    pub fn validate_api_key(&mut self, key_id: &str, provided_key: &str) -> AuthResult {
        let key = match self.api_keys.get(key_id) {
            Some(k) => k.clone(),
            None => {
                self.auth_failure += 1;
                return AuthResult {
                    authenticated: false,
                    user: None,
                    role: None,
                    reason: Some("API key not found".to_string()),
                };
            }
        };

        if key.revoked {
            self.auth_failure += 1;
            return AuthResult {
                authenticated: false,
                user: None,
                role: None,
                reason: Some("API key revoked".to_string()),
            };
        }

        let expected_hash = match hex_decode(&key.key_hash) {
            Some(h) => h,
            None => {
                self.auth_failure += 1;
                return AuthResult {
                    authenticated: false,
                    user: None,
                    role: None,
                    reason: Some("Invalid key hash".to_string()),
                };
            }
        };
        let mut mac = match HmacSha256::new_from_slice(&self.hmac_key) {
            Ok(m) => m,
            Err(_) => {
                self.auth_failure += 1;
                return AuthResult {
                    authenticated: false,
                    user: None,
                    role: None,
                    reason: Some("Internal error".to_string()),
                };
            }
        };
        mac.update(provided_key.as_bytes());
        if mac.verify_slice(&expected_hash).is_err() {
            self.auth_failure += 1;
            return AuthResult {
                authenticated: false,
                user: None,
                role: None,
                reason: Some("API key verification failed".to_string()),
            };
        }

        self.auth_success += 1;
        AuthResult {
            authenticated: true,
            user: Some(key.name),
            role: Some(key.role),
            reason: None,
        }
    }

    /// Revoke an API key.
    pub fn revoke_api_key(&mut self, key_id: &str) -> bool {
        if let Some(key) = self.api_keys.get_mut(key_id) {
            key.revoked = true;
            true
        } else {
            false
        }
    }

    /// Hash a password with a random salt using HMAC-SHA256.
    /// Returns `{salt_hex}:{hash_hex}`.
    pub fn hash_password(password: &str) -> String {
        let salt = Uuid::new_v4().as_bytes().to_vec();
        let mut mac = HmacSha256::new_from_slice(&salt).expect("HMAC can take key of any size");
        mac.update(password.as_bytes());
        let result = mac.finalize();
        format!("{}:{}", hex_encode(&salt), hex_encode(&result.into_bytes()))
    }

    /// Verify a password against a stored hash using constant-time comparison.
    pub fn verify_password(password: &str, stored: &str) -> bool {
        let parts: Vec<&str> = stored.splitn(2, ':').collect();
        if parts.len() != 2 {
            return false;
        }
        let salt = match hex_decode(parts[0]) {
            Some(s) => s,
            None => return false,
        };
        let expected_bytes = match hex_decode(parts[1]) {
            Some(h) => h,
            None => return false,
        };
        let mut mac = match HmacSha256::new_from_slice(&salt) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(password.as_bytes());
        mac.verify_slice(&expected_bytes).is_ok()
    }

    /// Record a failed login attempt for a user. Returns true if user is now locked out.
    pub fn record_failed_login(&mut self, user: &str) -> bool {
        let attempts = self.failed_attempts.entry(user.to_string()).or_insert(0);
        *attempts += 1;

        if *attempts >= self.max_failed_logins {
            self.locked_users
                .insert(user.to_string(), chrono::Utc::now());
            return true;
        }
        false
    }

    /// Check if a user is locked out and whether the lockout has expired.
    pub fn is_locked_out(&self, user: &str) -> bool {
        if let Some(locked_at) = self.locked_users.get(user) {
            let elapsed = chrono::Utc::now() - *locked_at;
            elapsed.num_seconds() < self.lockout_secs as i64
        } else {
            false
        }
    }

    /// Clear failed attempts for a user (on successful login).
    pub fn clear_failed_attempts(&mut self, user: &str) {
        self.failed_attempts.remove(user);
        self.locked_users.remove(user);
    }

    /// Check role permissions.
    pub fn check_permission(&self, role: &Role, required: &Role) -> bool {
        matches!(
            (role, required),
            (Role::Admin, _)
                | (Role::ReadWrite, Role::ReadWrite | Role::ReadOnly)
                | (Role::ReadOnly, Role::ReadOnly)
        )
    }

    /// Get failed attempt count for a user.
    pub fn failed_count(&self, user: &str) -> u32 {
        self.failed_attempts.get(user).copied().unwrap_or(0)
    }
}

impl Default for AuthShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for AuthShim {
    fn name(&self) -> &str {
        "auth"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("AuthShim initialized (method={})", self.method);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("AuthShim started (method={})", self.method);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("AuthShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("auth_success_total", self.auth_success as f64),
            Metric::new("auth_failure_total", self.auth_failure as f64),
            Metric::new("auth_active_tokens", self.tokens.len() as f64),
            Metric::new("auth_api_keys_total", self.api_keys.len() as f64),
            Metric::new("auth_locked_users", self.locked_users.len() as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_parse() {
        assert_eq!("admin".parse::<Role>().unwrap(), Role::Admin);
        assert_eq!("readwrite".parse::<Role>().unwrap(), Role::ReadWrite);
        assert_eq!("rw".parse::<Role>().unwrap(), Role::ReadWrite);
        assert_eq!("readonly".parse::<Role>().unwrap(), Role::ReadOnly);
        assert_eq!("ro".parse::<Role>().unwrap(), Role::ReadOnly);
        assert_eq!("denied".parse::<Role>().unwrap(), Role::Denied);
        assert!("invalid".parse::<Role>().is_err());
    }

    #[test]
    fn test_role_display() {
        assert_eq!(Role::Admin.to_string(), "admin");
        assert_eq!(Role::ReadOnly.to_string(), "readonly");
    }

    #[test]
    fn test_create_and_validate_token() {
        let mut shim = AuthShim::new();
        let token_value = shim.create_token("alice", Role::Admin, Some("10.0.0.1"));

        let result = shim.validate_token(&token_value);
        assert!(result.authenticated);
        assert_eq!(result.user.as_deref(), Some("alice"));
        assert_eq!(result.role, Some(Role::Admin));
        assert!(result.reason.is_none());
    }

    #[test]
    fn test_validate_nonexistent_token() {
        let mut shim = AuthShim::new();
        let result = shim.validate_token("nonexistent");
        assert!(!result.authenticated);
        assert_eq!(shim.auth_failure, 1);
    }

    #[test]
    fn test_validate_token_wrong_id() {
        let mut shim = AuthShim::new();
        let fake_token = format!("{}.{}", Uuid::new_v4(), Uuid::new_v4());
        let result = shim.validate_token(&fake_token);
        assert!(!result.authenticated);
        assert_eq!(shim.auth_failure, 1);
    }

    #[test]
    fn test_validate_token_tampered() {
        let mut shim = AuthShim::new();
        let token_value = shim.create_token("alice", Role::Admin, None);
        let parts: Vec<&str> = token_value.split('.').collect();
        let tampered = format!("{}.tampered_{}", parts[0], parts[1]);
        let result = shim.validate_token(&tampered);
        assert!(!result.authenticated);
        assert!(result
            .reason
            .as_deref()
            .unwrap()
            .contains("verification failed"));
    }

    #[test]
    fn test_validate_expired_token() {
        let mut shim = AuthShim::new();
        let token_value = shim.create_token("bob", Role::ReadWrite, None);
        let token_id = token_value.split('.').next().unwrap().to_string();
        if let Some(stored) = shim.tokens.get_mut(&token_id) {
            stored.expires_at = "2020-01-01T00:00:00Z".to_string();
        }

        let result = shim.validate_token(&token_value);
        assert!(!result.authenticated);
        assert!(result.reason.as_ref().unwrap().contains("expired"));
    }

    #[test]
    fn test_validate_denied_role() {
        let mut shim = AuthShim::new();
        let token_value = shim.create_token("charlie", Role::Denied, None);

        let result = shim.validate_token(&token_value);
        assert!(!result.authenticated);
        assert!(result.reason.as_ref().unwrap().contains("denied"));
    }

    #[test]
    fn test_revoke_token() {
        let mut shim = AuthShim::new();
        let token_value = shim.create_token("alice", Role::Admin, None);
        let token_id = token_value.split('.').next().unwrap().to_string();
        assert!(shim.revoke_token(&token_id));

        let result = shim.validate_token(&token_value);
        assert!(!result.authenticated);
    }

    #[test]
    fn test_register_and_validate_api_key() {
        let mut shim = AuthShim::new();
        let raw_key = "my-secret-api-key";
        let key_hash = shim.hash_api_key(raw_key);
        let key = ApiKey {
            key_id: "key-1".to_string(),
            name: "service-a".to_string(),
            key_hash,
            role: Role::ReadWrite,
            created_at: chrono::Utc::now().to_rfc3339(),
            last_used: None,
            revoked: false,
        };
        shim.register_api_key(key);

        let result = shim.validate_api_key("key-1", raw_key);
        assert!(result.authenticated);
        assert_eq!(result.role, Some(Role::ReadWrite));
    }

    #[test]
    fn test_validate_api_key_wrong_key() {
        let mut shim = AuthShim::new();
        let key_hash = shim.hash_api_key("correct-key");
        let key = ApiKey {
            key_id: "key-wrong".to_string(),
            name: "service-a".to_string(),
            key_hash,
            role: Role::ReadWrite,
            created_at: chrono::Utc::now().to_rfc3339(),
            last_used: None,
            revoked: false,
        };
        shim.register_api_key(key);

        let result = shim.validate_api_key("key-wrong", "wrong-key");
        assert!(!result.authenticated);
        assert!(result
            .reason
            .as_ref()
            .unwrap()
            .contains("verification failed"));
    }

    #[test]
    fn test_revoked_api_key() {
        let mut shim = AuthShim::new();
        let raw_key = "my-secret-api-key";
        let key_hash = shim.hash_api_key(raw_key);
        let key = ApiKey {
            key_id: "key-2".to_string(),
            name: "service-b".to_string(),
            key_hash,
            role: Role::Admin,
            created_at: chrono::Utc::now().to_rfc3339(),
            last_used: None,
            revoked: false,
        };
        shim.register_api_key(key);
        shim.revoke_api_key("key-2");

        let result = shim.validate_api_key("key-2", raw_key);
        assert!(!result.authenticated);
    }

    #[test]
    fn test_failed_login_lockout() {
        let mut shim = AuthShim {
            max_failed_logins: 3,
            lockout_secs: 300,
            ..AuthShim::new()
        };

        assert!(!shim.record_failed_login("alice"));
        assert!(!shim.record_failed_login("alice"));
        assert!(shim.record_failed_login("alice"));
        assert!(shim.is_locked_out("alice"));
    }

    #[test]
    fn test_clear_failed_attempts() {
        let mut shim = AuthShim {
            max_failed_logins: 5,
            ..AuthShim::new()
        };
        shim.record_failed_login("alice");
        shim.record_failed_login("alice");
        assert_eq!(shim.failed_count("alice"), 2);

        shim.clear_failed_attempts("alice");
        assert_eq!(shim.failed_count("alice"), 0);
    }

    #[test]
    fn test_check_permission_admin() {
        let shim = AuthShim::new();
        assert!(shim.check_permission(&Role::Admin, &Role::Admin));
        assert!(shim.check_permission(&Role::Admin, &Role::ReadWrite));
        assert!(shim.check_permission(&Role::Admin, &Role::ReadOnly));
    }

    #[test]
    fn test_check_permission_readwrite() {
        let shim = AuthShim::new();
        assert!(!shim.check_permission(&Role::ReadWrite, &Role::Admin));
        assert!(shim.check_permission(&Role::ReadWrite, &Role::ReadWrite));
        assert!(shim.check_permission(&Role::ReadWrite, &Role::ReadOnly));
    }

    #[test]
    fn test_check_permission_readonly() {
        let shim = AuthShim::new();
        assert!(!shim.check_permission(&Role::ReadOnly, &Role::Admin));
        assert!(!shim.check_permission(&Role::ReadOnly, &Role::ReadWrite));
        assert!(shim.check_permission(&Role::ReadOnly, &Role::ReadOnly));
    }

    #[test]
    fn test_hash_and_verify_password() {
        let password = "super_secret_123";
        let stored = AuthShim::hash_password(password);
        assert!(AuthShim::verify_password(password, &stored));
        assert!(!AuthShim::verify_password("wrong_password", &stored));
    }

    #[test]
    fn test_password_hash_format() {
        let stored = AuthShim::hash_password("test");
        let parts: Vec<&str> = stored.split(':').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 32);
        assert_eq!(parts[1].len(), 64);
    }

    #[test]
    fn test_password_hash_different_each_time() {
        let h1 = AuthShim::hash_password("same_password");
        let h2 = AuthShim::hash_password("same_password");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_verify_password_invalid_format() {
        assert!(!AuthShim::verify_password("pw", "invalid"));
        assert!(!AuthShim::verify_password("pw", "zz:zz"));
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = AuthShim::new();
        shim.create_token("alice", Role::Admin, None);
        shim.create_token("bob", Role::ReadOnly, None);
        shim.auth_success = 10;
        shim.auth_failure = 3;

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 5);
        assert_eq!(metrics[0].value, 10.0);
        assert_eq!(metrics[1].value, 3.0);
        assert_eq!(metrics[2].value, 2.0);
    }
}
