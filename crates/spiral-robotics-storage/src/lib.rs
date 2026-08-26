//! Project storage: SpiralDB access, schemas, queries, ingestion, and transfer.

pub mod bundle;
pub mod checkpoints;
mod database;
mod episode_schema;
pub mod episodes;
pub mod ingestion;
mod manager;
mod project_store;
mod schema;
mod settings;
mod spiral_database;
mod sql;
pub mod video;

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
    StoredStudioPreferences, StudioPreferencesStore,
};
pub use sql::SpiralSql;
