use clap::Parser;
use management_api::{start_server, ShimState};
use std::net::SocketAddr;

#[derive(Parser)]
#[command(name = "management-api")]
#[command(about = "gRPC Management API for EvergreenShim")]
#[command(version)]
struct Args {
    /// Address to bind the gRPC server
    #[arg(short, long, default_value = "0.0.0.0:50051")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Initialize logging
    let _ = shim_core::structured_logging::init_structured_logging(true, false);

    tracing::info!("Starting EvergreenShim Management API");

    let state = ShimState::new();

    start_server(args.listen, state).await
}
