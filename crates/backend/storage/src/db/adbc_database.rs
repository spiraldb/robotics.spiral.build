//! Spiral's shared-library driver behind Studio's async database contract.

mod catalog;
mod driver;
mod streaming;
mod worker;

use std::sync::Arc;

use adbc_core::options::OptionConnection;
use adbc_core::{Connection as _, Optionable as _, Statement as _};
use arrow_schema::Schema;
use futures::FutureExt as _;
use tokio::sync::{OnceCell, OwnedSemaphorePermit, Semaphore};

use super::database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseFuture, DatabaseResult,
    DatabaseSession, DatabaseTransaction, ExecutionSummary, RecordBatchStream, RelationMetadata,
    RelationName, SqlStatement,
};
use crate::settings::SpiralConnectionSettings;
use driver::{DriverConfiguration, LoadedDatabase};
use worker::ConnectionWorker;

/// Lazily loads a standalone Spiral ADBC library. No driver work runs in `new`.
pub struct AdbcDatabase {
    configuration: DriverConfiguration,
    database: OnceCell<Arc<LoadedDatabase>>,
}

impl AdbcDatabase {
    pub fn new(settings: SpiralConnectionSettings) -> Self {
        Self {
            configuration: DriverConfiguration::hosted(settings),
            database: OnceCell::new(),
        }
    }
}

impl Database for AdbcDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        async move {
            let database = self
                .database
                .get_or_try_init(|| async {
                    let configuration = self.configuration.clone();
                    tokio::task::spawn_blocking(move || configuration.load())
                        .await
                        .map_err(DatabaseError::from_worker)?
                        .map(Arc::new)
                })
                .await?
                .clone();
            let worker = ConnectionWorker::connect(database).await?;
            Ok(Box::new(AdbcConnection::new(worker)) as Box<dyn DatabaseConnection>)
        }
        .boxed()
    }
}

struct AdbcConnection {
    worker: Arc<ConnectionWorker>,
    result_slot: Arc<Semaphore>,
}

impl AdbcConnection {
    fn new(worker: ConnectionWorker) -> Self {
        Self {
            worker: Arc::new(worker),
            result_slot: Arc::new(Semaphore::new(1)),
        }
    }

    fn reserve_operation(&self) -> DatabaseResult<OwnedSemaphorePermit> {
        self.result_slot.clone().try_acquire_owned().map_err(|_| {
            DatabaseError::invalid_state(
                "connection has an active operation or result stream; consume or drop it first",
            )
        })
    }
}

impl DatabaseSession for AdbcConnection {
    fn query(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, RecordBatchStream> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker
                .query(sql)
                .await
                .map(|records| records.retain(self.worker.clone()).retain(permit))
        }
        .boxed()
    }

    fn query_schema(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, Arc<Schema>> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker
                .run(move |state| {
                    let _permit = permit;
                    let mut statement = state.connection().new_statement()?;
                    statement.set_sql_query(sql.as_str())?;
                    if let Some(parameters) = sql.parameters() {
                        statement.prepare()?;
                        statement.bind(parameters.clone())?;
                    }
                    Ok(Arc::new(statement.execute_schema()?))
                })
                .await
        }
        .boxed()
    }

    fn execute_update(&mut self, sql: SqlStatement) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker
                .run(move |state| {
                    let _permit = permit;
                    let mut statement = state.connection().new_statement()?;
                    statement.set_sql_query(sql.as_str())?;
                    if let Some(parameters) = sql.parameters() {
                        statement.prepare()?;
                        statement.bind(parameters.clone())?;
                    }
                    statement.execute_update()?;
                    Ok(ExecutionSummary)
                })
                .await
        }
        .boxed()
    }

    fn ingest(&mut self, ingestion: BatchIngestion) -> DatabaseFuture<'_, ExecutionSummary> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker.ingest(ingestion, permit).await
        }
        .boxed()
    }

    fn table_schema(&mut self, relation: RelationName) -> DatabaseFuture<'_, Option<Arc<Schema>>> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker
                .run(move |state| {
                    let _permit = permit;
                    state.table_schema(&relation)
                })
                .await
        }
        .boxed()
    }

    fn relations(&mut self) -> DatabaseFuture<'_, Vec<RelationMetadata>> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker
                .run(move |state| {
                    let _permit = permit;
                    state.relations()
                })
                .await
        }
        .boxed()
    }
}

impl DatabaseConnection for AdbcConnection {
    fn begin(self: Box<Self>) -> DatabaseFuture<'static, Box<dyn DatabaseTransaction>> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker
                .run(move |state| {
                    let _permit = permit;
                    state
                        .connection()
                        .set_option(OptionConnection::AutoCommit, "false".into())?;
                    state.set_transaction(true);
                    Ok(())
                })
                .await?;
            Ok(self as Box<dyn DatabaseTransaction>)
        }
        .boxed()
    }
}

impl DatabaseTransaction for AdbcConnection {
    fn commit(self: Box<Self>) -> DatabaseFuture<'static, ExecutionSummary> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker
                .run(move |state| {
                    let _permit = permit;
                    state.connection().commit()?;
                    state.set_transaction(false);
                    Ok(ExecutionSummary)
                })
                .await
        }
        .boxed()
    }

    fn rollback(self: Box<Self>) -> DatabaseFuture<'static, ()> {
        async move {
            let permit = self.reserve_operation()?;
            self.worker
                .run(move |state| {
                    let _permit = permit;
                    state.connection().rollback()?;
                    state.set_transaction(false);
                    Ok(())
                })
                .await
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
impl AdbcDatabase {
    pub(crate) fn memory() -> Self {
        Self {
            configuration: DriverConfiguration::memory(),
            database: OnceCell::new(),
        }
    }
}
