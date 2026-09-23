//! Training-run records in Studio and checkpoints in Spiral.

mod checkpoints;
mod training_runs;

pub(crate) use checkpoints::TrainingCheckpointPersistence;
pub use checkpoints::{CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointStore};
pub(crate) use training_runs::TrainingRunPersistence;
pub use training_runs::TrainingRunRecord;
