//! Studio's statements and writes on DataFusion's in-memory tables, for
//! tests: the SQL Spiral runs, on one set of tables every connection shares,
//! a transaction's writes held and folded as on Spiral until it commits.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arrow_schema::Schema;
use datafusion::catalog::MemTable;
use datafusion::common::TableReference;
use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use futures::FutureExt as _;

#[cfg(test)]
use super::ProjectStore;
use super::database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseFuture, DatabaseResult,
    DatabaseSession, DatabaseTransaction, ExecutionSummary, RecordBatchStream, RelationName,
    SqlStatement,
};
use super::staged_writes::{StagedRows, StagedWrite, StagedWrites};

/// Every table's rows, and how many transactions have committed.
#[derive(Clone, Default)]
pub(crate) struct MemoryDatabase {
    tables: Arc<Mutex<BTreeMap<RelationName, StagedRows>>>,
    commits: Arc<AtomicUsize>,
}

impl MemoryDatabase {
    /// A project over fresh tables, and how many transactions have committed
    /// in it.
    #[cfg(test)]
    pub(crate) async fn store() -> (ProjectStore, Arc<AtomicUsize>) {
        Self::store_with_catalog("http://localhost:1").await
    }

    /// The same, its catalog read from the journal at `catalog`.
    #[cfg(test)]
    pub(crate) async fn store_with_catalog(catalog: &str) -> (ProjectStore, Arc<AtomicUsize>) {
        let database = Self::default();
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

    /// A session over every table as it is now, each named by its whole name.
    fn session(&self) -> DatabaseResult<SessionContext> {
        let mut config = SessionConfig::new();
        config.options_mut().sql_parser.enable_ident_normalization = false;
        let context = SessionContext::new_with_state(
            SessionStateBuilder::new()
                .with_default_features()
                .with_config(config)
                .build(),
        );
        for (relation, rows) in self.tables.lock().expect("tables").iter() {
            let table = MemTable::try_new(rows.schema().clone(), vec![rows.batches().to_vec()])
                .map_err(DatabaseError::from_datafusion)?;
            context
                .register_table(TableReference::bare(relation.as_str()), Arc::new(table))
                .map_err(DatabaseError::from_datafusion)?;
        }
        Ok(context)
    }

    fn apply(&self, writes: StagedWrites) {
        let mut tables = self.tables.lock().expect("tables");
        for (relation, write) in writes.into_writes() {
            match write {
                StagedWrite::Append(rows) => {
                    let rows = match tables.remove(&relation) {
                        Some(held) => held.extended(rows),
                        None => rows,
                    };
                    tables.insert(relation, rows);
                }
                StagedWrite::Replace(rows) => {
                    tables.insert(relation, rows);
                }
                StagedWrite::Drop => {
                    tables.remove(&relation);
                }
            }
        }
    }

    fn schema(&self, relation: &RelationName) -> Option<Arc<Schema>> {
        let tables = self.tables.lock().expect("tables");
        tables.get(relation).map(|rows| rows.schema().clone())
    }

    async fn query(&self, sql: SqlStatement) -> DatabaseResult<RecordBatchStream> {
        sql.stream_on(&self.session()?).await
    }
}

impl Database for MemoryDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        let connection = self.clone();
        async move { Ok(Box::new(connection) as Box<dyn DatabaseConnection>) }.boxed()
    }
}

impl DatabaseSession for MemoryDatabase {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        async move { MemoryDatabase::query(self, sql).await }.boxed()
    }

    fn drop_table(&mut self, relation: RelationName) -> DatabaseFuture<'_, ExecutionSummary> {
        let mut writes = StagedWrites::default();
        writes.drop_table(relation);
        self.apply(writes);
        async { Ok(ExecutionSummary) }.boxed()
    }

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            let mut writes = StagedWrites::default();
            writes.stage(ingestion).await?;
            self.apply(writes);
            Ok(ExecutionSummary)
        }
        .boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        let schema = self.schema(&relation);
        async move { Ok(schema) }.boxed()
    }
}

impl DatabaseConnection for MemoryDatabase {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        async move {
            Ok(Box::new(MemoryTransaction {
                database: *self,
                writes: StagedWrites::default(),
            }) as Box<dyn DatabaseTransaction>)
        }
        .boxed()
    }
}

/// A transaction over the in-memory tables, its writes held until it commits.
struct MemoryTransaction {
    database: MemoryDatabase,
    writes: StagedWrites,
}

impl DatabaseSession for MemoryTransaction {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        async move { self.database.query(sql).await }.boxed()
    }

    fn drop_table(&mut self, relation: RelationName) -> DatabaseFuture<'_, ExecutionSummary> {
        self.writes.drop_table(relation);
        async { Ok(ExecutionSummary) }.boxed()
    }

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            self.writes.stage(ingestion).await?;
            Ok(ExecutionSummary)
        }
        .boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        let schema = match self.writes.schema(&relation) {
            Some(held) => held,
            None => self.database.schema(&relation),
        };
        async move { Ok(schema) }.boxed()
    }
}

impl DatabaseTransaction for MemoryTransaction {
    fn commit(self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary> {
        self.database.apply(self.writes);
        self.database.commits.fetch_add(1, Ordering::SeqCst);
        async { Ok(ExecutionSummary) }.boxed()
    }

    fn rollback(self: Box<Self>) -> DatabaseFuture<'static, ()> {
        async { Ok(()) }.boxed()
    }
}
