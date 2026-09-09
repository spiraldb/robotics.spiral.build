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
    batches: Option<BoxStream<'static, DatabaseResult<RecordBatch>>>,
    retained: Vec<Box<dyn Send + Sync>>,
}

impl RecordBatchStream {
    pub fn new(
        schema: Arc<Schema>,
        batches: BoxStream<'static, DatabaseResult<RecordBatch>>,
    ) -> Self {
        Self {
            schema,
            batches: Some(batches),
            retained: Vec::new(),
        }
    }

    /// Keep resources alive until EOF, an error, or stream drop. The input
    /// stream is released before these resources so driver cleanup can finish.
    pub(crate) fn retain(mut self, resource: impl Send + Sync + 'static) -> Self {
        self.retained.push(Box::new(resource));
        self
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
        let Some(batches) = self.batches.as_mut() else {
            return std::task::Poll::Ready(None);
        };
        let next = batches.as_mut().poll_next(context);
        if matches!(next, std::task::Poll::Ready(None | Some(Err(_)))) {
            self.batches = None;
            self.retained.clear();
        }
        next
    }
}

/// Opens independent logical connections to one immutable database configuration.
/// Initialization and connection admission must not block the async executor.
pub trait Database: Send + Sync {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>>;
}

/// Operations shared by autocommit connections and explicit transactions.
///
/// Calls and stream polling must not block the async executor. Queries return
/// owned, bounded streams retaining their driver resources even after the
/// connection handle is dropped. EOF, an error, or drop releases the stream.
/// While a result is active, other operations return `InvalidState` instead of
/// waiting for that stream to be consumed. Separate connections are independent.
///
/// All operations use the caller's transaction, including ingestion and schema
/// inspection. Dropping an operation future stops waiting, but does not promise
/// cancellation of an in-flight write or rollback of a completed commit.
pub trait DatabaseSession: Send + 'static {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream>;

    fn execute_update(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, ExecutionSummary>;

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary>;

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>>;

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<RelationMetadata>>;
}

/// An independently stateful logical database connection.
pub trait DatabaseConnection: DatabaseSession {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>>;
}

/// An explicit transaction that has exactly one terminal action.
/// Dropping an unfinished transaction schedules rollback without blocking the
/// dropping thread. Cleanup waits for any active stream; resources cannot be
/// reused before cleanup completes. Driver failures remain observable errors.
pub trait DatabaseTransaction: DatabaseSession {
    fn commit(self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary>;

    fn rollback(self: Box<Self>) -> DatabaseFuture<'static, ()>;
}

/// Owned SQL and optional single-row Arrow parameters passed through the database boundary.
#[derive(Clone, Debug)]
pub struct SqlStatement {
    text: String,
    parameters: Option<RecordBatch>,
}

impl<T: Into<String>> From<T> for SqlStatement {
    fn from(sql: T) -> Self {
        Self {
            text: sql.into(),
            parameters: None,
        }
    }
}

impl SqlStatement {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn with_parameters(mut self, parameters: RecordBatch) -> DatabaseResult<Self> {
        if parameters.num_rows() != 1 {
            return Err(DatabaseError::invalid_input(
                "SQL parameters require exactly one row",
            ));
        }
        self.parameters = Some(parameters);
        Ok(self)
    }

    pub(crate) fn parameters(&self) -> Option<&RecordBatch> {
        self.parameters.as_ref()
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

    pub(crate) fn into_string(self) -> String {
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
    ReplaceExisting,
    ReplaceOrCreate,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutionSummary;

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
    Unsupported,
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

    pub(crate) fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(DatabaseErrorKind::InvalidInput, message)
    }

    pub(crate) fn invalid_state(message: impl Into<String>) -> Self {
        Self::new(DatabaseErrorKind::InvalidState, message)
    }

    pub fn kind(&self) -> DatabaseErrorKind {
        self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DatabaseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;

    #[tokio::test]
    async fn stream_error_is_terminal_and_releases_retained_resources() {
        let schema = Arc::new(Schema::empty());
        let resource = Arc::new(());
        let weak = Arc::downgrade(&resource);
        let mut records = RecordBatchStream::new(
            schema.clone(),
            futures::stream::iter([
                Err(DatabaseError::new(
                    DatabaseErrorKind::Unavailable,
                    "reader failed",
                )),
                Ok(RecordBatch::new_empty(schema)),
            ])
            .boxed(),
        )
        .retain(resource);
        assert!(weak.upgrade().is_some());
        assert_eq!(
            records.next().await.unwrap().unwrap_err().kind(),
            DatabaseErrorKind::Unavailable
        );
        assert!(weak.upgrade().is_none());
        assert!(records.next().await.is_none());
    }

    #[tokio::test]
    async fn pending_stream_keeps_resources_until_drop() {
        let resource = Arc::new(());
        let weak = Arc::downgrade(&resource);
        let mut records = RecordBatchStream::new(
            Arc::new(Schema::empty()),
            futures::stream::pending().boxed(),
        )
        .retain(resource);
        assert!(futures::poll!(records.next()).is_pending());
        assert!(weak.upgrade().is_some());
        drop(records);
        assert!(weak.upgrade().is_none());
    }
}
