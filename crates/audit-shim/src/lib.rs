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

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

/// Audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Timestamp.
    pub timestamp: String,
    /// Database name.
    pub database: String,
    /// Query or operation type.
    pub operation: String,
    /// Table affected.
    pub table: Option<String>,
    /// Query text (if enabled).
    pub query: Option<String>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Rows affected.
    pub rows_affected: Option<u64>,
    /// Client address.
    pub client_addr: Option<String>,
    /// User.
    pub user: Option<String>,
    /// Success status.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
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
    last_log: Option<chrono::DateTime<chrono::Utc>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl AuditShim {
    /// Create a new audit shim.
    pub fn new() -> Self {
        Self {
            database: std::env::var("AUDIT_DATABASE").unwrap_or_default(),
            tables: std::env::var("AUDIT_TABLES")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_string())
                .collect(),
            format: std::env::var("AUDIT_FORMAT")
                .unwrap_or_else(|_| "json".to_string()),
            output: std::env::var("AUDIT_OUTPUT")
                .unwrap_or_else(|_| "stdout".to_string()),
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
            last_log: None,
            shutdown_tx: None,
        }
    }

    /// Format an audit entry.
    fn format_entry(&self, entry: &AuditEntry) -> String {
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

    /// Write an audit entry.
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
                    file.write_all(format!("{}\n", formatted).as_bytes()).await?;
                }
            }
            "webhook" => {
                if let Some(url) = &self.webhook_url {
                    let client = reqwest::Client::new();
                    client
                        .post(url)
                        .json(entry)
                        .send()
                        .await?;
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
            if self.tables.is_empty() { "all".to_string() } else { self.tables.join(",") },
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
        vec![
            Metric::new("audit_queries_logged_total", self.queries_logged as f64),
        ]
    }
}
