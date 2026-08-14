//! Lightweight remote PyTorch runtime served through HTTP and Arrow IPC.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use spiral_robotics_inference::{ModelArtifact, Policy, PolicyLoader};
use tracing_subscriber::EnvFilter;

struct PyTorchPolicyLoader;

impl PolicyLoader for PyTorchPolicyLoader {
    fn load(&self, artifact: ModelArtifact) -> spiral_robotics_inference::Result<Box<dyn Policy>> {
        load_policy(artifact)
    }
}

#[cfg(target_os = "macos")]
fn load_policy(artifact: ModelArtifact) -> spiral_robotics_inference::Result<Box<dyn Policy>> {
    if !spiral_robotics_inference_pytorch::act::metal_available() {
        return spiral_robotics_inference_pytorch::act::load_cpu_artifact(artifact)
            .map(|policy| Box::new(policy) as Box<dyn Policy>);
    }
    let fallback = artifact.clone();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        spiral_robotics_inference_pytorch::act::load_metal_artifact(artifact)
    })) {
        Ok(Ok(policy)) => Ok(Box::new(policy)),
        Ok(Err(error)) => {
            tracing::warn!("PyTorch MPS policy unavailable ({error}); using CPU");
            spiral_robotics_inference_pytorch::act::load_cpu_artifact(fallback)
                .map(|policy| Box::new(policy) as Box<dyn Policy>)
        }
        Err(_) => {
            tracing::warn!("PyTorch MPS policy discovery panicked; using CPU");
            spiral_robotics_inference_pytorch::act::load_cpu_artifact(fallback)
                .map(|policy| Box::new(policy) as Box<dyn Policy>)
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn load_policy(artifact: ModelArtifact) -> spiral_robotics_inference::Result<Box<dyn Policy>> {
    spiral_robotics_inference_pytorch::act::load_cpu_artifact(artifact)
        .map(|policy| Box::new(policy) as Box<dyn Policy>)
}

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
        Arc::new(PyTorchPolicyLoader),
    )
    .await?;
    Ok(())
}
