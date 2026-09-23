//! SQLite Studio state and Spiral data access, ingestion, and portable transfer.

pub mod curations;
mod datasets;
mod db;
pub mod episodes;
pub mod explorations;
mod manager;
mod settings;
mod spiral_access;
mod studio;
pub use studio::{StudioProject, StudioStore};
mod training;
mod transfer;
pub mod workflow_assistant;
pub mod workflows;

pub mod bundle {
    //! Portable snapshots containing Studio metadata and Spiral data.

    pub use crate::transfer::ProjectBundle;
}

pub mod checkpoints {
    //! Training checkpoints the worker commits to Spiral.

    pub use crate::training::{
        CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointStore,
    };
}

pub mod training_runs {
    //! Studio's record of the training runs it started.

    pub use crate::training::TrainingRunRecord;
}

pub mod ingestion {
    //! Hugging Face discovery and Spiral dataset materialization.

    pub use crate::transfer::HuggingFaceDatasetDiscovery;
}

pub mod video {
    //! Persisted video values shared by camera capture and episode storage.

    pub use crate::episodes::video::*;
}

// Keep the cross-crate storage boundary flat and explicit. The modules behind
// these exports describe implementation mechanisms, not product features.
pub use db::{AdbcDatabase, SpiralSql};
pub use db::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseResult, DatabaseSession, DatabaseTransaction, ExecutionSummary, IngestMode,
    RecordBatchStream, RelationKind, RelationMetadata, RelationName, SqlStatement,
};
pub use db::{ProjectConnection, ProjectStore};
pub use db::{SCHEMA_VERSION, StorageSchema};
pub use manager::StorageManager;
pub use settings::{
    BackendStateStore, SavedConnection, ServiceEndpoint, SpiralConnectionSettings,
    StoredStudioPreferences, StoredTrainingRuntimePreference, StoredVideoPreference,
};
pub use spiral_access::{SpiralAccess, SpiralAccessPublisher, SpiralConnection};
