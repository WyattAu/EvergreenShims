#![allow(dead_code)]
//! Auth shim — authentication/authorization layer.
//!
//! Provides authentication and authorization for database connections.
//!
//! ## Environment Variables
//!
//! ```text
//! AUTH_METHOD            Method: password, certificate, ldap, oauth (default: password)
//! AUTH_LDAP_URL          LDAP server URL
//! AUTH_LDAP_BASE         LDAP search base
//! AUTH_OAUTH_ISSUER      OAuth2 issuer URL
//! AUTH_OAUTH_AUDIENCE    OAuth2 audience
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// Auth shim.
pub struct AuthShim {
    method: String,
    ldap_url: Option<String>,
    ldap_base: Option<String>,
    oauth_issuer: Option<String>,
    oauth_audience: Option<String>,
    auth_success: u64,
    auth_failure: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl AuthShim {
    pub fn new() -> Self {
        Self {
            method: std::env::var("AUTH_METHOD").unwrap_or_else(|_| "password".to_string()),
            ldap_url: std::env::var("AUTH_LDAP_URL").ok(),
            ldap_base: std::env::var("AUTH_LDAP_BASE").ok(),
            oauth_issuer: std::env::var("AUTH_OAUTH_ISSUER").ok(),
            oauth_audience: std::env::var("AUTH_OAUTH_AUDIENCE").ok(),
            auth_success: 0,
            auth_failure: 0,
            shutdown_tx: None,
        }
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
        ]
    }
}
