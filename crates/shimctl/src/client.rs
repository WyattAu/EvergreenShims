use anyhow::{Context, Result};
use reqwest::Client;
use serde::Serialize;

pub struct ManagementClient {
    client: Client,
    endpoint: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct StatusResponse {
    pub status: String,
    pub version: String,
    pub uptime: String,
    pub healthy: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct MetricsResponse {
    pub metrics: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReloadResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct BackupEntry {
    pub id: String,
    pub created_at: String,
    pub size_bytes: u64,
    pub status: String,
}

#[derive(Debug, Serialize, serde::Deserialize)]
pub struct BackupListResponse {
    pub backups: Vec<BackupEntry>,
}

#[derive(Debug, serde::Deserialize)]
pub struct BackupTriggerResponse {
    pub backup_id: String,
    pub status: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct MigrationStatusResponse {
    pub current_version: String,
    pub pending: Vec<String>,
    pub applied: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MigrationApplyResponse {
    pub success: bool,
    pub applied: Vec<String>,
    pub message: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct MigrationRollbackResponse {
    pub success: bool,
    pub rolled_back: String,
    pub message: String,
}

impl ManagementClient {
    pub fn new(endpoint: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            endpoint: endpoint.trim_end_matches('/').to_string(),
        })
    }

    pub async fn get_status(&self) -> Result<StatusResponse> {
        let url = format!("{}/status", self.endpoint);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse status response")?;
        Ok(resp)
    }

    pub async fn get_metrics(&self) -> Result<MetricsResponse> {
        let url = format!("{}/metrics", self.endpoint);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse metrics response")?;
        Ok(resp)
    }

    pub async fn get_livez(&self) -> Result<HealthResponse> {
        let url = format!("{}/livez", self.endpoint);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse liveness response")?;
        Ok(resp)
    }

    pub async fn get_readyz(&self) -> Result<HealthResponse> {
        let url = format!("{}/readyz", self.endpoint);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse readiness response")?;
        Ok(resp)
    }

    pub async fn reload_config(&self) -> Result<ReloadResponse> {
        let url = format!("{}/reload", self.endpoint);
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse reload response")?;
        Ok(resp)
    }

    pub async fn validate_config(&self, config_path: &str) -> Result<ValidateResponse> {
        let url = format!("{}/validate", self.endpoint);
        let body = serde_json::json!({ "config_path": config_path });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse validate response")?;
        Ok(resp)
    }

    pub async fn list_backups(&self) -> Result<Vec<BackupEntry>> {
        let url = format!("{}/backups", self.endpoint);
        let resp: BackupListResponse = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse backup list response")?;
        Ok(resp.backups)
    }

    pub async fn trigger_backup(&self) -> Result<BackupTriggerResponse> {
        let url = format!("{}/backups", self.endpoint);
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse backup trigger response")?;
        Ok(resp)
    }

    pub async fn get_migration_status(&self) -> Result<MigrationStatusResponse> {
        let url = format!("{}/migrations", self.endpoint);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse migration status response")?;
        Ok(resp)
    }

    pub async fn apply_migrations(&self) -> Result<MigrationApplyResponse> {
        let url = format!("{}/migrations/apply", self.endpoint);
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse migration apply response")?;
        Ok(resp)
    }

    pub async fn rollback_migration(&self) -> Result<MigrationRollbackResponse> {
        let url = format!("{}/migrations/rollback", self.endpoint);
        let resp = self
            .client
            .post(&url)
            .send()
            .await
            .context("Failed to connect to management API")?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse migration rollback response")?;
        Ok(resp)
    }
}
