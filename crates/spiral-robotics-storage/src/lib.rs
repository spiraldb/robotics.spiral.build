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
pub use adbc::{ProjectStore, StorageConnection, is_unavailable_error};
pub use manager::StorageManager;
pub use schema::{
    ROBOT_CONFIGURATION_COLUMN, ROBOT_IDENTIFIER_COLUMN, ROBOT_UPDATED_MILLIS_COLUMN,
    SCHEMA_VERSION, is_dataset_id, is_identifier, robot_configuration_relation,
};
pub use settings::{
    BackendStateStore, SpiralConnectionSettings, SpiralCredentials, StoredStudioPreferences,
    StudioPreferencesStore, environment_spiral_endpoint, environment_spiral_spfs_endpoint,
    normalize_spiral_endpoint, normalize_spiral_spfs_endpoint, normalize_training_runtime_endpoint,
};
pub use sql::{drop_video_dataset, string_literal};
