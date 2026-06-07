use anyhow::Result;
use clap::Subcommand;

use crate::client::ManagementClient;

#[derive(Subcommand)]
pub enum Command {
    /// Show shim status
    Status,

    /// Show Prometheus metrics
    Metrics,

    /// Check health (liveness and readiness)
    Health,

    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Backup management
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },

    /// Migration management
    Migration {
        #[command(subcommand)]
        action: MigrationAction,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Trigger config reload
    Reload,

    /// Validate config file locally
    Validate {
        /// Path to config file to validate
        #[arg(short, long)]
        file: String,
    },
}

#[derive(Subcommand)]
pub enum BackupAction {
    /// List recent backups
    List,

    /// Trigger immediate backup
    Trigger,
}

#[derive(Subcommand)]
pub enum MigrationAction {
    /// Show migration status
    Status,

    /// Apply pending migrations
    Apply,

    /// Rollback last migration
    Rollback,
}

pub async fn execute(command: Command, client: &ManagementClient) -> Result<()> {
    match command {
        Command::Status => {
            let status = client.get_status().await?;
            println!("Shim Status:");
            println!("  Status:  {}", status.status);
            println!("  Version: {}", status.version);
            println!("  Uptime:  {}", status.uptime);
            println!("  Healthy: {}", status.healthy);
        }
        Command::Metrics => {
            let metrics = client.get_metrics().await?;
            println!("{}", serde_json::to_string_pretty(&metrics.metrics)?);
        }
        Command::Health => {
            println!("Liveness check:");
            match client.get_livez().await {
                Ok(livez) => {
                    println!("  Status:    {}", livez.status);
                    println!("  Timestamp: {}", livez.timestamp);
                }
                Err(e) => println!("  Error: {}", e),
            }

            println!("\nReadiness check:");
            match client.get_readyz().await {
                Ok(readyz) => {
                    println!("  Status:    {}", readyz.status);
                    println!("  Timestamp: {}", readyz.timestamp);
                }
                Err(e) => println!("  Error: {}", e),
            }
        }
        Command::Config { action } => match action {
            ConfigAction::Reload => {
                let resp = client.reload_config().await?;
                println!(
                    "Config reload: {}",
                    if resp.success { "OK" } else { "FAILED" }
                );
                println!("Message: {}", resp.message);
            }
            ConfigAction::Validate { file } => {
                let resp = client.validate_config(&file).await?;
                println!(
                    "Config validation: {}",
                    if resp.valid { "VALID" } else { "INVALID" }
                );
                if !resp.errors.is_empty() {
                    println!("Errors:");
                    for err in &resp.errors {
                        println!("  - {}", err);
                    }
                }
            }
        },
        Command::Backup { action } => match action {
            BackupAction::List => {
                let backups = client.list_backups().await?;
                if backups.is_empty() {
                    println!("No backups found.");
                } else {
                    println!("ID                  Created                   Size Status");
                    println!("{}", "-".repeat(70));
                    for b in &backups {
                        println!(
                            "{:<20} {:<25} {:>10} {}",
                            b.id, b.created_at, b.size_bytes, b.status
                        );
                    }
                }
            }
            BackupAction::Trigger => {
                let resp = client.trigger_backup().await?;
                println!("Backup triggered:");
                println!("  ID:     {}", resp.backup_id);
                println!("  Status: {}", resp.status);
            }
        },
        Command::Migration { action } => match action {
            MigrationAction::Status => {
                let status = client.get_migration_status().await?;
                println!("Migration Status:");
                println!("  Current version: {}", status.current_version);
                println!("  Applied: {}", status.applied.join(", "));
                println!("  Pending: {}", status.pending.join(", "));
            }
            MigrationAction::Apply => {
                let resp = client.apply_migrations().await?;
                println!(
                    "Migration apply: {}",
                    if resp.success { "OK" } else { "FAILED" }
                );
                println!("Applied: {}", resp.applied.join(", "));
                println!("Message: {}", resp.message);
            }
            MigrationAction::Rollback => {
                let resp = client.rollback_migration().await?;
                println!(
                    "Migration rollback: {}",
                    if resp.success { "OK" } else { "FAILED" }
                );
                println!("Rolled back: {}", resp.rolled_back);
                println!("Message: {}", resp.message);
            }
        },
    }
    Ok(())
}
