use std::sync::Arc;

use adbc_core::{Connection as _, Database as _, Statement as _};
use adbc_driver_manager::ManagedConnection;
use futures::StreamExt as _;
use tokio::sync::{Mutex, OwnedSemaphorePermit, mpsc, oneshot};

use super::catalog::IngestionOperation;
use super::driver::LoadedDatabase;
use super::streaming::{DriverBuffers, IngestionReader};
use super::{
    BatchIngestion, DatabaseError, DatabaseResult, ExecutionSummary, RecordBatchStream,
    SqlStatement,
};

const BATCH_CAPACITY: usize = 2;

/// One logical connection; idle connections occupy no worker threads.
/// The async mutex admits blocking work only after the previous worker exits.
pub(super) struct ConnectionWorker {
    state: Arc<Mutex<Option<ConnectionState>>>,
    runtime: tokio::runtime::Handle,
    #[cfg(test)]
    cleanup_result: Option<oneshot::Sender<DatabaseResult<()>>>,
}

pub(super) struct ConnectionState {
    connection: ManagedConnection,
    database: Arc<LoadedDatabase>,
    transaction: bool,
}

impl ConnectionState {
    pub(super) fn connection(&mut self) -> &mut ManagedConnection {
        &mut self.connection
    }
    pub(super) fn set_transaction(&mut self, active: bool) {
        self.transaction = active;
    }

    fn rollback_abandoned(&mut self) -> DatabaseResult<()> {
        if self.transaction {
            // TODO(Spiral): releasing an Arrow reader currently requests engine
            // cancellation without exposing its completion. Upstream must make
            // release/rollback coordinate before this can guarantee cleanup.
            // Never poll, retry, sleep, or drain results to hide that gap.
            self.connection.rollback()?;
            self.transaction = false;
        }
        Ok(())
    }
}

impl ConnectionWorker {
    pub(super) async fn connect(database: Arc<LoadedDatabase>) -> DatabaseResult<Self> {
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let connection = database.database().new_connection()?;
            Ok(Self {
                state: Arc::new(Mutex::new(Some(ConnectionState {
                    connection,
                    database,
                    transaction: false,
                }))),
                runtime,
                #[cfg(test)]
                cleanup_result: None,
            })
        })
        .await
        .map_err(DatabaseError::from_worker)?
    }

    pub(super) async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut ConnectionState) -> DatabaseResult<T> + Send + 'static,
    ) -> DatabaseResult<T> {
        let mut state = self.state.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || operation(state.as_mut().expect("live connection")))
            .await
            .map_err(DatabaseError::from_worker)?
    }

    pub(super) async fn query(&self, sql: SqlStatement) -> DatabaseResult<RecordBatchStream> {
        let mut state = self.state.clone().lock_owned().await;
        let (schema_sender, schema_receiver) = oneshot::channel();
        let (batches_sender, batches_receiver) = mpsc::channel(BATCH_CAPACITY);
        let worker = tokio::task::spawn_blocking(move || -> DatabaseResult<()> {
            let state = state.as_mut().expect("live connection");
            let mut statement = state.connection.new_statement()?;
            statement.set_sql_query(sql.as_str())?;
            let mut reader = statement.execute()?;
            if schema_sender.send(reader.schema()).is_err() {
                return Ok(());
            }
            // One blocking job drives the whole result, with bounded backpressure.
            // Keep the statement, connection, and library alive through reader drop.
            while !batches_sender.is_closed() {
                let Some(batch) = reader.next() else {
                    break;
                };
                let batch = batch
                    .map_err(DatabaseError::from_arrow)
                    .map(|batch| DriverBuffers::retain(batch, state.database.clone()));
                let failed = batch.is_err();
                if batches_sender.blocking_send(batch).is_err() || failed {
                    break;
                }
            }
            drop(reader);
            drop(statement);
            Ok(())
        });
        let schema = match schema_receiver.await {
            Ok(schema) => schema,
            Err(_) => {
                worker.await.map_err(DatabaseError::from_worker)??;
                return Err(DatabaseError::invalid_state(
                    "query ended before returning its schema",
                ));
            }
        };
        let batches = futures::stream::try_unfold(
            (batches_receiver, Some(worker)),
            |(mut receiver, mut worker)| async move {
                if let Some(batch) = receiver.recv().await {
                    return batch.map(|batch| Some((batch, (receiver, worker))));
                }
                // EOF is published only after driver resources have been released;
                // worker panics must not silently become successful query EOF.
                if let Some(worker) = worker.take() {
                    worker.await.map_err(DatabaseError::from_worker)??;
                }
                Ok(None)
            },
        )
        .boxed();
        Ok(RecordBatchStream::new(schema, batches))
    }

    pub(super) async fn ingest(
        &self,
        ingestion: BatchIngestion,
        permit: OwnedSemaphorePermit,
    ) -> DatabaseResult<ExecutionSummary> {
        let (relation, mode, batches) = ingestion.into_parts();
        let reader = IngestionReader::new(batches);
        self.run(move |state| {
            let _permit = permit;
            IngestionOperation::new(relation, mode, reader).execute(state)
        })
        .await
    }
}

impl Drop for ConnectionWorker {
    fn drop(&mut self) {
        let state = self.state.clone();
        #[cfg(test)]
        let cleanup_result = self.cleanup_result.take();
        // The stream and any in-flight operation hold the mutex until their
        // reader/statement is released. Never block the dropping thread.
        self.runtime.spawn(async move {
            let mut state = state.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if let Some(mut connection) = state.take() {
                    let result = connection.rollback_abandoned();
                    if let Err(error) = &result {
                        tracing::warn!(%error, "could not clean up ADBC database connection");
                    }
                    drop(connection);
                    #[cfg(test)]
                    if let Some(cleanup_result) = cleanup_result {
                        let _ = cleanup_result.send(result);
                    }
                }
            })
            .await
            .ok();
        });
    }
}

#[cfg(test)]
impl ConnectionWorker {
    pub(super) fn observe_cleanup(&mut self) -> oneshot::Receiver<DatabaseResult<()>> {
        let (sender, receiver) = oneshot::channel();
        self.cleanup_result = Some(sender);
        receiver
    }
}
