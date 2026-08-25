//! Framework-neutral inference contracts for robot policies.
//!
//! The public API intentionally contains no tensor-framework types. A caller
//! supplies borrowed observations and receives an owned action chunk, making
//! it possible for the robotics runtime to own scheduling, deadlines, and
//! safety independently from the model implementation.

mod artifact;
mod error;
mod policy;

pub use artifact::{ModelArtifact, ModelDigests, ModelFiles, ModelIdentity};
#[cfg(feature = "download")]
pub use artifact::{RemoteModel, RemoteModelUrls};
pub use error::{Error, Result};
pub use policy::{
    ActionChunk, ActionChunkShape, ActionChunkSpecification, ImageDimensions, ImageFormat,
    ImageFrame, ImageInputSpecification, InferenceInput, InferenceStats, ObservationSpecification,
    Policy, PolicyKind, PolicyLoader, PolicySpecification,
};
