//! LeRobot-compatible Action Chunking Transformer inference.

mod config;
mod engine;
mod layers;
mod model;
mod resnet;
mod weights;

pub use config::ActConfig;
pub use engine::ActPolicy;
#[cfg(feature = "cpu")]
pub use engine::{CpuActPolicy, load_cpu, load_cpu_artifact};
#[cfg(feature = "metal")]
pub use engine::{MetalActPolicy, load_metal, load_metal_artifact, metal_available};

/// Official LeRobot checkpoint used by examples and compatibility tests.
pub const REFERENCE_REPOSITORY: &str = "lerobot/act_aloha_sim_transfer_cube_human";
pub const REFERENCE_REVISION: &str = "ba73b2766f1371cdc133ca4efb97eb090d744625";
pub const REFERENCE_CONFIG_SHA256: &str =
    "f8bc4fa4ba67255cf7edfef9a20bc5374a53d6abeaad4cbe6201cf3b4889a9aa";
pub const REFERENCE_WEIGHTS_SHA256: &str =
    "42772891cb6eba1e7bc36ad8e12c0fa0723c61f036fa235c725ce6026e6e81df";

pub fn reference_model_files(
    directory: impl AsRef<std::path::Path>,
) -> crate::Result<crate::ModelFiles> {
    let identity = crate::ModelIdentity::pinned(
        REFERENCE_REPOSITORY,
        REFERENCE_REVISION,
        REFERENCE_CONFIG_SHA256,
        REFERENCE_WEIGHTS_SHA256,
    )?;
    crate::ModelFiles::from_directory_with_identity(directory, identity)
}

/// Fully content-pinned reference artifact used by the CLI and compatibility
/// tests. Both the source commit and downloaded file bytes are verified.
#[cfg(feature = "hub")]
pub fn reference_hub_model() -> crate::Result<crate::HubModel> {
    crate::HubModel::pinned(REFERENCE_REPOSITORY, REFERENCE_REVISION)?
        .with_checksums(REFERENCE_CONFIG_SHA256, REFERENCE_WEIGHTS_SHA256)
}
