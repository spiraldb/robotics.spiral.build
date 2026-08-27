//! Async database capabilities shared by storage features.
//!
//! The backend sees SQL and Arrow values through these traits. Driver sessions,
//! engine relations, runtime handles, and other implementation types remain in
//! the adapter that implements [`Database`].

use std::fmt;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use futures::future::BoxFuture;
use futures::stream::{BoxStream, Stream};

pub type DatabaseResult<T> = Result<T, DatabaseError>;
pub type DatabaseFuture<'a, T> = BoxFuture<'a, DatabaseResult<T>>;

/// A fallible Arrow stream that retains its schema even when it has no rows.
pub struct RecordBatchStream {
    schema: Arc<Schema>,
    batches: BoxStream<'static, DatabaseResult<RecordBatch>>,
}

impl RecordBatchStream {
    pub fn new(
        schema: Arc<Schema>,
        batches: BoxStream<'static, DatabaseResult<RecordBatch>>,
    ) -> Self {
        Self { schema, batches }
    }

    pub fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }
}

impl Stream for RecordBatchStream {
    type Item = DatabaseResult<RecordBatch>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.batches.as_mut().poll_next(context)
    }
}

/// Opens independent logical connections to one configured database.
pub trait Database: Send + Sync {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>>;
}

/// Operations shared by autocommit connections and explicit transactions.
pub trait DatabaseSession: Send + 'static {
    fn execute(&mut self, command: DatabaseCommand) -> DatabaseFuture<'_, DatabaseOutput>;

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>>;

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<RelationMetadata>>;
}

/// An independently stateful logical database connection.
pub trait DatabaseConnection: DatabaseSession {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>>;
}

/// An explicit transaction that has exactly one terminal action.
pub trait DatabaseTransaction: DatabaseSession {
    fn commit(self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary>;

    fn rollback(self: Box<Self>) -> DatabaseFuture<'static, ()>;
}

/// One database operation. SQL and Arrow ingestion share the same execution
/// boundary even though their inputs differ.
pub enum DatabaseCommand {
    Statement(SqlStatement),
    Ingest(BatchIngestion),
}

impl DatabaseCommand {
    pub fn statement(sql: impl Into<String>) -> Self {
        Self::Statement(SqlStatement::new(sql))
    }

    pub fn ingest(ingestion: BatchIngestion) -> Self {
        Self::Ingest(ingestion)
    }
}

/// The only two outcomes visible across the database boundary.
pub enum DatabaseOutput {
    Records(RecordBatchStream),
    AffectedRows(ExecutionSummary),
}

impl DatabaseOutput {
    pub fn into_records(self) -> DatabaseResult<RecordBatchStream> {
        match self {
            Self::Records(records) => Ok(records),
            Self::AffectedRows(_) => Err(DatabaseError::invalid_state(
                "database command did not produce records",
            )),
        }
    }

    pub fn into_summary(self) -> DatabaseResult<ExecutionSummary> {
        match self {
            Self::AffectedRows(summary) => Ok(summary),
            Self::Records(_) => Err(DatabaseError::invalid_state(
                "database command produced records instead of an update count",
            )),
        }
    }
}

/// Owned SQL text passed through the async database boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlStatement(String);

impl SqlStatement {
    pub fn new(sql: impl Into<String>) -> Self {
        Self(sql.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

/// Validated dotted relation path.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelationName(String);

impl RelationName {
    pub fn parse(name: impl Into<String>) -> DatabaseResult<Self> {
        let name = name.into();
        if name.is_empty() || name.split('.').any(str::is_empty) {
            return Err(DatabaseError::invalid_input(
                "relation names need non-empty dot-separated components",
            ));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

/// Bulk Arrow input and its destination behavior.
pub struct BatchIngestion {
    relation: RelationName,
    mode: IngestMode,
    batches: RecordBatchStream,
}

impl BatchIngestion {
    pub fn new(relation: RelationName, mode: IngestMode, batches: RecordBatchStream) -> Self {
        Self {
            relation,
            mode,
            batches,
        }
    }

    pub fn into_parts(self) -> (RelationName, IngestMode, RecordBatchStream) {
        (self.relation, self.mode, self.batches)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngestMode {
    Append,
    CreateOrAppend,
    Replace,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutionSummary {
    affected_rows: Option<u64>,
}

impl ExecutionSummary {
    pub const fn new(affected_rows: Option<u64>) -> Self {
        Self { affected_rows }
    }

    pub const fn affected_rows(self) -> Option<u64> {
        self.affected_rows
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationMetadata {
    name: RelationName,
    kind: RelationKind,
}

impl RelationMetadata {
    pub fn new(name: RelationName, kind: RelationKind) -> Self {
        Self { name, kind }
    }

    pub fn into_parts(self) -> (RelationName, RelationKind) {
        (self.name, self.kind)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationKind {
    Table,
    View,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseErrorKind {
    Unavailable,
    InvalidInput,
    InvalidState,
    NotFound,
    Conflict,
    Cancelled,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseError {
    kind: DatabaseErrorKind,
    message: String,
}

impl DatabaseError {
    pub fn new(kind: DatabaseErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(DatabaseErrorKind::InvalidInput, message)
    }

    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::new(DatabaseErrorKind::InvalidState, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(DatabaseErrorKind::Internal, message)
    }

    pub const fn kind(&self) -> DatabaseErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DatabaseError {}
