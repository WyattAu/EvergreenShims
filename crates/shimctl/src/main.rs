use anyhow::Result;
use clap::Parser;
use shimctl::{Cli, ManagementClient};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = ManagementClient::new(&cli.endpoint)?;
    shimctl::commands::execute(cli.command, &client).await
}
