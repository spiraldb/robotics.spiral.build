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

pub(crate) use arrow_rows::{
    ArrowColumns, ListColumnBuilder, OptionalFloatListColumn, StringListColumn, UnsignedListColumn,
};
pub use database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
    DatabaseResult, DatabaseSession, DatabaseTransaction, ExecutionSummary, IngestMode,
    RecordBatchStream, RelationKind, RelationMetadata, RelationName, SqlStatement,
};
pub use project_store::{ProjectConnection, ProjectStore};
pub(crate) use project_store::{ProjectDatabasePersistence, StorageClock};
pub use schema::{SCHEMA_VERSION, StorageSchema};
pub(crate) use schema::{Table, column};
pub(crate) use sql::ProjectQueries;
pub use sql::SpiralSql;

pub use adbc_database::AdbcDatabase;

#[cfg(test)]
mod contract_tests;
