//! Training-run records in Studio and checkpoints in Spiral.

mod checkpoints;
mod training_runs;

pub(crate) use checkpoints::TrainingCheckpointPersistence;
pub use checkpoints::{CheckpointArtifact, CheckpointKey};
pub(crate) use training_runs::TrainingRunPersistence;
pub use training_runs::TrainingRunRecord;
