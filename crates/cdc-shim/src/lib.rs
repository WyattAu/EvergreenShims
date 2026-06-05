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
//! CDC_BATCH_SIZE        Batch size for publishing (default: 100)
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// CDC event operation types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CdcOperation {
    Insert,
    Update,
    Delete,
}

impl std::fmt::Display for CdcOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Insert => write!(f, "insert"),
            Self::Update => write!(f, "update"),
            Self::Delete => write!(f, "delete"),
        }
    }
}

impl std::str::FromStr for CdcOperation {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "insert" | "i" => Ok(Self::Insert),
            "update" | "u" => Ok(Self::Update),
            "delete" | "d" => Ok(Self::Delete),
            _ => Err(format!("Unknown CDC operation: {}", s)),
        }
    }
}

/// A captured change event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdcEvent {
    pub event_id: String,
    pub lsn: String,
    pub timestamp: String,
    pub table: String,
    pub operation: CdcOperation,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub published: bool,
}

/// WAL position tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalPosition {
    pub lsn: String,
    pub segment: u64,
    pub offset: u64,
    pub last_flush: String,
}

/// CDC statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdcStats {
    pub events_captured: u64,
    pub events_published: u64,
    pub events_failed: u64,
    pub events_by_table: HashMap<String, u64>,
    pub events_by_operation: HashMap<String, u64>,
    pub lag_seconds: f64,
}

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
    batch_size: usize,
    events_captured: u64,
    events_published: u64,
    events_failed: u64,
    lag_seconds: f64,
    wal_position: WalPosition,
    pending_events: Vec<CdcEvent>,
    event_counter: u64,
    table_filter_enabled: bool,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl CdcShim {
    pub fn new() -> Self {
        let tables: Vec<String> = std::env::var("CDC_TABLES")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_string())
            .collect();

        Self {
            output: std::env::var("CDC_OUTPUT").unwrap_or_else(|_| "kafka".to_string()),
            tables: tables.clone(),
            format: std::env::var("CDC_FORMAT").unwrap_or_else(|_| "json".to_string()),
            compression: std::env::var("CDC_COMPRESSION").unwrap_or_else(|_| "none".to_string()),
            kafka_brokers: std::env::var("CDC_KAFKA_BROKERS").ok(),
            kafka_topic: std::env::var("CDC_KAFKA_TOPIC").ok(),
            webhook_url: std::env::var("CDC_WEBHOOK_URL").ok(),
            db_type: std::env::var("CDC_DB_TYPE").unwrap_or_else(|_| "postgres".to_string()),
            batch_size: std::env::var("CDC_BATCH_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
            events_captured: 0,
            events_published: 0,
            events_failed: 0,
            lag_seconds: 0.0,
            wal_position: WalPosition {
                lsn: "0/0".to_string(),
                segment: 0,
                offset: 0,
                last_flush: chrono::Utc::now().to_rfc3339(),
            },
            pending_events: Vec::new(),
            event_counter: 0,
            table_filter_enabled: !tables.is_empty(),
            shutdown_tx: None,
        }
    }

    /// Check if a table should be captured.
    pub fn should_capture(&self, table: &str) -> bool {
        !self.table_filter_enabled || self.tables.iter().any(|t| t.eq_ignore_ascii_case(table))
    }

    /// Create a new CDC event.
    pub fn create_event(
        &mut self,
        table: &str,
        operation: CdcOperation,
        before: Option<serde_json::Value>,
        after: Option<serde_json::Value>,
    ) -> CdcEvent {
        self.event_counter += 1;
        CdcEvent {
            event_id: format!("cdc-{:010}", self.event_counter),
            lsn: self.wal_position.lsn.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            table: table.to_string(),
            operation,
            before,
            after,
            published: false,
        }
    }

    /// Capture an event. Returns false if table is filtered.
    pub fn capture(&mut self, mut event: CdcEvent) -> bool {
        if !self.should_capture(&event.table) {
            return false;
        }

        self.events_captured += 1;
        event.lsn = self.wal_position.lsn.clone();
        self.pending_events.push(event);
        true
    }

    /// Publish all pending events (simulate batch publish).
    pub fn publish_batch(&mut self) -> u64 {
        let batch: Vec<CdcEvent> = self
            .pending_events
            .drain(..self.batch_size.min(self.pending_events.len()))
            .collect();
        let count = batch.len() as u64;

        for mut event in batch {
            event.published = true;
            self.events_published += 1;
        }

        if count > 0 {
            self.wal_position.last_flush = chrono::Utc::now().to_rfc3339();
        }

        count
    }

    /// Simulate a publish failure for remaining pending events.
    pub fn fail_pending(&mut self) -> u64 {
        let count = self.pending_events.len() as u64;
        self.events_failed += count;
        self.pending_events.clear();
        count
    }

    /// Update WAL position.
    pub fn set_wal_position(&mut self, lsn: &str, segment: u64, offset: u64) {
        self.wal_position = WalPosition {
            lsn: lsn.to_string(),
            segment,
            offset,
            last_flush: chrono::Utc::now().to_rfc3339(),
        };
    }

    /// Advance the WAL position by an offset.
    pub fn advance_wal(&mut self, delta: u64) {
        self.wal_position.offset += delta;
        if self.wal_position.offset >= 0xFF_FFFF_FF {
            self.wal_position.segment += 1;
            self.wal_position.offset = 0;
        }
        self.wal_position.lsn =
            format!("{}/{}", self.wal_position.segment, self.wal_position.offset);
    }

    /// Set the CDC lag.
    pub fn set_lag(&mut self, seconds: f64) {
        self.lag_seconds = seconds;
    }

    /// Get pending event count.
    pub fn pending_count(&self) -> usize {
        self.pending_events.len()
    }

    /// Get CDC statistics.
    pub fn stats(&self) -> CdcStats {
        let mut by_table = HashMap::new();
        let mut by_op = HashMap::new();
        for event in &self.pending_events {
            *by_table.entry(event.table.clone()).or_insert(0) += 1;
            *by_op.entry(event.operation.to_string()).or_insert(0) += 1;
        }

        CdcStats {
            events_captured: self.events_captured,
            events_published: self.events_published,
            events_failed: self.events_failed,
            events_by_table: by_table,
            events_by_operation: by_op,
            lag_seconds: self.lag_seconds,
        }
    }

    /// Serialize an event to the configured format.
    pub fn serialize_event(&self, event: &CdcEvent) -> String {
        match self.format.as_str() {
            "json" => serde_json::to_string(event).unwrap_or_default(),
            _ => serde_json::to_string(event).unwrap_or_default(),
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
            },
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
            Metric::new("cdc_pending_events", self.pending_events.len() as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shim() -> CdcShim {
        CdcShim {
            tables: Vec::new(),
            table_filter_enabled: false,
            batch_size: 5,
            ..CdcShim::new()
        }
    }

    #[test]
    fn test_cdc_operation_parse() {
        assert_eq!(
            "insert".parse::<CdcOperation>().unwrap(),
            CdcOperation::Insert
        );
        assert_eq!(
            "update".parse::<CdcOperation>().unwrap(),
            CdcOperation::Update
        );
        assert_eq!(
            "delete".parse::<CdcOperation>().unwrap(),
            CdcOperation::Delete
        );
        assert!("invalid".parse::<CdcOperation>().is_err());
    }

    #[test]
    fn test_should_capture_all_tables() {
        let shim = make_shim();
        assert!(shim.should_capture("users"));
        assert!(shim.should_capture("orders"));
    }

    #[test]
    fn test_should_capture_filtered() {
        let shim = CdcShim {
            tables: vec!["users".to_string()],
            table_filter_enabled: true,
            ..CdcShim::new()
        };
        assert!(shim.should_capture("users"));
        assert!(!shim.should_capture("orders"));
    }

    #[test]
    fn test_capture_and_publish() {
        let mut shim = make_shim();
        let e1 = shim.create_event(
            "users",
            CdcOperation::Insert,
            None,
            Some(serde_json::json!({"id": 1})),
        );
        let e2 = shim.create_event(
            "users",
            CdcOperation::Update,
            Some(serde_json::json!({"id": 1})),
            Some(serde_json::json!({"id": 1, "name": "alice"})),
        );

        assert!(shim.capture(e1));
        assert!(shim.capture(e2));
        assert_eq!(shim.events_captured, 2);
        assert_eq!(shim.pending_count(), 2);

        let published = shim.publish_batch();
        assert_eq!(published, 2);
        assert_eq!(shim.events_published, 2);
        assert_eq!(shim.pending_count(), 0);
    }

    #[test]
    fn test_capture_filtered_table() {
        let mut shim = CdcShim {
            tables: vec!["orders".to_string()],
            table_filter_enabled: true,
            ..CdcShim::new()
        };
        let e = shim.create_event("users", CdcOperation::Insert, None, None);
        assert!(!shim.capture(e));
        assert_eq!(shim.events_captured, 0);
    }

    #[test]
    fn test_publish_batch_respects_batch_size() {
        let mut shim = make_shim();
        for i in 0..10 {
            let e = shim.create_event(
                "users",
                CdcOperation::Insert,
                None,
                Some(serde_json::json!({"id": i})),
            );
            shim.capture(e);
        }

        let first_batch = shim.publish_batch();
        assert_eq!(first_batch, 5);
        assert_eq!(shim.pending_count(), 5);

        let second_batch = shim.publish_batch();
        assert_eq!(second_batch, 5);
    }

    #[test]
    fn test_fail_pending() {
        let mut shim = make_shim();
        let e = shim.create_event("users", CdcOperation::Insert, None, None);
        shim.capture(e);

        let failed = shim.fail_pending();
        assert_eq!(failed, 1);
        assert_eq!(shim.events_failed, 1);
        assert_eq!(shim.pending_count(), 0);
    }

    #[test]
    fn test_set_and_advance_wal() {
        let mut shim = make_shim();
        shim.set_wal_position("0/1000", 0, 1000);
        assert_eq!(shim.wal_position.lsn, "0/1000");

        shim.advance_wal(500);
        assert_eq!(shim.wal_position.offset, 1500);
    }

    #[test]
    fn test_advance_wal_segment_rollover() {
        let mut shim = make_shim();
        shim.set_wal_position("0/FFFFFFF0", 0, 0xFFFF_FFF0);
        shim.advance_wal(32);
        assert_eq!(shim.wal_position.segment, 1);
        assert_eq!(shim.wal_position.offset, 0);
    }

    #[test]
    fn test_serialize_event_json() {
        let shim = make_shim();
        let event = CdcEvent {
            event_id: "cdc-1".to_string(),
            lsn: "0/100".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            table: "users".to_string(),
            operation: CdcOperation::Insert,
            before: None,
            after: Some(serde_json::json!({"id": 1})),
            published: false,
        };
        let json = shim.serialize_event(&event);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["event_id"], "cdc-1");
        assert_eq!(parsed["table"], "users");
    }

    #[test]
    fn test_stats() {
        let mut shim = make_shim();
        let e1 = shim.create_event("users", CdcOperation::Insert, None, None);
        let e2 = shim.create_event("orders", CdcOperation::Insert, None, None);
        shim.capture(e1);
        shim.capture(e2);

        let stats = shim.stats();
        assert_eq!(stats.events_captured, 2);
        assert_eq!(stats.events_published, 0);
        assert_eq!(stats.events_by_table.get("users"), Some(&1));
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = make_shim();
        shim.events_captured = 100;
        shim.events_published = 95;
        shim.events_failed = 3;
        shim.lag_seconds = 2.5;

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 5);
        assert_eq!(metrics[0].value, 100.0);
        assert_eq!(metrics[1].value, 95.0);
        assert_eq!(metrics[3].value, 2.5);
    }
}
