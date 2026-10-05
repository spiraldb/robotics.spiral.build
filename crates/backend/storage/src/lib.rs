//! SQLite Studio state and Spiral data access, ingestion, and portable transfer.

pub mod curations;
mod datasets;
mod db;
mod manager;
mod provider_keys;
mod sessions;
mod settings;
mod spiral_access;
mod studio;
pub use studio::{StudioProject, StudioStore};
mod training;
mod transfer;

pub mod answers {
    //! Each dataset's table of stored model answers, which curation fills write.

    pub use crate::datasets::{ColumnAnswers, DatasetAnswers, EpisodeAnswer};
}

pub mod bundle {
    //! Portable snapshots containing Studio metadata and Spiral data.

    pub use crate::transfer::ProjectBundle;
}

pub mod checkpoints {
    //! Training checkpoints the worker commits to Spiral.

    pub use crate::training::{CheckpointArtifact, CheckpointKey};
}

pub mod training_runs {
    //! Studio's record of the training runs it started.

    pub use crate::training::TrainingRunRecord;
}

pub mod ingestion {
    //! Hugging Face discovery and Spiral dataset materialization.

    pub use crate::transfer::HuggingFaceDatasetDiscovery;
}

// Keep the cross-crate storage boundary flat and explicit. The modules behind
// these exports describe implementation mechanisms, not product features.
pub use db::AdbcDatabase;
pub(crate) use db::ProjectConnection;
pub use db::ProjectStore;
pub use db::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseResult, DatabaseSession, DatabaseTransaction, ExecutionSummary, IngestMode,
    RecordBatchStream, RelationName, SqlStatement,
};
pub use db::{SCHEMA_VERSION, STUDIO_TABLE_PREFIX};
pub use manager::StorageManager;
pub use provider_keys::ProviderKeys;
pub use sessions::{SessionStore, StoredSession};
pub use settings::{
    ServiceEndpoint, SpiralConnectionSettings, StoredStudioPreferences,
    StoredTrainingRuntimePreference,
};
pub use spiral_access::{SpiralAccess, SpiralAccessPublisher, SpiralConnection};
