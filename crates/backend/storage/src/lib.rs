//! Project storage: SpiralDB access, schemas, queries, ingestion, and transfer.

mod db;
pub mod episodes;
mod manager;
mod native_datasets;
mod rollout_runs;
mod settings;
mod training;
mod transfer;
pub mod workflow_assistant;
pub mod workflow_documents;
pub mod workflow_features;

pub mod bundle {
    //! Portable single-file snapshots of the Studio-owned Spiral tables.

    pub use crate::transfer::ProjectBundle;
}

pub mod checkpoints {
    //! Application-owned boundary for immutable training artifacts.

    pub use crate::training::{
        CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointStore,
    };
}

pub mod ingestion {
    //! Hugging Face discovery and Spiral-native dataset materialization.

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
pub use db::{
    ROBOT_CONFIGURATION_COLUMN, ROBOT_IDENTIFIER_COLUMN, ROBOT_UPDATED_MILLIS_COLUMN,
    SCHEMA_VERSION, StorageSchema,
};
pub use manager::StorageManager;
pub use settings::{
    BackendStateStore, ServiceEndpoint, SpiralConnectionSettings, SpiralCredentials,
    StoredStudioPreferences, StoredTrainingRuntimePreference, StoredVideoPreference,
    StudioPreferencesStore,
};

pub(crate) const LEGACY_PROJECT_ERROR: &str = "this project was created by an older Studio and its storage format is no longer readable — start a fresh project";
