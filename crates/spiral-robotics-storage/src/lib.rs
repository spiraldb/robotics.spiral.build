//! Project storage: SpiralDB access, schemas, queries, ingestion, and transfer.

mod adbc;
pub mod bundle;
pub mod checkpoints;
mod episode_schema;
pub mod episodes;
pub mod ingestion;
mod manager;
mod schema;
mod settings;
mod sql;
pub mod video;

// Keep the cross-crate storage boundary flat and explicit. The modules behind
// these exports describe implementation mechanisms, not product features.
pub use adbc::{ProjectStore, StorageConnection};
pub use manager::StorageManager;
pub use schema::{
    ROBOT_CONFIGURATION_COLUMN, ROBOT_IDENTIFIER_COLUMN, ROBOT_UPDATED_MILLIS_COLUMN,
    SCHEMA_VERSION, StorageSchema,
};
pub use settings::{
    BackendStateStore, ServiceEndpoint, SpiralConnectionSettings, SpiralCredentials,
    StoredStudioPreferences, StudioPreferencesStore,
};
pub use sql::SpiralSql;
