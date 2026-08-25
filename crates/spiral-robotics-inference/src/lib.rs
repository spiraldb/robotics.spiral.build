//! Framework-neutral inference contracts for robot policies.
//!
//! The public API intentionally contains no tensor-framework types. A caller
//! supplies borrowed observations and receives an owned action chunk, making
//! it possible for the robotics runtime to own scheduling, deadlines, and
//! safety independently from the model implementation.

mod artifact;
mod policy;

pub use artifact::ModelArtifact;
pub use policy::{
    ActionChunk, ActionChunkShape, ActionChunkSpecification, ImageDimensions, ImageFormat,
    ImageFrame, ImageInputSpecification, InferenceInput, InferenceStats, ObservationSpecification,
    Policy, PolicyKind, PolicyLoader, PolicySpecification,
};

/// Errors surfaced by artifact validation, model loading, or inference.
#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("invalid model artifact: {0}")]
    Artifact(String),
    #[error("invalid inference input: {0}")]
    Input(String),
    #[error("inference failed: {0}")]
    Inference(String),
}

pub type InferenceResult<T> = std::result::Result<T, InferenceError>;
