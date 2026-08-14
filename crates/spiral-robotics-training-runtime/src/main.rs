//! Lightweight remote PyTorch runtime served through HTTP and Arrow IPC.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "Serve Spiral Robotics policy training through HTTP and Arrow IPC")]
struct Args {
    /// Address used by Studio clients.
    #[arg(long, default_value = "0.0.0.0:50051")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("spiral_robotics_training=info")),
        )
        .init();
    tracing::info!(address = %args.bind, "HTTP and Arrow IPC training runtime ready");
    spiral_robotics_training_http::serve(
        args.bind,
        Arc::new(spiral_robotics_training_pytorch::PyTorchTrainingBackend::new()),
    )
    .await?;
    Ok(())
}
