//! CLI management tool for EvergreenShims.
//!
//! `shimctl` provides a command-line interface for interacting with a running
//! shim instance via its management API. Supports backup, migration, configuration,
//! health, and failover operations.

pub mod client;
pub mod commands;

pub use client::ManagementClient;

use clap::Parser;
use commands::Command;

#[derive(Parser)]
#[command(
    name = "shimctl",
    about = "CLI management tool for EvergreenShims",
    version
)]
pub struct Cli {
    /// Management API endpoint
    #[arg(short, long, default_value = "http://localhost:9090")]
    pub endpoint: String,

    #[command(subcommand)]
    pub command: Command,
}

#[cfg(test)]
mod tests {
    use crate::client;
    use crate::commands::{BackupAction, Command, ConfigAction, MigrationAction};
    use crate::Cli;
    use clap::Parser;

    fn parse_args(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn test_status_subcommand() {
        let cli = parse_args(&["shimctl", "status"]);
        assert!(matches!(cli.command, Command::Status));
        assert_eq!(cli.endpoint, "http://localhost:9090");
    }

    #[test]
    fn test_metrics_subcommand() {
        let cli = parse_args(&["shimctl", "metrics"]);
        assert!(matches!(cli.command, Command::Metrics));
    }

    #[test]
    fn test_health_subcommand() {
        let cli = parse_args(&["shimctl", "health"]);
        assert!(matches!(cli.command, Command::Health));
    }

    #[test]
    fn test_config_reload_subcommand() {
        let cli = parse_args(&["shimctl", "config", "reload"]);
        match cli.command {
            Command::Config { action } => assert!(matches!(action, ConfigAction::Reload)),
            _ => panic!("Expected Config subcommand"),
        }
    }

    #[test]
    fn test_config_validate_subcommand() {
        let cli = parse_args(&["shimctl", "config", "validate", "-f", "/etc/shim.toml"]);
        match cli.command {
            Command::Config { action } => match action {
                ConfigAction::Validate { file } => assert_eq!(file, "/etc/shim.toml"),
                _ => panic!("Expected Validate action"),
            },
            _ => panic!("Expected Config subcommand"),
        }
    }

    #[test]
    fn test_config_validate_long_flag() {
        let cli = parse_args(&["shimctl", "config", "validate", "--file", "config.toml"]);
        match cli.command {
            Command::Config { action } => match action {
                ConfigAction::Validate { file } => assert_eq!(file, "config.toml"),
                _ => panic!("Expected Validate action"),
            },
            _ => panic!("Expected Config subcommand"),
        }
    }

    #[test]
    fn test_backup_list_subcommand() {
        let cli = parse_args(&["shimctl", "backup", "list"]);
        match cli.command {
            Command::Backup { action } => assert!(matches!(action, BackupAction::List)),
            _ => panic!("Expected Backup subcommand"),
        }
    }

    #[test]
    fn test_backup_trigger_subcommand() {
        let cli = parse_args(&["shimctl", "backup", "trigger"]);
        match cli.command {
            Command::Backup { action } => assert!(matches!(action, BackupAction::Trigger)),
            _ => panic!("Expected Backup subcommand"),
        }
    }

    #[test]
    fn test_migration_status_subcommand() {
        let cli = parse_args(&["shimctl", "migration", "status"]);
        match cli.command {
            Command::Migration { action } => assert!(matches!(action, MigrationAction::Status)),
            _ => panic!("Expected Migration subcommand"),
        }
    }

    #[test]
    fn test_migration_apply_subcommand() {
        let cli = parse_args(&["shimctl", "migration", "apply"]);
        match cli.command {
            Command::Migration { action } => assert!(matches!(action, MigrationAction::Apply)),
            _ => panic!("Expected Migration subcommand"),
        }
    }

    #[test]
    fn test_migration_rollback_subcommand() {
        let cli = parse_args(&["shimctl", "migration", "rollback"]);
        match cli.command {
            Command::Migration { action } => assert!(matches!(action, MigrationAction::Rollback)),
            _ => panic!("Expected Migration subcommand"),
        }
    }

    #[test]
    fn test_custom_endpoint() {
        let cli = parse_args(&["shimctl", "--endpoint", "http://remote:8080", "status"]);
        assert_eq!(cli.endpoint, "http://remote:8080");
    }

    #[test]
    fn test_short_endpoint_flag() {
        let cli = parse_args(&["shimctl", "-e", "http://prod:9090", "status"]);
        assert_eq!(cli.endpoint, "http://prod:9090");
    }

    #[test]
    fn test_endpoint_before_subcommand() {
        let cli = parse_args(&["shimctl", "--endpoint", "http://x:1", "metrics"]);
        assert_eq!(cli.endpoint, "http://x:1");
        assert!(matches!(cli.command, Command::Metrics));
    }

    // --- Client Response Deserialization Tests ---

    #[test]
    fn test_status_response_deserialize() {
        let json = r#"{"status":"running","version":"1.0.0","uptime":"3600s","healthy":true}"#;
        let resp: client::StatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "running");
        assert_eq!(resp.version, "1.0.0");
        assert_eq!(resp.uptime, "3600s");
        assert!(resp.healthy);
    }

