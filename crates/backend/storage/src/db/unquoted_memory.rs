//! The driver's in-memory Spiral as tests reach it with Studio's statements.
//!
//! That engine differs from hosted Spiral three ways a test of more than one
//! statement meets: it finds a table ingestion made only by its unquoted
//! name (`datasets.demo.episodes`, never `"datasets"."demo"."episodes"`),
//! each connection has a catalog of its own, and a rollback keeps what it
//! wrote. So every project connection here shares one engine connection,
//! every statement reaches it with its double quotes taken out — a test names
//! its datasets with lowercase ids — and a transaction only counts its
//! commit.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arrow_schema::Schema;
use futures::FutureExt as _;
use tokio::sync::Mutex;

use super::database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseFuture, DatabaseSession,
    DatabaseTransaction, ExecutionSummary, RecordBatchStream, RelationName, SqlStatement,
};
use super::{AdbcDatabase, ProjectStore};

#[derive(Clone)]
pub(crate) struct UnquotedMemory {
    engine: Arc<Mutex<Box<dyn DatabaseConnection>>>,
    commits: Arc<AtomicUsize>,
}

impl UnquotedMemory {
    /// A project over a fresh in-memory engine, and how many transactions
    /// have committed in it.
    pub(crate) async fn store() -> (ProjectStore, Arc<AtomicUsize>) {
        Self::store_with_catalog("http://localhost:1").await
    }

    /// The same, its catalog read from the journal at `catalog`.
    pub(crate) async fn store_with_catalog(catalog: &str) -> (ProjectStore, Arc<AtomicUsize>) {
        let database = Self {
            engine: Arc::new(Mutex::new(
                AdbcDatabase::memory()
                    .connect()
                    .await
                    .expect("the in-memory engine"),
            )),
            commits: Arc::new(AtomicUsize::new(0)),
        };
        let commits = database.commits.clone();
        let settings =
            crate::SpiralConnectionSettings::new(catalog.into(), "http://localhost:2".into())
                .expect("a test endpoint")
                .with_project("contract")
                .expect("a fixed test project");
        let store = ProjectStore::new(
            crate::SpiralConnection::new(settings, crate::SpiralAccess::fixed("token".into())),
            Arc::new(database),
        )
        .expect("a fixed test catalog client");
        (store, commits)
    }

    fn unquoted(sql: SqlStatement) -> SqlStatement {
        let unquoted = SqlStatement::from(sql.as_str().replace('"', ""));
        match sql.parameters() {
            Some(parameters) => unquoted
                .with_parameters(parameters.clone())
                .expect("the statement's own parameters"),
            None => unquoted,
        }
    }
}

impl Database for UnquotedMemory {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        let connection = self.clone();
        async move { Ok(Box::new(connection) as Box<dyn DatabaseConnection>) }.boxed()
    }
}

impl DatabaseSession for UnquotedMemory {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        async move { self.engine.lock().await.query(Self::unquoted(sql)).await }.boxed()
    }

    fn execute_update(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            self.engine
                .lock()
                .await
                .execute_update(Self::unquoted(sql))
                .await
        }
        .boxed()
    }

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move { self.engine.lock().await.ingest(ingestion).await }.boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        async move { self.engine.lock().await.table_schema(relation).await }.boxed()
    }
}

impl DatabaseConnection for UnquotedMemory {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        async move { Ok(self as Box<dyn DatabaseTransaction>) }.boxed()
    }
}

impl DatabaseTransaction for UnquotedMemory {
    fn commit(self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        async { Ok(ExecutionSummary) }.boxed()
    }

    fn rollback(self: Box<Self>) -> DatabaseFuture<'static, ()> {
        async { Ok(()) }.boxed()
    }
}
