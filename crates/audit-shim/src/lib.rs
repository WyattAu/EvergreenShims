#![allow(dead_code)]
//! Audit shim — database query logging and SIEM export.
//!
//! Captures database queries and exports them to syslog, file, or webhook.
//!
//! ## Environment Variables
//!
//! ```text
//! AUDIT_DATABASE      Database name to audit
//! AUDIT_TABLES        Comma-separated tables to audit (empty = all)
//! AUDIT_FORMAT        Output format: json, syslog, cef (default: json)
//! AUDIT_OUTPUT        Output destination: file, stdout, webhook (default: stdout)
//! AUDIT_OUTPUT_FILE   File path when output=file
//! AUDIT_WEBHOOK_URL   Webhook URL when output=webhook
//! AUDIT_LOG_QUERIES   Log full query text (default: false)
//! AUDIT_LOG_PARAMETERS Log query parameters (default: false)
//! AUDIT_MIN_DURATION_MS Minimum query duration to log (default: 0, log all)
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

/// Audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub timestamp: String,
    pub database: String,
    pub operation: String,
    pub table: Option<String>,
    pub query: Option<String>,
    pub duration_ms: u64,
    pub rows_affected: Option<u64>,
    pub client_addr: Option<String>,
    pub user: Option<String>,
    pub success: bool,
    pub error: Option<String>,
}

/// Audit filter for querying the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditFilter {
    pub operation: Option<String>,
    pub table: Option<String>,
    pub success: Option<bool>,
    pub min_duration_ms: Option<u64>,
    pub since: Option<String>,
    pub limit: usize,
}

impl Default for AuditFilter {
    fn default() -> Self {
        Self {
            operation: None,
            table: None,
            success: None,
            min_duration_ms: None,
            since: None,
            limit: 100,
        }
    }
}

/// Audit statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditStats {
    pub total_entries: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub operations: HashMap<String, u64>,
    pub avg_duration_ms: f64,
    pub max_duration_ms: u64,
}