    #[test]
    fn test_metrics_response_deserialize() {
        let json = r#"{"metrics":{"uptime":100.0,"events":42}}"#;
        let resp: client::MetricsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.metrics.get("uptime").is_some());
    }

    #[test]
    fn test_health_response_deserialize() {
        let json = r#"{"status":"alive","timestamp":"2024-01-01T00:00:00Z"}"#;
        let resp: client::HealthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "alive");
        assert_eq!(resp.timestamp, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn test_reload_response_deserialize() {
        let json = r#"{"success":true,"message":"config reloaded"}"#;
        let resp: client::ReloadResponse = serde_json::from_str(json).unwrap();
        assert!(resp.success);
        assert_eq!(resp.message, "config reloaded");
    }

    #[test]
    fn test_validate_response_deserialize() {
        let json = r#"{"valid":false,"errors":["invalid port","missing host"]}"#;
        let resp: client::ValidateResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.valid);
        assert_eq!(resp.errors.len(), 2);
    }

    #[test]
    fn test_backup_entry_deserialize() {
        let json =
            r#"{"id":"bak-001","created_at":"2024-01-01","size_bytes":1024,"status":"completed"}"#;
        let resp: client::BackupEntry = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "bak-001");
        assert_eq!(resp.size_bytes, 1024);
        assert_eq!(resp.status, "completed");
    }

    #[test]
    fn test_backup_list_response_deserialize() {
        let json =
            r#"{"backups":[{"id":"b1","created_at":"2024-01-01","size_bytes":512,"status":"ok"}]}"#;
        let resp: client::BackupListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.backups.len(), 1);
    }

    #[test]
    fn test_backup_trigger_response_deserialize() {
        let json = r#"{"backup_id":"bak-002","status":"triggered"}"#;
        let resp: client::BackupTriggerResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.backup_id, "bak-002");
    }

    #[test]
    fn test_migration_status_response_deserialize() {
        let json = r#"{"current_version":"3","pending":["004_add_email"],"applied":["001_init","002_schema","003_users"]}"#;
        let resp: client::MigrationStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.current_version, "3");
        assert_eq!(resp.pending.len(), 1);
        assert_eq!(resp.applied.len(), 3);
    }

    #[test]
    fn test_migration_apply_response_deserialize() {
        let json =
            r#"{"success":true,"applied":["004_add_email"],"message":"1 migration applied"}"#;
        let resp: client::MigrationApplyResponse = serde_json::from_str(json).unwrap();
        assert!(resp.success);
        assert_eq!(resp.applied.len(), 1);
    }

    #[test]
    fn test_migration_rollback_response_deserialize() {
        let json = r#"{"success":true,"rolled_back":"004_add_email","message":"rolled back"}"#;
        let resp: client::MigrationRollbackResponse = serde_json::from_str(json).unwrap();
        assert!(resp.success);
        assert_eq!(resp.rolled_back, "004_add_email");
    }
}
