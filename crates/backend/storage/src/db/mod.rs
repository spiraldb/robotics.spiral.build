//! Database mechanisms shared by the storage-owned features.
//!
//! This layer owns the database abstraction, Spiral adapter, project
//! connection lifecycle, SQL construction, physical schema, and Arrow row
//! decoding. Product persistence belongs to the feature modules above it.

mod arrow_rows;
mod database;
mod project_store;
mod schema;
mod spiral_database;
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
pub(crate) use schema::{DatabaseBundleSection, ManagedRelationSchema, Table, column};
pub use schema::{
    ROBOT_CONFIGURATION_COLUMN, ROBOT_IDENTIFIER_COLUMN, ROBOT_UPDATED_MILLIS_COLUMN,
    SCHEMA_VERSION, StorageSchema,
};
pub(crate) use sql::ProjectQueries;
pub use sql::SpiralSql;

pub use spiral_database::SpiralDatabase;

#[cfg(test)]
mod contract_tests;
