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
}
