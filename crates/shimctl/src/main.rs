use anyhow::Result;
use clap::Parser;
use shimctl::{Cli, ManagementClient};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let shimctl::commands::Command::Completion { shell } = &cli.command {
        let mut cmd = <shimctl::Cli as clap::CommandFactory>::command();
        clap_complete::generate(*shell, &mut cmd, "shimctl", &mut std::io::stdout());
        return Ok(());
    }

    let client = ManagementClient::new(&cli.endpoint)?;
    shimctl::commands::execute(cli.command, &client).await
}