/// Audit shim for database query logging.
pub struct AuditShim {
    database: String,
    tables: Vec<String>,
    format: String,
    output: String,
    output_file: Option<String>,
    webhook_url: Option<String>,
    log_queries: bool,
    log_parameters: bool,
    min_duration_ms: u64,
    queries_logged: u64,
    queries_filtered: u64,
    total_duration_ms: u64,
    max_duration_ms: u64,
    last_log: Option<chrono::DateTime<chrono::Utc>>,
    log: Vec<AuditEntry>,
    entry_counter: u64,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl AuditShim {
    pub fn new() -> Self {
        Self {
            database: std::env::var("AUDIT_DATABASE").unwrap_or_default(),
            tables: std::env::var("AUDIT_TABLES")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect(),
            format: std::env::var("AUDIT_FORMAT").unwrap_or_else(|_| "json".to_string()),
            output: std::env::var("AUDIT_OUTPUT").unwrap_or_else(|_| "stdout".to_string()),
            output_file: std::env::var("AUDIT_OUTPUT_FILE").ok(),
            webhook_url: std::env::var("AUDIT_WEBHOOK_URL").ok(),
            log_queries: std::env::var("AUDIT_LOG_QUERIES")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            log_parameters: std::env::var("AUDIT_LOG_PARAMETERS")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            min_duration_ms: std::env::var("AUDIT_MIN_DURATION_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            queries_logged: 0,
            queries_filtered: 0,
            total_duration_ms: 0,
            max_duration_ms: 0,
            last_log: None,
            log: Vec::new(),
            entry_counter: 0,
            shutdown_tx: None,
        }
    }

    /// Check if a table should be audited (empty table list = audit all).
    pub fn should_audit_table(&self, table: &str) -> bool {
        self.tables.is_empty() || self.tables.iter().any(|t| t.eq_ignore_ascii_case(table))
    }

    /// Create a new audit entry.
    pub fn create_entry(
        &mut self,
        operation: &str,
        table: Option<&str>,
        query: Option<&str>,
        duration_ms: u64,
        rows_affected: Option<u64>,
        client_addr: Option<&str>,
        user: Option<&str>,
        success: bool,
        error: Option<&str>,
    ) -> AuditEntry {
        self.entry_counter += 1;

        AuditEntry {
            id: format!("audit-{:010}", self.entry_counter),
            timestamp: chrono::Utc::now().to_rfc3339(),
            database: self.database.clone(),
            operation: operation.to_string(),
            table: table.map(|t| t.to_string()),
            query: if self.log_queries {
                query.map(|q| q.to_string())
            } else {
                None
            },
            duration_ms,
            rows_affected,
            client_addr: client_addr.map(|a| a.to_string()),
            user: user.map(|u| u.to_string()),
            success,
            error: error.map(|e| e.to_string()),
        }
    }

    /// Log an audit entry. Returns false if entry was filtered by min_duration_ms.
    pub fn log_entry(&mut self, mut entry: AuditEntry) -> bool {
        if entry.duration_ms < self.min_duration_ms {
            self.queries_filtered += 1;
            return false;
        }

        self.total_duration_ms += entry.duration_ms;
        if entry.duration_ms > self.max_duration_ms {
            self.max_duration_ms = entry.duration_ms;
        }

        self.queries_logged += 1;
        self.last_log = Some(chrono::Utc::now());

        let formatted = self.format_entry(&entry);
        tracing::info!(target: "audit", "{}", formatted);

        self.log.push(entry);
        true
    }

    /// Format an audit entry.
    pub fn format_entry(&self, entry: &AuditEntry) -> String {
        match self.format.as_str() {
            "json" => serde_json::to_string(entry).unwrap_or_default(),
            "cef" => {
                format!(
                    "CEF:0|EvergreenShim|audit|1.0|{}|{}|{}|database={} operation={} table={}",
                    entry.success as i64,
                    entry.operation,
                    entry.duration_ms,
                    entry.database,
                    entry.operation,
                    entry.table.as_deref().unwrap_or("none"),
                )
            }
            _ => serde_json::to_string(entry).unwrap_or_default(),
        }
    }

    /// Query the audit log with a filter.
    pub fn query(&self, filter: &AuditFilter) -> Vec<&AuditEntry> {
        self.log
            .iter()
            .filter(|e| {
                if let Some(ref op) = filter.operation {
                    if !e.operation.eq_ignore_ascii_case(op) {
                        return false;
                    }
                }
                if let Some(ref tbl) = filter.table {
                    if e.table
                        .as_deref()
                        .map(|t| !t.eq_ignore_ascii_case(tbl))
                        .unwrap_or(true)
                    {
                        return false;
                    }
                }
                if let Some(success) = filter.success {
                    if e.success != success {
                        return false;
                    }
                }
                if let Some(min_dur) = filter.min_duration_ms {
                    if e.duration_ms < min_dur {
                        return false;
                    }
                }
                true
            })
            .take(filter.limit)
            .collect()
    }

    /// Get audit statistics.
    pub fn stats(&self) -> AuditStats {
        let mut operations: HashMap<String, u64> = HashMap::new();
        let mut success_count = 0u64;
        let mut failure_count = 0u64;

        for entry in &self.log {
            *operations.entry(entry.operation.clone()).or_insert(0) += 1;
            if entry.success {
                success_count += 1;
            } else {
                failure_count += 1;
            }
        }

        let avg_duration_ms = if self.queries_logged > 0 {
            self.total_duration_ms as f64 / self.queries_logged as f64
        } else {
            0.0
        };

        AuditStats {
            total_entries: self.queries_logged,
            success_count,
            failure_count,
            operations,
            avg_duration_ms,
            max_duration_ms: self.max_duration_ms,
        }
    }

    /// Clear the in-memory audit log.
    pub fn clear_log(&mut self) {
        self.log.clear();
    }

    /// Get the number of entries in the log.
    pub fn entry_count(&self) -> usize {
        self.log.len()
    }

    async fn write_entry(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        let formatted = self.format_entry(entry);

        match self.output.as_str() {
            "stdout" => {
                tracing::info!(target: "audit", "{}", formatted);
            }
            "file" => {
                if let Some(path) = &self.output_file {
                    let mut file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .await?;
                    file.write_all(format!("{}\n", formatted).as_bytes())
                        .await?;
                }
            }
            "webhook" => {
                if let Some(url) = &self.webhook_url {
                    let client = reqwest::Client::new();
                    client.post(url).json(entry).send().await?;
                }
            }
            _ => {}
        }

        Ok(())
    }
}

impl Default for AuditShim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Capability for AuditShim {
    fn name(&self) -> &str {
        "audit"
    }

    async fn init(&mut self, config: &Config) -> Result<()> {
        if let Some(audit_config) = &config.audit {
            self.database = audit_config.database.clone();
            self.tables = audit_config.tables.clone();
            self.format = audit_config.format.clone();
        }
        tracing::info!(
            "AuditShim initialized (database={}, format={}, tables={})",
            self.database,
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
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        tracing::info!("AuditShim started (output={})", self.output);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        tracing::info!("AuditShim stopped");
        Ok(())
    }

    fn metrics(&self) -> Vec<Metric> {
        let avg = if self.queries_logged > 0 {
            self.total_duration_ms as f64 / self.queries_logged as f64
        } else {
            0.0
        };

        vec![
            Metric::new("audit_queries_logged_total", self.queries_logged as f64),
            Metric::new("audit_queries_filtered_total", self.queries_filtered as f64),
            Metric::new("audit_avg_duration_ms", avg),
            Metric::new("audit_max_duration_ms", self.max_duration_ms as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shim() -> AuditShim {
        AuditShim {
            database: "testdb".to_string(),
            log_queries: true,
            ..AuditShim::new()
        }
    }

    #[test]
    fn test_should_audit_table_all() {
        let shim = AuditShim::new();
        assert!(shim.should_audit_table("users"));
        assert!(shim.should_audit_table("orders"));
    }

    #[test]
    fn test_should_audit_table_filtered() {
        let mut shim = AuditShim::new();
        shim.tables = vec!["users".to_string(), "orders".to_string()];
        assert!(shim.should_audit_table("users"));
        assert!(!shim.should_audit_table("payments"));
    }

    #[test]
    fn test_should_audit_table_case_insensitive() {
        let mut shim = AuditShim::new();
        shim.tables = vec!["USERS".to_string()];
        assert!(shim.should_audit_table("users"));
    }

    #[test]
    fn test_log_entry_basic() {
        let mut shim = make_shim();
        let entry = shim.create_entry(
            "SELECT",
            Some("users"),
            Some("SELECT * FROM users"),
            5,
            Some(10),
            None,
            None,
            true,
            None,
        );
        let logged = shim.log_entry(entry);
        assert!(logged);
        assert_eq!(shim.queries_logged, 1);
        assert_eq!(shim.entry_count(), 1);
    }

    #[test]
    fn test_log_entry_filtered_by_min_duration() {
        let mut shim = AuditShim {
            min_duration_ms: 100,
            ..make_shim()
        };
        let entry = shim.create_entry("SELECT", None, None, 5, None, None, None, true, None);
        let logged = shim.log_entry(entry);
        assert!(!logged);
        assert_eq!(shim.queries_filtered, 1);
        assert_eq!(shim.queries_logged, 0);
    }

    #[test]
    fn test_log_entry_passes_min_duration() {
        let mut shim = AuditShim {
            min_duration_ms: 10,
            ..make_shim()
        };
        let entry = shim.create_entry("SELECT", None, None, 50, None, None, None, true, None);
        let logged = shim.log_entry(entry);
        assert!(logged);
        assert_eq!(shim.queries_logged, 1);
    }

    #[test]
    fn test_log_queries_disabled() {
        let mut shim = AuditShim {
            log_queries: false,
            ..make_shim()
        };
        let entry = shim.create_entry(
            "SELECT",
            Some("users"),
            Some("SELECT 1"),
            5,
            None,
            None,
            None,
            true,
            None,
        );
        assert!(entry.query.is_none());
    }

    #[test]
    fn test_query_by_operation() {
        let mut shim = make_shim();
        let e1 = shim.create_entry("SELECT", None, None, 10, None, None, None, true, None);
        shim.log_entry(e1);
        let e2 = shim.create_entry("INSERT", None, None, 5, None, None, None, true, None);
        shim.log_entry(e2);

        let filter = AuditFilter {
            operation: Some("SELECT".to_string()),
            ..AuditFilter::default()
        };
        let results = shim.query(&filter);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].operation, "SELECT");
    }

    #[test]
    fn test_query_by_success() {
        let mut shim = make_shim();
        let e1 = shim.create_entry("SELECT", None, None, 10, None, None, None, true, None);
        shim.log_entry(e1);
        let e2 = shim.create_entry(
            "SELECT",
            None,
            None,
            5,
            None,
            None,
            None,
            false,
            Some("timeout"),
        );
        shim.log_entry(e2);

        let filter = AuditFilter {
            success: Some(false),
            ..AuditFilter::default()
        };
        let results = shim.query(&filter);
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
    }

    #[test]
    fn test_query_with_limit() {
        let mut shim = make_shim();
        for _ in 0..10 {
            let e = shim.create_entry("SELECT", None, None, 1, None, None, None, true, None);
            shim.log_entry(e);
        }

        let filter = AuditFilter {
            limit: 3,
            ..AuditFilter::default()
        };
        assert_eq!(shim.query(&filter).len(), 3);
    }

    #[test]
    fn test_stats() {
        let mut shim = make_shim();
        let e1 = shim.create_entry("SELECT", None, None, 10, None, None, None, true, None);
        shim.log_entry(e1);
        let e2 = shim.create_entry("INSERT", None, None, 20, None, None, None, true, None);
        shim.log_entry(e2);
        let e3 = shim.create_entry("SELECT", None, None, 30, None, None, None, false, None);
        shim.log_entry(e3);

        let stats = shim.stats();
        assert_eq!(stats.total_entries, 3);
        assert_eq!(stats.success_count, 2);
        assert_eq!(stats.failure_count, 1);
        assert_eq!(stats.max_duration_ms, 30);
        assert!((stats.avg_duration_ms - 20.0).abs() < 0.01);
    }

    #[test]
    fn test_clear_log() {
        let mut shim = make_shim();
        let e = shim.create_entry("SELECT", None, None, 1, None, None, None, true, None);
        shim.log_entry(e);
        assert_eq!(shim.entry_count(), 1);

        shim.clear_log();
        assert_eq!(shim.entry_count(), 0);
        assert_eq!(shim.queries_logged, 1);
    }

    #[test]
    fn test_format_json() {
        let shim = AuditShim {
            format: "json".to_string(),
            ..make_shim()
        };
        let entry = AuditEntry {
            id: "test".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            database: "db".to_string(),
            operation: "SELECT".to_string(),
            table: None,
            query: None,
            duration_ms: 5,
            rows_affected: None,
            client_addr: None,
            user: None,
            success: true,
            error: None,
        };
        let formatted = shim.format_entry(&entry);
        let parsed: serde_json::Value = serde_json::from_str(&formatted).unwrap();
        assert_eq!(parsed["operation"], "SELECT");
    }

    #[test]
    fn test_format_cef() {
        let shim = AuditShim {
            format: "cef".to_string(),
            ..make_shim()
        };
        let entry = AuditEntry {
            id: "test".to_string(),
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            database: "db".to_string(),
            operation: "SELECT".to_string(),
            table: Some("users".to_string()),
            query: None,
            duration_ms: 5,
            rows_affected: None,
            client_addr: None,
            user: None,
            success: true,
            error: None,
        };
        let formatted = shim.format_entry(&entry);
        assert!(formatted.starts_with("CEF:0|"));
    }

    #[tokio::test]
    async fn test_metrics() {
        let mut shim = make_shim();
        let e = shim.create_entry("SELECT", None, None, 100, None, None, None, true, None);
        shim.log_entry(e);

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].name, "audit_queries_logged_total");
        assert_eq!(metrics[0].value, 1.0);
        assert_eq!(metrics[2].name, "audit_avg_duration_ms");
        assert_eq!(metrics[2].value, 100.0);
    }
}
