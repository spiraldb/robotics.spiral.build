//! Database mechanisms shared by the storage-owned features.
//!
//! This layer owns the database abstraction, Spiral adapter, project
//! connection lifecycle, SQL construction, physical schema, and Arrow row
//! decoding. Product persistence belongs to the feature modules above it.

mod adbc_database;
mod arrow_rows;
mod database;
mod project_store;
mod schema;
mod sql;

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
pub(crate) use schema::StorageSchema;
pub(crate) use schema::Table;
pub use sql::KeptInterval;
pub(crate) use sql::ProjectQueries;

pub use adbc_database::AdbcDatabase;

#[cfg(test)]
mod contract_tests;
