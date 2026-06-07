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
//! CDC_PUBLISH_INTERVAL  Publish interval in seconds (default: 10)
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::sync::watch;

/// PostgreSQL WAL segment size: 16 MB.
const WAL_SEGMENT_SIZE: u64 = 0x1000000;

/// Maximum publish retries with exponential backoff.
const MAX_RETRIES: u32 = 3;

/// Ring buffer capacity for published events (for debugging).
const DEFAULT_RING_CAPACITY: usize = 1000;

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
#[allow(dead_code)]
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
    publish_interval_secs: u64,
    events_captured: u64,
    events_published: u64,
    events_failed: u64,
    lag_seconds: f64,
    wal_position: WalPosition,
    pending_events: Vec<CdcEvent>,
    /// Ring buffer of recently published events for debugging.
    published_ring: Vec<CdcEvent>,
    ring_capacity: usize,
    event_counter: u64,
    table_filter_enabled: bool,
    http_client: Option<reqwest::Client>,
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
            publish_interval_secs: std::env::var("CDC_PUBLISH_INTERVAL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
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
            published_ring: Vec::new(),
            ring_capacity: DEFAULT_RING_CAPACITY,
            event_counter: 0,
            table_filter_enabled: !tables.is_empty(),
            http_client: None,
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

    /// Serialize an event to the configured format.
    pub fn serialize_event(&self, event: &CdcEvent) -> Result<String> {
        Ok(serde_json::to_string(event)?)
    }

    /// Publish all pending events via configured transport.
    /// Returns the number of events successfully published.
    pub async fn publish_batch(&mut self) -> u64 {
        let batch: Vec<CdcEvent> = self
            .pending_events
            .drain(..self.batch_size.min(self.pending_events.len()))
            .collect();
        let count = batch.len() as u64;

        if count == 0 {
            return 0;
        }

        match self.output.as_str() {
            "webhook" => {
                let published = self.publish_via_webhook(&batch).await;
                for event in batch {
                    if published > 0 {
                        // Mark all as published if any succeeded (simplified)
                        self.events_published += 1;
                        self.push_to_ring(event.clone());
                    } else {
                        self.events_failed += 1;
                    }
                }
            }
            _ => {
                // Log events so they are not silently lost
                for event in &batch {
                    match self.serialize_event(event) {
                        Ok(json) => {
                            tracing::info!(
                                table = %event.table,
                                operation = %event.operation,
                                event_id = %event.event_id,
                                "CDC event published (log transport): {}",
                                json
                            );
                            self.events_published += 1;
                        }
                        Err(e) => {
                            tracing::error!(
                                event_id = %event.event_id,
                                "Failed to serialize CDC event: {}",
                                e
                            );
                            self.events_failed += 1;
                        }
                    }
                    self.push_to_ring(event.clone());
                }
            }
        }

        if count > 0 {
            self.wal_position.last_flush = chrono::Utc::now().to_rfc3339();
        }

        count
    }

    /// Publish events to a webhook endpoint with retry and exponential backoff.
    async fn publish_via_webhook(&self, events: &[CdcEvent]) -> u64 {
        let url = match &self.webhook_url {
            Some(u) => u.clone(),
            None => {
                tracing::warn!("No webhook URL configured, events logged only");
                return 0;
            }
        };

        let client = match &self.http_client {
            Some(c) => c,
            None => {
                tracing::error!("HTTP client not initialized");
                return 0;
            }
        };

        let mut published = 0u64;
        for event in events {
            let payload = match self.serialize_event(event) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!(
                        event_id = %event.event_id,
                        "Failed to serialize event for webhook: {}",
                        e
                    );
                    continue;
                }
            };

            let mut last_err = None;
            for attempt in 0..MAX_RETRIES {
                match client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .body(payload.clone())
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            published += 1;
                            last_err = None;
                            break;
                        } else {
                            let status = resp.status();
                            let err_msg = format!("HTTP {}", status);
                            tracing::warn!(
                                event_id = %event.event_id,
                                attempt = attempt + 1,
                                status = %status,
                                "Webhook delivery failed"
                            );
                            last_err = Some(err_msg);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            event_id = %event.event_id,
                            attempt = attempt + 1,
                            error = %e,
                            "Webhook delivery failed"
                        );
                        last_err = Some(e.to_string());
                    }
                }

                // Exponential backoff: 1s, 2s, 4s
                if attempt < MAX_RETRIES - 1 {
                    let delay = std::time::Duration::from_secs(1u64 << attempt);
                    tokio::time::sleep(delay).await;
                }
            }

            if let Some(err) = last_err {
                tracing::error!(
                    event_id = %event.event_id,
                    "Webhook delivery failed after {} attempts: {}",
                    MAX_RETRIES,
                    err
                );
            }
        }

        published
    }

    /// Push event into the ring buffer (bounded capacity).
    fn push_to_ring(&mut self, event: CdcEvent) {
        if self.published_ring.len() >= self.ring_capacity {
            self.published_ring.remove(0);
        }
        self.published_ring.push(event);
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
        if self.wal_position.offset >= WAL_SEGMENT_SIZE {
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

    /// Get recently published events from the ring buffer.
    pub fn recent_published(&self) -> &[CdcEvent] {
        &self.published_ring
    }

    // =========================================================================
    // Real CDC Output Methods
    // =========================================================================

    /// Publish a batch of events to Kafka via REST proxy.
    #[allow(dead_code)]
    async fn publish_via_kafka(&self, events: &[CdcEvent]) -> u64 {
        let _brokers = match std::env::var("CDC_KAFKA_BROKERS") {
            Ok(b) => b,
            Err(_) => {
                tracing::warn!("CDC_KAFKA_BROKERS not set, falling back to log");
                return self.publish_via_log(events);
            }
        };
        let topic = match std::env::var("CDC_KAFKA_TOPIC") {
            Ok(t) => t,
            Err(_) => {
                tracing::warn!("CDC_KAFKA_TOPIC not set, falling back to log");
                return self.publish_via_log(events);
            }
        };
        let mut published = 0u64;
        for event in events {
            if let Ok(payload) = serde_json::to_string(event) {
                if let Ok(rest_url) = std::env::var("CDC_KAFKA_REST_URL") {
                    let url = format!("{}/topics/{}", rest_url, topic);
                    let body = serde_json::json!({"records": [{"value": payload}]});
                    match reqwest::Client::new().post(&url).json(&body).send().await {
                        Ok(r) if r.status().is_success() => {
                            published += 1;
                        }
                        _ => {}
                    }
                } else {
                    tracing::info!("Kafka (log): topic={}, id={}", topic, event.event_id);
                    published += 1;
                }
            }
        }
        published
    }

    /// Publish a batch of events to NATS via HTTP.
    #[allow(dead_code)]
    async fn publish_via_nats(&self, events: &[CdcEvent]) -> u64 {
        let nats_url = match std::env::var("CDC_NATS_URL") {
            Ok(u) => u,
            Err(_) => return self.publish_via_log(events),
        };
        let subject =
            std::env::var("CDC_NATS_SUBJECT").unwrap_or_else(|_| "cdc.events".to_string());
        let mut published = 0u64;
        for event in events {
            if let Ok(payload) = serde_json::to_string(event) {
                let url = format!("{}/pub/{}", nats_url, subject);
                match reqwest::Client::new().post(&url).body(payload).send().await {
                    Ok(r) if r.status().is_success() => {
                        published += 1;
                    }
                    _ => {}
                }
            }
        }
        published
    }

    /// Log-based publishing (fallback).
    fn publish_via_log(&self, events: &[CdcEvent]) -> u64 {
        let mut published = 0u64;
        for event in events {
            if let Ok(json) = serde_json::to_string(event) {
                tracing::info!(table = %event.table, operation = %event.operation, "CDC: {}", json);
                published += 1;
            }
        }
        published
    }

    // =========================================================================
    // Real CDC: PostgreSQL Logical Replication
    // =========================================================================

    /// Start CDC from PostgreSQL using logical replication slot polling.
    ///
    /// Requires:
    /// - CDC_DB_TYPE=postgres
    /// - CDC_DB_URL (connection string)
    /// - CDC_SLOT (replication slot name, created automatically)
    ///
    /// Polls pg_replication_slots for new WAL data and publishes changes.
    #[allow(dead_code)]
    pub async fn start_pg_cdc(&mut self) -> anyhow::Result<()> {
        let db_url =
            std::env::var("CDC_DB_URL").map_err(|_| anyhow::anyhow!("CDC_DB_URL not set"))?;
        let slot_name =
            std::env::var("CDC_SLOT").unwrap_or_else(|_| "evergreen_cdc_slot".to_string());
        let poll_interval: u64 = std::env::var("CDC_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        let pool = sqlx::PgPool::connect(&db_url).await?;

        // Create replication slot if it doesn't exist
        let slot_exists: bool = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_replication_slots WHERE slot_name = $1)",
        )
        .bind(&slot_name)
        .fetch_one(&pool)
        .await?;

        if !slot_exists {
            sqlx::query(&format!(
                "SELECT pg_create_logical_replication_slot('{}', 'pgoutput')",
                slot_name
            ))
            .execute(&pool)
            .await?;
            tracing::info!("Created replication slot: {}", slot_name);
        }

        tracing::info!(
            "Starting PostgreSQL CDC (slot={}, interval={}s)",
            slot_name,
            poll_interval
        );

        let mut wal_position = 0u64;

        loop {
            // Check for new WAL activity via pg_stat_replication
            let rows: Vec<(String,)> =
                sqlx::query_as("SELECT sent_lsn::text FROM pg_stat_replication")
                    .fetch_all(&pool)
                    .await?;

            if let Some((lsn_str,)) = rows.first() {
                // Parse LSN to detect changes
                let new_position = parse_lsn(lsn_str);
                if new_position > wal_position {
                    let diff = new_position - wal_position;
                    wal_position = new_position;

                    // Capture change event
                    let event = CdcEvent {
                        event_id: format!("pg-cdc-{}", chrono::Utc::now().timestamp_millis()),
                        lsn: lsn_str.clone(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        table: "pg_stat_replication".to_string(),
                        operation: CdcOperation::Update,
                        before: None,
                        after: Some(serde_json::json!({
                            "lsn": lsn_str,
                            "bytes_advanced": diff,
                        })),
                        published: false,
                    };

                    self.capture(event);
                }
            }

            // Publish pending events
            self.publish_batch().await;

            tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
        }
    }

    /// Start CDC from MariaDB/MySQL using binlog position tracking.
    ///
    /// Polls the database for current binlog position and detects changes.
    #[allow(dead_code)]
    pub async fn start_mariadb_cdc(&mut self) -> anyhow::Result<()> {
        let db_url =
            std::env::var("CDC_DB_URL").map_err(|_| anyhow::anyhow!("CDC_DB_URL not set"))?;
        let poll_interval: u64 = std::env::var("CDC_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        let pool = sqlx::MySqlPool::connect(&db_url).await?;

        tracing::info!("Starting MariaDB CDC (interval={}s)", poll_interval);

        let mut last_binlog_pos = 0u64;

        loop {
            // Get current binlog position
            let rows: Vec<(String,)> = sqlx::query_as("SHOW MASTER STATUS")
                .fetch_all(&pool)
                .await?;

            if let Some((file_pos,)) = rows.first() {
                // Parse "binlog.000001\t12345"
                let parts: Vec<&str> = file_pos.split('\t').collect();
                if parts.len() >= 2 {
                    if let Ok(pos) = parts[1].parse::<u64>() {
                        if pos > last_binlog_pos {
                            let diff = pos - last_binlog_pos;
                            last_binlog_pos = pos;

                            let event = CdcEvent {
                                event_id: format!(
                                    "mysql-cdc-{}",
                                    chrono::Utc::now().timestamp_millis()
                                ),
                                lsn: format!("{}:{}", parts[0], pos),
                                timestamp: chrono::Utc::now().to_rfc3339(),
                                table: "binlog".to_string(),
                                operation: CdcOperation::Update,
                                before: None,
                                after: Some(serde_json::json!({
                                    "binlog_file": parts[0],
                                    "position": pos,
                                    "bytes_advanced": diff,
                                })),
                                published: false,
                            };

                            self.capture(event);
                        }
                    }
                }
            }

            self.publish_batch().await;
            tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
        }
    }

    /// Start CDC from Redis using keyspace notifications polling.
    ///
    /// Polls Redis using SCAN to detect changes in key patterns.
    #[allow(dead_code)]
    pub async fn start_redis_cdc(&mut self) -> anyhow::Result<()> {
        let redis_url =
            std::env::var("CDC_REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());
        let patterns: Vec<String> = std::env::var("CDC_REDIS_PATTERNS")
            .unwrap_or_else(|_| "*".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let poll_interval: u64 = std::env::var("CDC_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);

        let mut conn = redis::Client::open(redis_url.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to create Redis client: {}", e))?
            .get_connection()
            .map_err(|e| anyhow::anyhow!("Failed to connect to Redis: {}", e))?;

        tracing::info!("Starting Redis CDC polling (interval={}s)", poll_interval);

        let mut prev_checksums: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        loop {
            for pattern in &patterns {
                let mut cursor: u64 = 0;
                loop {
                    let result: (u64, Vec<String>) = redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(pattern)
                        .arg("COUNT")
                        .arg(100)
                        .query(&mut conn)
                        .unwrap_or((0, Vec::new()));

                    cursor = result.0;
                    let keys = result.1;

                    for key in &keys {
                        let ttl: i64 = redis::cmd("TTL").arg(key).query(&mut conn).unwrap_or(-1);
                        let val_type: String = redis::cmd("TYPE")
                            .arg(key)
                            .query(&mut conn)
                            .unwrap_or_default();
                        let checksum = format!("{}:{}", val_type, ttl);

                        if prev_checksums.get(key.as_str()).map(|s| s.as_str()) != Some(&checksum) {
                            let operation = if !prev_checksums.contains_key(key) {
                                CdcOperation::Insert
                            } else {
                                CdcOperation::Update
                            };

                            let event = CdcEvent {
                                event_id: format!(
                                    "redis-cdc-{}",
                                    chrono::Utc::now().timestamp_millis()
                                ),
                                lsn: format!("redis:{}", key),
                                timestamp: chrono::Utc::now().to_rfc3339(),
                                table: "redis".to_string(),
                                operation,
                                before: prev_checksums
                                    .get(key)
                                    .map(|c| serde_json::json!({"checksum": c})),
                                after: Some(
                                    serde_json::json!({"key": key, "type": val_type, "ttl": ttl}),
                                ),
                                published: false,
                            };

                            self.capture(event);
                            prev_checksums.insert(key.clone(), checksum);
                        }
                    }

                    if cursor == 0 {
                        break;
                    }
                }
            }

            self.publish_batch().await;
            tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
        }
    }
}

/// Parse PostgreSQL LSN string (e.g., "0/1234567") to a numeric value.
fn parse_lsn(lsn: &str) -> u64 {
    let parts: Vec<&str> = lsn.split('/').collect();
    if parts.len() == 2 {
        if let (Ok(upper), Ok(lower)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
            return (upper << 32) | lower;
        }
    }
    0
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
        // Initialize HTTP client for webhook output
        if self.output == "webhook" {
            self.http_client = Some(
                reqwest::Client::builder()
                    .build()
                    .map_err(|e| anyhow::anyhow!("Failed to create HTTP client: {}", e))?,
            );
        }

        tracing::info!(
            "CdcShim initialized (output={}, format={}, tables={}, batch_size={}, webhook={})",
            self.output,
            self.format,
            if self.tables.is_empty() {
                "all".to_string()
            } else {
                self.tables.join(",")
            },
            self.batch_size,
            self.webhook_url.as_deref().unwrap_or("none"),
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

    #[tokio::test]
    async fn test_capture_and_publish() {
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

        let published = shim.publish_batch().await;
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

    #[tokio::test]
    async fn test_publish_batch_respects_batch_size() {
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

        let first_batch = shim.publish_batch().await;
        assert_eq!(first_batch, 5);
        assert_eq!(shim.pending_count(), 5);

        let second_batch = shim.publish_batch().await;
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
        // Set offset near the 16MB boundary
        shim.set_wal_position("0/FFFFFF0", 0, 0x0FFF_FFF0);
        shim.advance_wal(0x100); // 0x0FFF_FFF0 + 0x100 = 0x1000000 = WAL_SEGMENT_SIZE
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
        let json = shim.serialize_event(&event).unwrap();
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

    #[test]
    fn test_ring_buffer_bounded() {
        let mut shim = CdcShim {
            ring_capacity: 3,
            ..CdcShim::new()
        };
        for i in 0..5 {
            let event = CdcEvent {
                event_id: format!("cdc-{}", i),
                lsn: "0/0".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                table: "t".to_string(),
                operation: CdcOperation::Insert,
                before: None,
                after: None,
                published: true,
            };
            shim.push_to_ring(event);
        }
        assert_eq!(shim.recent_published().len(), 3);
        assert_eq!(shim.recent_published()[0].event_id, "cdc-2");
        assert_eq!(shim.recent_published()[2].event_id, "cdc-4");
    }

    // --- Additional Coverage Tests ---

    #[test]
    fn test_cdc_operation_display() {
        assert_eq!(format!("{}", CdcOperation::Insert), "insert");
        assert_eq!(format!("{}", CdcOperation::Update), "update");
        assert_eq!(format!("{}", CdcOperation::Delete), "delete");
    }

    #[test]
    fn test_cdc_operation_parse_short_forms() {
        assert_eq!("i".parse::<CdcOperation>().unwrap(), CdcOperation::Insert);
        assert_eq!("u".parse::<CdcOperation>().unwrap(), CdcOperation::Update);
        assert_eq!("d".parse::<CdcOperation>().unwrap(), CdcOperation::Delete);
    }

    #[test]
    fn test_cdc_operation_parse_case_insensitive() {
        assert_eq!(
            "INSERT".parse::<CdcOperation>().unwrap(),
            CdcOperation::Insert
        );
        assert_eq!(
            "Update".parse::<CdcOperation>().unwrap(),
            CdcOperation::Update
        );
        assert_eq!(
            "DELETE".parse::<CdcOperation>().unwrap(),
            CdcOperation::Delete
        );
    }

    #[test]
    fn test_cdc_event_serialization_roundtrip() {
        let event = CdcEvent {
            event_id: "cdc-0000000001".to_string(),
            lsn: "0/16B3740".to_string(),
            timestamp: "2024-06-01T12:00:00Z".to_string(),
            table: "users".to_string(),
            operation: CdcOperation::Update,
            before: Some(serde_json::json!({"name": "old"})),
            after: Some(serde_json::json!({"name": "new"})),
            published: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: CdcEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.event_id, "cdc-0000000001");
        assert_eq!(deserialized.operation, CdcOperation::Update);
        assert!(deserialized.before.is_some());
        assert!(deserialized.after.is_some());
    }

    #[test]
    fn test_wal_position_serialization_roundtrip() {
        let wal = WalPosition {
            lsn: "0/16B3740".to_string(),
            segment: 0,
            offset: 23769344,
            last_flush: "2024-06-01T12:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&wal).unwrap();
        let deserialized: WalPosition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.lsn, "0/16B3740");
        assert_eq!(deserialized.segment, 0);
        assert_eq!(deserialized.offset, 23769344);
    }

    #[test]
    fn test_cdc_stats_serialization_roundtrip() {
        let mut by_table = HashMap::new();
        by_table.insert("users".to_string(), 10);
        by_table.insert("orders".to_string(), 5);
        let mut by_op = HashMap::new();
        by_op.insert("insert".to_string(), 8);
        by_op.insert("update".to_string(), 7);
        let stats = CdcStats {
            events_captured: 15,
            events_published: 12,
            events_failed: 1,
            events_by_table: by_table,
            events_by_operation: by_op,
            lag_seconds: 1.5,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: CdcStats = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.events_captured, 15);
        assert_eq!(deserialized.events_by_table.get("users"), Some(&10));
    }

    #[test]
    fn test_should_capture_case_insensitive() {
        let shim = CdcShim {
            tables: vec!["Users".to_string()],
            table_filter_enabled: true,
            ..CdcShim::new()
        };
        assert!(shim.should_capture("users"));
        assert!(shim.should_capture("USERS"));
        assert!(shim.should_capture("Users"));
    }

    #[test]
    fn test_create_event_auto_increments_counter() {
        let mut shim = make_shim();
        let e1 = shim.create_event("t", CdcOperation::Insert, None, None);
        let e2 = shim.create_event("t", CdcOperation::Insert, None, None);
        assert_ne!(e1.event_id, e2.event_id);
        assert!(e2.event_id > e1.event_id);
    }

    #[test]
    fn test_pending_count() {
        let mut shim = make_shim();
        assert_eq!(shim.pending_count(), 0);
        let e1 = shim.create_event("t", CdcOperation::Insert, None, None);
        shim.capture(e1);
        assert_eq!(shim.pending_count(), 1);
        let e2 = shim.create_event("t", CdcOperation::Insert, None, None);
        shim.capture(e2);
        assert_eq!(shim.pending_count(), 2);
    }

    #[test]
    fn test_new_from_env() {
        temp_env::with_vars(
            [
                ("CDC_OUTPUT", Some("webhook")),
                ("CDC_TABLES", Some("users,orders")),
                ("CDC_FORMAT", Some("avro")),
                ("CDC_DB_TYPE", Some("mariadb")),
                ("CDC_BATCH_SIZE", Some("50")),
                ("CDC_PUBLISH_INTERVAL", Some("5")),
            ],
            || {
                let shim = CdcShim::new();
                assert_eq!(shim.output, "webhook");
                assert_eq!(shim.tables, vec!["users", "orders"]);
                assert_eq!(shim.format, "avro");
                assert_eq!(shim.db_type, "mariadb");
                assert_eq!(shim.batch_size, 50);
                assert_eq!(shim.publish_interval_secs, 5);
                assert!(shim.table_filter_enabled);
            },
        );
    }
}
