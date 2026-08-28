//! Project storage: SpiralDB access, schemas, queries, ingestion, and transfer.

mod arrow_rows;
pub mod bundle;
pub mod checkpoints;
mod database;
mod episode_schema;
pub mod episodes;
pub mod ingestion;
mod manager;
mod project_store;
mod rollout_runs;
mod schema;
mod settings;
mod spiral_database;
mod sql;
mod training_runs;
pub mod video;
pub mod workflow_documents;

// Keep the cross-crate storage boundary flat and explicit. The modules behind
// these exports describe implementation mechanisms, not product features.
pub use database::{
    BatchIngestion, Database, DatabaseCommand, DatabaseConnection, DatabaseError,
    DatabaseErrorKind, DatabaseFuture, DatabaseOutput, DatabaseResult, DatabaseSession,
    DatabaseTransaction, ExecutionSummary, IngestMode, RecordBatchStream, RelationKind,
    RelationMetadata, RelationName, SqlStatement,
};
pub use manager::StorageManager;
pub use project_store::{ProjectConnection, ProjectStore};
pub use schema::{
    ROBOT_CONFIGURATION_COLUMN, ROBOT_IDENTIFIER_COLUMN, ROBOT_UPDATED_MILLIS_COLUMN,
    SCHEMA_VERSION, StorageSchema,
};
pub use settings::{
    BackendStateStore, ServiceEndpoint, SpiralConnectionSettings, SpiralCredentials,
    StoredStudioPreferences, StoredTrainingRuntimePreference, StoredVideoPreference,
    StudioPreferencesStore,
};
pub use sql::SpiralSql;

pub(crate) const LEGACY_PROJECT_ERROR: &str = "this project was created by an older Studio and its storage format is no longer readable — start a fresh project";
