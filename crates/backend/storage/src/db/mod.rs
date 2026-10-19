//! Database mechanisms shared by the storage-owned features.
//!
//! This layer owns the database abstraction, the Spiral SDK adapter, project
//! connection lifecycle, SQL construction, physical schema, and Arrow row
//! decoding. Product persistence belongs to the feature modules above it.

mod arrow_rows;
mod database;
mod datafusion_sql;
mod project_store;
mod schema;
mod spiral_sdk;
mod sql;
mod staged_writes;

pub(crate) use arrow_rows::ArrowColumns;
pub use database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseResult, DatabaseSession, DatabaseTransaction, ExecutionSummary, IngestMode,
    RecordBatchStream, RelationName, SqlStatement,
};
pub(crate) use project_store::ProjectConnection;
pub use project_store::ProjectStore;
pub(crate) use project_store::{ProjectDatabasePersistence, StorageClock};
pub use schema::SCHEMA_VERSION;
pub(crate) use schema::Table;
pub use sql::KeptInterval;
pub(crate) use sql::ProjectQueries;

pub use spiral_sdk::SpiralSdkDatabase;

#[cfg(test)]
mod contract_tests;
#[cfg(any(test, feature = "test-support"))]
mod memory_database;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use memory_database::MemoryDatabase;
