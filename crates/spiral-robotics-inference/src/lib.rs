//! Native, standalone inference for robot policies.
//!
//! The public API intentionally contains no tensor-framework types. A caller
//! supplies borrowed observations and receives an owned action chunk, making
//! it possible for the robotics runtime to own scheduling, deadlines, and
//! safety independently from the model implementation.

mod artifact;
mod error;
mod interface;

pub mod act;

#[cfg(feature = "hub")]
pub use artifact::{HubModel, download_from_hub};
pub use artifact::{ModelArtifact, ModelFiles, ModelIdentity};
pub use error::{Error, Result};
pub use interface::{
    ActionChunk, ImageFormat, ImageInputSpecification, ImageView, InferenceInput, InferenceStats,
    Policy, PolicyKind, PolicySpecification,
};
