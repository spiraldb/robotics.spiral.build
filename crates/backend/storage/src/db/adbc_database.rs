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
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, watch};

use super::database::{
    BatchIngestion, Database, DatabaseConnection, DatabaseError, DatabaseFuture, DatabaseResult,
    DatabaseSession, DatabaseTransaction, ExecutionSummary, RecordBatchStream, RelationName,
    SqlStatement,
};
use crate::{ProviderKeys, SpiralConnection};
use driver::{DriverConfiguration, LoadedDatabase};
use worker::ConnectionWorker;

/// The driver database a new connection opens on, and the provider keys it
/// was created with.
struct OpenedDatabase {
    keys: ProviderKeys,
    database: Arc<LoadedDatabase>,
}

/// Lazily loads a standalone Spiral ADBC library. No driver work runs in `new`.
pub struct AdbcDatabase {
    configuration: DriverConfiguration,
    keys: Option<watch::Receiver<ProviderKeys>>,
    opened: Mutex<Option<OpenedDatabase>>,
}

impl AdbcDatabase {
    pub fn new(connection: SpiralConnection) -> Self {
        Self::of(DriverConfiguration::hosted(connection))
    }

    fn of(configuration: DriverConfiguration) -> Self {
        Self {
            configuration,
            keys: None,
            opened: Mutex::new(None),
        }
    }

    /// The same database, handing the driver the provider keys `keys` holds.
    /// The driver takes them only when a database is created, so every new
    /// connection reads the current keys.
    pub fn with_provider_keys(mut self, keys: watch::Receiver<ProviderKeys>) -> Self {
        self.keys = Some(keys);
        self
    }

    /// The driver database for a new connection: the open one while the keys
    /// are the ones it was created with, else a new one. Connections already
    /// open keep the database they opened on until they close.
    async fn current(&self) -> DatabaseResult<Arc<LoadedDatabase>> {
        let keys = self
            .keys
            .as_ref()
            .map(|keys| keys.borrow().clone())
            .unwrap_or_default();
        let mut opened = self.opened.lock().await;
        if let Some(open) = opened.as_ref().filter(|open| open.keys == keys) {
            return Ok(open.database.clone());
        }
        let configuration = self.configuration.clone().with_provider_keys(&keys);
        let database = tokio::task::spawn_blocking(move || configuration.load())
            .await
            .map_err(DatabaseError::from_worker)??;
        *opened = Some(OpenedDatabase {
            keys,
            database: database.clone(),
        });
        Ok(database)
    }
}

impl Database for AdbcDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        async move {
            let worker = ConnectionWorker::connect(self.current().await?).await?;
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
        Self::of(DriverConfiguration::memory())
    }
}
