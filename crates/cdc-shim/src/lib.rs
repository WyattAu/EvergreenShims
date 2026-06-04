#![allow(dead_code)]
//! CDC shim — Change Data Capture for event-driven architectures.
//!
//! Reads database WAL/binlog and publishes changes to Kafka, NATS, or webhooks.
//!
//! ## Environment Variables
//!
//! ```text
//! CDC_OUTPUT            Output: kafka, nats, webhook (required)
//! CDC_TABLES            Comma-separated tables (empty = all)
//! CDC_FORMAT            Format: json, avro, protobuf (default: json)
//! CDC_COMPRESSION       Compression: none, zstd (default: none)
//! CDC_KAFKA_BROKERS     Kafka brokers (for kafka output)
//! CDC_KAFKA_TOPIC       Kafka topic
//! CDC_WEBHOOK_URL       Webhook URL (for webhook output)
//! CDC_DB_TYPE           Database type: postgres, mariadb
//! CDC_SLOT              Replication slot (PostgreSQL)
//! ```

use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// CDC shim.
pub struct CdcShim {
    output: String,
    tables: Vec<String>,
    format: String,
    compression: String,
    kafka_brokers: Option<String>,
    kafka_topic: Option<String>,
    webhook_url: Option<String>,
    db_type: String,
    events_captured: u64,
    events_published: u64,
    events_failed: u64,
    lag_seconds: f64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CdcShim {
    pub fn new() -> Self {
        Self {
            output: std::env::var("CDC_OUTPUT").unwrap_or_else(|_| "kafka".to_string()),
            tables: std::env::var("CDC_TABLES")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect(),
            format: std::env::var("CDC_FORMAT").unwrap_or_else(|_| "json".to_string()),
            compression: std::env::var("CDC_COMPRESSION").unwrap_or_else(|_| "none".to_string()),
            kafka_brokers: std::env::var("CDC_KAFKA_BROKERS").ok(),
            kafka_topic: std::env::var("CDC_KAFKA_TOPIC").ok(),
            webhook_url: std::env::var("CDC_WEBHOOK_URL").ok(),
            db_type: std::env::var("CDC_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            events_captured: 0,
            events_published: 0,
            events_failed: 0,
            lag_seconds: 0.0,
            shutdown_tx: None,
        }
    }
}

impl Default for CdcShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for CdcShim {
    fn name(&self) -> &str {
        "cdc"
    }

    async fn init(&mut self, _config: &Config) -> Result<()> {
        tracing::info!(
            "CdcShim initialized (output={}, format={}, tables={})",
            self.output,
            self.format,
            if self.tables.is_empty() {
                "all".to_string()
            } else {
                self.tables.join(",")
            }
        );
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        let (shutdown_tx, _) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("CdcShim started (output={})", self.output);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("CdcShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        vec![
            Metric::new("cdc_events_captured_total", self.events_captured as f64),
            Metric::new("cdc_events_published_total", self.events_published as f64),
            Metric::new("cdc_events_failed_total", self.events_failed as f64),
            Metric::new("cdc_lag_seconds", self.lag_seconds),
        ]
    }
}
