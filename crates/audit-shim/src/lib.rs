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
//! AUDIT_LOG_DIR       Directory for audit log files (default: /var/log/audit-shim)
//! AUDIT_MAX_ENTRIES   Max in-memory entries before rotation (default: 100000)
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use shim_core::{Capability, Config, Metric, Result};
use tokio::fs;
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
    log_dir: PathBuf,
    max_entries: usize,
    http_client: Option<reqwest::Client>,
}

impl AuditShim {
    pub fn new() -> Self {
        let max_entries: usize = std::env::var("AUDIT_MAX_ENTRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100_000);

        let log_dir: PathBuf = std::env::var("AUDIT_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/var/log/audit-shim"));

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
            log_dir,
            max_entries,
            http_client: None,
        }
    }

    /// Create a shim with explicit configuration (for testing).
    pub fn with_config(database: &str, log_dir: PathBuf, max_entries: usize) -> Self {
        let mut shim = Self::new();
        shim.database = database.to_string();
        shim.log_dir = log_dir;
        shim.max_entries = max_entries;
        shim
    }

    /// Load existing audit log files from disk into memory.
    async fn load_from_disk(&mut self) -> anyhow::Result<()> {
        if !self.log_dir.exists() {
            fs::create_dir_all(&self.log_dir).await?;
            return Ok(());
        }

        let mut entries: Vec<AuditEntry> = Vec::new();
        let mut max_counter: u64 = 0;

        let mut dir = fs::read_dir(&self.log_dir).await?;
        let mut log_files: Vec<PathBuf> = Vec::new();

        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("log") {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with("audit-") {
                    log_files.push(path);
                }
            }
        }

        log_files.sort();

        for path in &log_files {
            let content = fs::read_to_string(path).await?;
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<AuditEntry>(line) {
                    Ok(entry) => {
                        if let Some(id_str) = entry.id.strip_prefix("audit-") {
                            if let Ok(num) = id_str.parse::<u64>() {
                                if num > max_counter {
                                    max_counter = num;
                                }
                            }
                        }
                        entries.push(entry);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to parse audit entry from {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }

        self.entry_counter = max_counter;
        self.log = entries;

        tracing::info!(
            "Loaded {} audit entries from {} log files",
            self.log.len(),
            log_files.len()
        );
        Ok(())
    }

    /// Check if a table should be audited (empty table list = audit all).
    pub fn should_audit_table(&self, table: &str) -> bool {
        self.tables.is_empty() || self.tables.iter().any(|t| t.eq_ignore_ascii_case(table))
    }

    /// Create a new audit entry.
    #[allow(clippy::too_many_arguments)]
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
    pub async fn log_entry(&mut self, entry: AuditEntry) -> anyhow::Result<bool> {
        if entry.duration_ms < self.min_duration_ms {
            self.queries_filtered += 1;
            return Ok(false);
        }

        self.total_duration_ms += entry.duration_ms;
        if entry.duration_ms > self.max_duration_ms {
            self.max_duration_ms = entry.duration_ms;
        }

        self.queries_logged += 1;
        self.last_log = Some(chrono::Utc::now());

        let formatted = self.format_entry(&entry);
        tracing::info!(target: "audit", "{}", formatted);

        // Disk persistence
        self.write_to_disk(&entry).await?;

        // Webhook
        if self.output == "webhook" {
            self.send_webhook(&entry).await?;
        }

        // In-memory
        self.log.push(entry);

        // Log rotation
        if self.log.len() > self.max_entries {
            self.rotate_log().await?;
        }

        Ok(true)
    }

    /// Write a single entry to the daily audit log file on disk.
    async fn write_to_disk(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        fs::create_dir_all(&self.log_dir).await?;

        let ts = chrono::Utc::now();
        let date_str = ts.format("%Y-%m-%d").to_string();
        let path = self.log_dir.join(format!("audit-{}.log", date_str));

        let json_line = serde_json::to_string(entry)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(json_line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        Ok(())
    }

    /// Rotate the in-memory log: drain entries into a new file on disk.
    async fn rotate_log(&mut self) -> anyhow::Result<()> {
        if self.log.is_empty() {
            return Ok(());
        }

        let ts = chrono::Utc::now();
        let date_str = ts.format("%Y-%m-%d").to_string();
        let timestamp_str = ts.format("%H%M%S").to_string();
        let path = self
            .log_dir
            .join(format!("audit-{}-rotate-{}.log", date_str, timestamp_str));

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        for entry in &self.log {
            let json_line = serde_json::to_string(entry)?;
            file.write_all(json_line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        tracing::info!(
            "Rotated {} audit entries to {}",
            self.log.len(),
            path.display()
        );

        self.log.clear();
        Ok(())
    }

    /// Send an entry to the configured webhook URL.
    async fn send_webhook(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        if let (Some(url), Some(client)) = (&self.webhook_url, &self.http_client) {
            client.post(url).json(entry).send().await?;
        }
        Ok(())
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
        let since_dt = filter
            .since
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

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
                if let Some(ref since_dt) = since_dt {
                    if let Ok(entry_dt) = chrono::DateTime::parse_from_rfc3339(&e.timestamp) {
                        if entry_dt.with_timezone(&chrono::Utc) < *since_dt {
                            return false;
                        }
                    } else {
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

    /// Clear audit entries older than the given retention cutoff.
    /// Entries newer than `retention` are kept in memory.
    pub fn clear_log_before(&mut self, retention: chrono::DateTime<chrono::Utc>) {
        let before = self.log.len();
        self.log.retain(|e| {
            chrono::DateTime::parse_from_rfc3339(&e.timestamp)
                .map(|dt| dt.with_timezone(&chrono::Utc) >= retention)
                .unwrap_or(true)
        });
        let removed = before - self.log.len();
        if removed > 0 {
            tracing::info!("Cleared {} audit entries older than {}", removed, retention);
        }
    }

    /// Get the number of entries in the log.
    pub fn entry_count(&self) -> usize {
        self.log.len()
    }

    /// Get the log directory path.
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Manually trigger log rotation.
    pub async fn force_rotate(&mut self) -> anyhow::Result<()> {
        self.rotate_log().await
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

        // Create shared HTTP client once
        self.http_client = Some(reqwest::Client::builder().build().map_err(|e| {
            shim_core::Error::Config(format!("Failed to create HTTP client: {}", e))
        })?);

        // Ensure log directory exists
        fs::create_dir_all(&self.log_dir).await?;

        // Load existing audit entries from disk
        if let Err(e) = self.load_from_disk().await {
            tracing::warn!("Failed to load audit log from disk: {}", e);
        }

        tracing::info!(
            "AuditShim initialized (database={}, format={}, tables={}, log_dir={})",
            self.database,
            self.format,
            if self.tables.is_empty() {
                "all".to_string()
            } else {
                self.tables.join(",")
            },
            self.log_dir.display(),
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

    fn make_shim() -> (AuditShim, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let shim = AuditShim {
            database: "testdb".to_string(),
            log_queries: true,
            log_dir: dir.path().to_path_buf(),
            ..AuditShim::new()
        };
        (shim, dir)
    }

    fn make_shim_with_dir(dir: &Path) -> AuditShim {
        AuditShim {
            database: "testdb".to_string(),
            log_queries: true,
            log_dir: dir.to_path_buf(),
            max_entries: 5,
            ..AuditShim::new()
        }
    }

    fn make_http_client() -> reqwest::Client {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
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

    #[tokio::test]
    async fn test_log_entry_basic() {
        let (mut shim, _dir) = make_shim();
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
        let logged = shim.log_entry(entry).await.unwrap();
        assert!(logged);
        assert_eq!(shim.queries_logged, 1);
        assert_eq!(shim.entry_count(), 1);
    }

    #[tokio::test]
    async fn test_log_entry_filtered_by_min_duration() {
        let (base_shim, _dir) = make_shim();
        let mut shim = AuditShim {
            min_duration_ms: 100,
            ..base_shim
        };
        let entry = shim.create_entry("SELECT", None, None, 5, None, None, None, true, None);
        let logged = shim.log_entry(entry).await.unwrap();
        assert!(!logged);
        assert_eq!(shim.queries_filtered, 1);
        assert_eq!(shim.queries_logged, 0);
    }

    #[tokio::test]
    async fn test_log_entry_passes_min_duration() {
        let (base_shim, _dir) = make_shim();
        let mut shim = AuditShim {
            min_duration_ms: 10,
            ..base_shim
        };
        let entry = shim.create_entry("SELECT", None, None, 50, None, None, None, true, None);
        let logged = shim.log_entry(entry).await.unwrap();
        assert!(logged);
        assert_eq!(shim.queries_logged, 1);
    }

    #[test]
    fn test_log_queries_disabled() {
        let (base_shim, _dir) = make_shim();
        let mut shim = AuditShim {
            log_queries: false,
            ..base_shim
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
        let (mut shim, _dir) = make_shim();
        let e1 = shim.create_entry("SELECT", None, None, 10, None, None, None, true, None);
        shim.log.push(e1);
        let e2 = shim.create_entry("INSERT", None, None, 5, None, None, None, true, None);
        shim.log.push(e2);

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
        let (mut shim, _dir) = make_shim();
        let e1 = shim.create_entry("SELECT", None, None, 10, None, None, None, true, None);
        shim.log.push(e1);
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
        shim.log.push(e2);

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
        let (mut shim, _dir) = make_shim();
        for _ in 0..10 {
            let e = shim.create_entry("SELECT", None, None, 1, None, None, None, true, None);
            shim.log.push(e);
        }

        let filter = AuditFilter {
            limit: 3,
            ..AuditFilter::default()
        };
        assert_eq!(shim.query(&filter).len(), 3);
    }

    #[test]
    fn test_query_since_filter() {
        let (mut shim, _dir) = make_shim();

        let early = AuditEntry {
            id: "audit-0000000001".to_string(),
            timestamp: "2025-01-01T10:00:00Z".to_string(),
            database: "testdb".to_string(),
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
        let late = AuditEntry {
            id: "audit-0000000002".to_string(),
            timestamp: "2025-01-01T12:00:00Z".to_string(),
            database: "testdb".to_string(),
            operation: "INSERT".to_string(),
            table: None,
            query: None,
            duration_ms: 5,
            rows_affected: None,
            client_addr: None,
            user: None,
            success: true,
            error: None,
        };

        shim.log.push(early);
        shim.log.push(late);

        let filter = AuditFilter {
            since: Some("2025-01-01T11:00:00Z".to_string()),
            ..AuditFilter::default()
        };
        let results = shim.query(&filter);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].operation, "INSERT");
    }

    #[test]
    fn test_stats() {
        let (mut shim, _dir) = make_shim();
        let e1 = shim.create_entry("SELECT", None, None, 10, None, None, None, true, None);
        shim.log.push(e1);
        let e2 = shim.create_entry("INSERT", None, None, 20, None, None, None, true, None);
        shim.log.push(e2);
        let e3 = shim.create_entry("SELECT", None, None, 30, None, None, None, false, None);
        shim.log.push(e3);

        shim.queries_logged = 3;
        shim.total_duration_ms = 60;
        shim.max_duration_ms = 30;

        let stats = shim.stats();
        assert_eq!(stats.total_entries, 3);
        assert_eq!(stats.success_count, 2);
        assert_eq!(stats.failure_count, 1);
        assert_eq!(stats.max_duration_ms, 30);
        assert!((stats.avg_duration_ms - 20.0).abs() < 0.01);
    }

    #[test]
    fn test_clear_log_before() {
        let (mut shim, _dir) = make_shim();

        let old_entry = AuditEntry {
            id: "audit-0000000001".to_string(),
            timestamp: "2025-01-01T10:00:00Z".to_string(),
            database: "testdb".to_string(),
            operation: "SELECT".to_string(),
            table: None,
            query: None,
            duration_ms: 1,
            rows_affected: None,
            client_addr: None,
            user: None,
            success: true,
            error: None,
        };
        let new_entry = AuditEntry {
            id: "audit-0000000002".to_string(),
            timestamp: "2025-06-01T10:00:00Z".to_string(),
            database: "testdb".to_string(),
            operation: "SELECT".to_string(),
            table: None,
            query: None,
            duration_ms: 1,
            rows_affected: None,
            client_addr: None,
            user: None,
            success: true,
            error: None,
        };

        shim.log.push(old_entry);
        shim.log.push(new_entry);
        assert_eq!(shim.entry_count(), 2);

        let cutoff: chrono::DateTime<chrono::Utc> = chrono::NaiveDate::from_ymd_opt(2025, 3, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        shim.clear_log_before(cutoff);
        assert_eq!(shim.entry_count(), 1);
        assert_eq!(shim.log[0].timestamp, "2025-06-01T10:00:00Z");
    }

    #[test]
    fn test_format_json() {
        let (base_shim, _dir) = make_shim();
        let shim = AuditShim {
            format: "json".to_string(),
            ..base_shim
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
        let (base_shim, _dir) = make_shim();
        let shim = AuditShim {
            format: "cef".to_string(),
            ..base_shim
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
        let (mut shim, _dir) = make_shim();
        let e = shim.create_entry("SELECT", None, None, 100, None, None, None, true, None);
        shim.log_entry(e).await.unwrap();

        let metrics = shim.metrics();
        assert_eq!(metrics.len(), 4);
        assert_eq!(metrics[0].name, "audit_queries_logged_total");
        assert_eq!(metrics[0].value, 1.0);
        assert_eq!(metrics[2].name, "audit_avg_duration_ms");
        assert_eq!(metrics[2].value, 100.0);
    }

    // ── Disk persistence tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_disk_persistence_write_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().to_path_buf();

        // Write entries to disk
        {
            let mut shim = make_shim_with_dir(&log_dir);
            for i in 0..3 {
                let entry = shim.create_entry(
                    "SELECT",
                    Some("users"),
                    None,
                    i,
                    None,
                    None,
                    None,
                    true,
                    None,
                );
                shim.log_entry(entry).await.unwrap();
            }
            assert_eq!(shim.entry_count(), 3);
        }

        // Reload from disk
        {
            let mut shim = make_shim_with_dir(&log_dir);
            assert_eq!(shim.entry_count(), 0);
            shim.load_from_disk().await.unwrap();
            assert_eq!(shim.entry_count(), 3);
            assert_eq!(shim.entry_counter, 3);
            assert_eq!(shim.log[0].database, "testdb");
        }
    }

    #[tokio::test]
    async fn test_disk_persistence_daily_file_naming() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().to_path_buf();

        let mut shim = make_shim_with_dir(&log_dir);
        let entry = shim.create_entry("INSERT", None, None, 1, None, None, None, true, None);
        shim.log_entry(entry).await.unwrap();

        // Check that a file with today's date was created
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let expected = log_dir.join(format!("audit-{}.log", today));
        assert!(expected.exists(), "Expected {}", expected.display());

        // Verify file content is valid JSON
        let content = fs::read_to_string(&expected).await.unwrap();
        let line = content.lines().next().unwrap();
        let parsed: AuditEntry = serde_json::from_str(line).unwrap();
        assert_eq!(parsed.operation, "INSERT");
    }

    // ── Log rotation test ──────────────────────────────────────────

    #[tokio::test]
    async fn test_log_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().to_path_buf();

        let mut shim = make_shim_with_dir(&log_dir);
        // max_entries is 5 via make_shim_with_dir
        assert_eq!(shim.max_entries, 5);

        // Write 7 entries — should rotate after the 6th push (log.len() > 5)
        for i in 0..7 {
            let entry = shim.create_entry("SELECT", None, None, i, None, None, None, true, None);
            shim.log_entry(entry).await.unwrap();
        }

        // After rotation, in-memory should be smaller than max_entries
        assert!(
            shim.entry_count() <= shim.max_entries,
            "Expected <= {}, got {}",
            shim.max_entries,
            shim.entry_count()
        );

        // There should be a rotation file on disk
        let mut found_rotation = false;
        let mut dir_entries = fs::read_dir(&log_dir).await.unwrap();
        while let Some(entry) = dir_entries.next_entry().await.unwrap() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.contains("rotate") {
                found_rotation = true;
                break;
            }
        }
        assert!(
            found_rotation,
            "Expected a rotation file in {}",
            log_dir.display()
        );
    }

    // ── Webhook test with mock server ──────────────────────────────

    #[tokio::test]
    async fn test_webhook_mock_server() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let (mut stream, _peer) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let counter = counter_clone.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 4096];
                    let _n = stream.read(&mut buf).await.unwrap_or(0);
                    counter.fetch_add(1, Ordering::SeqCst);
                    let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"ok\":true}";
                    let _ = stream.write_all(resp).await;
                });
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let (base_shim, _dir) = make_shim();
        let mut shim = AuditShim {
            output: "webhook".to_string(),
            webhook_url: Some(format!("http://{}/webhook", addr)),
            http_client: Some(make_http_client()),
            ..base_shim
        };

        let entry = shim.create_entry("SELECT", None, None, 5, None, None, None, true, None);
        shim.log_entry(entry).await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    // ── Shared reqwest::Client test ────────────────────────────────

    #[test]
    fn test_shared_http_client_created_in_new() {
        let shim = AuditShim::new();
        assert!(shim.http_client.is_none());

        let (base_shim, _dir) = make_shim();
        // http_client is not set by new() — it's set in init()
        // In tests that don't go through init(), it stays None
        assert!(base_shim.http_client.is_none());

        // Verify it can be set manually for testing
        let shim = AuditShim {
            http_client: Some(make_http_client()),
            ..base_shim
        };
        assert!(shim.http_client.is_some());
    }
}
