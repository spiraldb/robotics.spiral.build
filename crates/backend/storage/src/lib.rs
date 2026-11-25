//! SQLite Studio state and Spiral data access, ingestion, and portable transfer.

// The SDK/DataFusion catalog types exceed the default trait recursion depth.
#![recursion_limit = "256"]

pub mod curations;
mod datasets;
mod db;
mod ffmpeg;
mod manager;
mod provider_keys;
mod robot_connector_tokens;
mod sessions;
mod settings;
mod spiral_access;
mod staged_episodes;
mod studio;
pub use studio::{StudioProject, StudioStore};
mod training;
mod transfer;
mod workspaces;

pub mod answers {
    //! Each dataset's stored model answers, a table per answer column, which
    //! curation fills write.

    pub use crate::datasets::{ColumnAnswers, EpisodeAnswer, StatementAnswers, StoredAnswers};
}

pub mod bundle {
    //! Portable snapshots containing Studio metadata and Spiral data.

    pub use crate::transfer::ProjectBundle;
}

pub mod ingestion {
    //! Hugging Face discovery; [`crate::HuggingFaceImport`] imports what it
    //! finds.

    pub use crate::transfer::HuggingFaceDatasetDiscovery;
}

// Keep the cross-crate storage boundary flat and explicit. The modules behind
// these exports describe implementation mechanisms, not product features.
pub(crate) use db::ProjectConnection;
pub use db::ProjectStore;
pub use db::SpiralSdkDatabase;
pub use db::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseResult, DatabaseSession, DatabaseTransaction, ExecutionSummary, IngestMode,
    RecordBatchStream, RelationName, SqlStatement,
};
pub use db::{KeptInterval, SCHEMA_VERSION};
pub use ffmpeg::FfmpegExecutable;
pub use manager::StorageManager;
pub use provider_keys::ProviderKeys;
pub use robot_connector_tokens::RobotConnectorTokens;
pub use sessions::{SessionStore, StoredSession};
pub use settings::{ServiceEndpoint, SpiralConnectionSettings};
pub use spiral_access::{SpiralAccess, SpiralAccessPublisher, SpiralConnection};
pub use staged_episodes::{EpisodeName, EpisodeReceipt, StagedEpisodeError, StagedEpisodes};
pub use transfer::HuggingFaceImport;
pub use workspaces::WorkspaceStore;
