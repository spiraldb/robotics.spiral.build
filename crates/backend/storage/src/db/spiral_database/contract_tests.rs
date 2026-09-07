use super::*;
use crate::db::contract_tests::DatabaseContract;

/// Real native execution with an offline connection-local catalog.
struct QueryDatabase {
    session: SpiralSession,
}

impl QueryDatabase {
    fn new() -> Self {
        let session = spiral_defaults::default_session().unwrap();
        let runtime = EngineRuntime::current(1).unwrap();
        Self {
            session: runtime.install_io(session),
        }
    }
}

impl Database for QueryDatabase {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        async move {
            Ok(Box::new(SpiralConnection::new(ConnectionCtx::new(
                self.session.clone(),
            ))) as Box<dyn DatabaseConnection>)
        }
        .boxed()
    }
}

#[tokio::test]
async fn native_query_stream_contract() {
    DatabaseContract::new(Arc::new(QueryDatabase::new()))
        .query_streams()
        .await
        .unwrap();
}

#[tokio::test]
async fn project_stream_keeps_its_operation_lease_after_connection_drop() {
    let settings = SpiralConnectionSettings::new(
        "http://localhost:1".to_owned(),
        "http://localhost:2".to_owned(),
    )
    .unwrap()
    .with_project("contract")
    .unwrap();
    let store = crate::ProjectStore::new(
        settings,
        "contract".to_owned(),
        Arc::new(QueryDatabase::new()),
    )
    .unwrap();
    let mut connection = store.connect().await.unwrap();
    let mut records = connection.query("SELECT 1".into()).await.unwrap();
    drop(connection);
    let mut exclusive = Box::pin(store.connect_exclusive());
    assert!(futures::poll!(&mut exclusive).is_pending());
    while records.next().await.transpose().unwrap().is_some() {}
    let connection = exclusive.await.unwrap();
    drop(connection);
    // Also release admission when a consumer stops before EOF.
    let mut connection = store.connect().await.unwrap();
    let records = connection.query("SELECT 2".into()).await.unwrap();
    drop(connection);
    let mut exclusive = Box::pin(store.connect_exclusive());
    assert!(futures::poll!(&mut exclusive).is_pending());
    drop(records);
    exclusive.await.unwrap();
}

// Keep the required behavior executable until Spiral exposes cancellation
// completion (or makes rollback await it). Re-enable after updating the driver
// and, if needed, wiring its completion handle into ConnectionCleanup.
#[tokio::test]
#[ignore = "upstream Spiral: dropped stream cancellation can outlive the rollback attempt"]
async fn dropped_transaction_waits_for_result_before_rollback() {
    use spiral_engine::sql::TransactionState;

    let database = QueryDatabase::new();
    let context = ConnectionCtx::new(database.session);
    let connection = Box::new(SpiralConnection::new(context.clone()));
    let mut transaction = connection.begin().await.unwrap();
    let native_transaction = context.active_transaction().unwrap();
    let records = transaction.query("SELECT 1".into()).await.unwrap();
    drop(transaction);
    assert_ne!(native_transaction.state(), TransactionState::RolledBack);
    drop(records);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while native_transaction.state() != TransactionState::RolledBack {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "cleanup timed out: {error}; state={:?}, active_queries={}",
            native_transaction.state(),
            native_transaction.active_query_count()
        )
    });
}

#[test]
fn empty_delete_normalization_does_not_hide_other_failures() {
    let delete = SqlStatement::from("DELETE FROM robotics.events WHERE id = 1");
    assert!(SpiralConnection::is_empty_delete(
        &delete,
        &"without any affected rows"
    ));
    assert!(!SpiralConnection::is_empty_delete(
        &delete,
        &"catalog access denied"
    ));
    assert!(!SpiralConnection::is_empty_delete(
        &SqlStatement::from("UPDATE robotics.events SET id = 1"),
        &"without any affected rows",
    ));
}
