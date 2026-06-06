#![allow(dead_code)]
//! DynamoDB shim — health checks, backup, and cost tracking.
//!
//! ## Environment Variables
//!
//! ```text
//! DYNAMODB_REGION        AWS region (default: us-east-1)
//! DYNAMODB_ENDPOINT      Custom endpoint (for LocalStack)
//! DYNAMODB_TABLE         Table name to monitor
//! DYNAMODB_BACKUP_TABLE  Table name for backup exports
//! ```

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// DynamoDB table info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamoTableInfo {
    pub table_name: String,
    pub item_count: u64,
    pub table_size_bytes: u64,
    pub status: String,
    pub billing_mode: String,
    pub read_capacity: i64,
    pub write_capacity: i64,
}

/// DynamoDB health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamoHealth {
    pub ok: bool,
    pub table_count: u32,
    pub tables: Vec<String>,
}

/// DynamoDB shim.
pub struct DynamoShim {
    region: String,
    endpoint: Option<String>,
    table: String,
    health_checks: u64,
    backup_exports: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl DynamoShim {
    pub fn new() -> Self {
        Self {
            region: std::env::var("DYNAMODB_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
            endpoint: std::env::var("DYNAMODB_ENDPOINT").ok(),
            table: std::env::var("DYNAMODB_TABLE").unwrap_or_default(),
            health_checks: 0,
            backup_exports: 0,
            shutdown_tx: None,
        }
    }

    /// Build a DynamoDB client.
    async fn build_client(&self) -> anyhow::Result<aws_sdk_dynamodb::Client> {
        let mut config_loader =
            aws_config::from_env().region(aws_config::Region::new(self.region.clone()));
        if let Some(ref endpoint) = self.endpoint {
            config_loader = config_loader.endpoint_url(endpoint);
        }
        let sdk_config = config_loader.load().await;
        Ok(aws_sdk_dynamodb::Client::new(&sdk_config))
    }

    /// Check DynamoDB health.
    pub async fn check_health(&mut self) -> anyhow::Result<DynamoHealth> {
        self.health_checks += 1;
        let client = self.build_client().await?;

        let resp = client.list_tables().limit(100).send().await?;
        let table_names: Vec<String> = resp.table_names().to_vec();
        let table_count = table_names.len() as u32;
        let ok = table_count > 0;

        Ok(DynamoHealth {
            ok,
            table_count,
            tables: table_names,
        })
    }

    /// Get table info.
    pub async fn describe_table(&self, table_name: &str) -> anyhow::Result<DynamoTableInfo> {
        let client = self.build_client().await?;
        let resp = client
            .describe_table()
            .table_name(table_name)
            .send()
            .await?;

        let table = resp
            .table()
            .ok_or_else(|| anyhow::anyhow!("No table in response"))?;

        Ok(DynamoTableInfo {
            table_name: table_name.to_string(),
            item_count: table.item_count().unwrap_or(0) as u64,
            table_size_bytes: table.table_size_bytes().unwrap_or(0) as u64,
            status: table
                .table_status()
                .map(|s| format!("{:?}", s))
                .unwrap_or_else(|| "Unknown".to_string()),
            billing_mode: table
                .billing_mode_summary()
                .and_then(|b| b.billing_mode().map(|m| format!("{:?}", m)))
                .unwrap_or_else(|| "PayPerRequest".to_string()),
            read_capacity: table
                .provisioned_throughput()
                .map(|p| p.read_capacity_units().unwrap_or(0))
                .unwrap_or(0),
            write_capacity: table
                .provisioned_throughput()
                .map(|p| p.write_capacity_units().unwrap_or(0))
                .unwrap_or(0),
        })
    }

    pub fn region(&self) -> &str {
        &self.region
    }
    pub fn table(&self) -> &str {
        &self.table
    }
}

impl Default for DynamoShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for DynamoShim {
    fn name(&self) -> &str {
        "dynamodb"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!("DynamoShim initialized (region={})", self.region);
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("DynamoShim started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("DynamoShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("dynamo_health_checks_total", self.health_checks as f64),
            Metric::new("dynamo_backup_exports_total", self.backup_exports as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dynamo_defaults() {
        temp_env::with_vars(
            [
                ("DYNAMODB_REGION", None::<&str>),
                ("DYNAMODB_TABLE", None::<&str>),
            ],
            || {
                let shim = DynamoShim::new();
                assert_eq!(shim.region(), "us-east-1");
                assert_eq!(shim.table(), "");
            },
        );
    }

    #[test]
    fn test_dynamo_env_overrides() {
        temp_env::with_vars(
            [
                ("DYNAMODB_REGION", Some("eu-west-1")),
                ("DYNAMODB_TABLE", Some("my-table")),
            ],
            || {
                let shim = DynamoShim::new();
                assert_eq!(shim.region(), "eu-west-1");
                assert_eq!(shim.table(), "my-table");
            },
        );
    }

    #[test]
    fn test_dynamo_metrics() {
        let shim = DynamoShim::new();
        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 2);
    }

    #[test]
    fn test_dynamo_capability() {
        let shim = DynamoShim::new();
        assert_eq!(shim.name(), "dynamodb");
    }
}
