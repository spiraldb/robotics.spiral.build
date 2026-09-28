use super::*;
use crate::SpiralConnectionSettings;
use crate::db::DatabaseErrorKind;
use crate::db::contract_tests::DatabaseContract;
use arrow_array::{Array as _, Int64Array, RecordBatch, StringViewArray};
use futures::{StreamExt as _, TryStreamExt as _};
use serde::Deserialize;

#[tokio::test]
async fn bound_parameters_preserve_arrow_types_and_connection_reuse() {
    let database = AdbcDatabase::memory();
    let mut connection = database.connect().await.unwrap();
    let schema = Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
        "id",
        arrow_schema::DataType::Int64,
        true,
    )]));
    for expected in [Some(42), Some(73), None] {
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![expected]))],
        )
        .unwrap();
        let statement = SqlStatement::from("SELECT $1 AS id")
            .with_parameters(batch)
            .unwrap();
        let records = connection
            .query(statement)
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        let values = records[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(values.iter().collect::<Vec<_>>(), vec![expected]);
    }
    for rows in [0, 2] {
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![Some(1); rows]))],
        )
        .unwrap();
        assert!(
            SqlStatement::from("SELECT $1")
                .with_parameters(batch)
                .is_err()
        );
    }
}

#[tokio::test]
async fn adbc_query_stream_contract() {
    DatabaseContract::new(Arc::new(AdbcDatabase::memory()))
        .query_streams()
        .await
        .unwrap();
}

#[tokio::test]
async fn project_stream_keeps_its_operation_lease_after_connection_drop() {
    let settings =
        SpiralConnectionSettings::new("http://localhost:1".into(), "http://localhost:2".into())
            .unwrap()
            .with_project("contract")
            .unwrap();
    let store = crate::ProjectStore::new(
        crate::SpiralConnection::new(settings, crate::SpiralAccess::fixed("token".into())),
        Arc::new(AdbcDatabase::memory()),
    )
    .unwrap();
    let mut connection = store.connect().await.unwrap();
    let mut records = connection.query("SELECT 1".into()).await.unwrap();
    drop(connection);
    let mut exclusive = Box::pin(store.connect_exclusive());
    assert!(futures::poll!(&mut exclusive).is_pending());
    while records.next().await.transpose().unwrap().is_some() {}
    drop(exclusive.await.unwrap());
    let mut connection = store.connect().await.unwrap();
    let records = connection.query("SELECT 2".into()).await.unwrap();
    drop(connection);
    let mut exclusive = Box::pin(store.connect_exclusive());
    assert!(futures::poll!(&mut exclusive).is_pending());
    drop(records);
    exclusive.await.unwrap();
}

#[tokio::test]
async fn result_columns_retain_the_driver_after_database_drop() {
    let database = AdbcDatabase::memory();
    let mut connection = database.connect().await.unwrap();
    let mut records = connection
        .query(
            "SELECT CAST(42 AS BIGINT) AS id, 'long string retained across driver release' AS text"
                .into(),
        )
        .await
        .unwrap();
    let batch = records.next().await.unwrap().unwrap();
    let database_owner = Arc::downgrade(&database.current().await.unwrap());
    let column = batch.column(1).slice(0, 1);
    drop(batch);
    drop(records);
    drop(connection);
    drop(database);
    assert!(database_owner.upgrade().is_some());
    let strings = column.as_any().downcast_ref::<StringViewArray>().unwrap();
    assert_eq!(
        strings.value(0),
        "long string retained across driver release"
    );
    drop(column);
}

#[tokio::test(flavor = "current_thread")]
async fn ingestion_can_poll_an_async_producer_without_blocking_the_executor() {
    let schema = Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
        "id",
        arrow_schema::DataType::Int64,
        false,
    )]));
    let batch =
        RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(vec![1, 2]))]).unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        sender.send(batch).unwrap();
    });
    let input = RecordBatchStream::new(
        schema,
        futures::stream::once(async move { Ok(receiver.await.unwrap()) }).boxed(),
    );
    let reader = super::streaming::IngestionReader::new(input);
    let batches = tokio::task::spawn_blocking(move || reader.collect::<Result<Vec<_>, _>>())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .values(),
        &[1, 2]
    );
}

#[tokio::test]
async fn typed_errors_and_failed_operations_leave_connection_usable() {
    let database = AdbcDatabase::memory();
    let mut connection = database.connect().await.unwrap();
    assert!(
        connection
            .table_schema(RelationName::parse("missing").unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(connection.query("SELECT FROM".into()).await.is_err());
    assert_eq!(
        connection
            .query("SELECT 1".into())
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .iter()
            .map(RecordBatch::num_rows)
            .sum::<usize>(),
        1
    );
    let error: DatabaseError = adbc_core::error::Error::with_message_and_status(
        "not supported",
        adbc_core::error::Status::NotImplemented,
    )
    .into();
    assert_eq!(error.kind(), DatabaseErrorKind::Unsupported);
}

#[derive(Deserialize)]
struct LocalSpiralConfig {
    api_url: String,
    spfs_url: String,
    project_id: String,
    /// A user access token, such as `spiraldb login --show-token` prints.
    access_token: String,
}

impl LocalSpiralConfig {
    fn load() -> crate::SpiralConnection {
        let path = std::env::var("SPIRAL_SCHEMA_TEST_CONFIG")
            .expect("set SPIRAL_SCHEMA_TEST_CONFIG to a local Spiral connection JSON file");
        let config: Self = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for endpoint in [&config.api_url, &config.spfs_url] {
            let url = url::Url::parse(endpoint).unwrap();
            assert!(matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]")
            ));
        }
        let settings = SpiralConnectionSettings::new(config.api_url, config.spfs_url)
            .unwrap()
            .with_project(&config.project_id)
            .unwrap();
        crate::SpiralConnection::new(settings, crate::SpiralAccess::fixed(config.access_token))
    }
}

#[tokio::test]
#[ignore = "requires local Spiral API and SpFS; set SPIRAL_SCHEMA_TEST_CONFIG"]
async fn adbc_persistence_contract() {
    DatabaseContract::new(Arc::new(AdbcDatabase::new(LocalSpiralConfig::load())))
        .ingestion_and_transactions()
        .await
        .unwrap();
}

// Retain the required lifecycle behavior until Spiral coordinates Arrow stream
// release with query completion. This reproduces the native adapter's blocker.
#[tokio::test]
#[ignore = "upstream Spiral: dropped stream cancellation can outlive the rollback attempt"]
async fn dropped_transaction_waits_for_result_before_rollback() {
    let database = tokio::task::spawn_blocking(|| DriverConfiguration::memory().load())
        .await
        .unwrap()
        .unwrap();
    let mut worker = ConnectionWorker::connect(database).await.unwrap();
    let mut cleanup = worker.observe_cleanup();
    let connection = Box::new(AdbcConnection::new(worker));
    let mut transaction = connection.begin().await.unwrap();
    let mut records = transaction
        .query("SELECT * FROM range(0, 1000000)".into())
        .await
        .unwrap();
    // Start execution before abandoning a still-live result.
    records.next().await.unwrap().unwrap();
    drop(transaction);
    assert!(matches!(
        cleanup.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    drop(records);
    tokio::time::timeout(std::time::Duration::from_secs(5), cleanup)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

struct HostedIngestionFixture {
    database: Arc<AdbcDatabase>,
    table: RelationName,
}

impl HostedIngestionFixture {
    fn new() -> Self {
        Self {
            database: Arc::new(AdbcDatabase::new(LocalSpiralConfig::load())),
            table: RelationName::parse(format!(
                "adbc_visibility_{}",
                uuid::Uuid::new_v4().simple()
            ))
            .unwrap(),
        }
    }

    fn input(&self, value: i64) -> BatchIngestion {
        let schema = Arc::new(Schema::new(vec![arrow_schema::Field::new(
            "id",
            arrow_schema::DataType::Int64,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![value]))],
        )
        .unwrap();
        BatchIngestion::new(
            self.table.clone(),
            crate::db::IngestMode::CreateOrAppend,
            RecordBatchStream::new(schema, futures::stream::iter([Ok(batch)]).boxed()),
        )
    }

    async fn verify_isolation(&self) -> DatabaseResult<()> {
        let mut connection = self.database.connect().await?;
        connection.ingest(self.input(1)).await?;
        let mut transaction = connection.begin().await?;
        transaction.ingest(self.input(2)).await?;
        let mut observer = self.database.connect().await?;
        let records = observer
            .query(format!("SELECT * FROM {}", self.table.as_str()).into())
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        assert_eq!(
            records.iter().map(RecordBatch::num_rows).sum::<usize>(),
            1,
            "ADBC ingestion must stay inside its transaction"
        );
        transaction.rollback().await?;
        Ok(())
    }

    async fn verify_noop_delete(&self) -> DatabaseResult<()> {
        let mut connection = self.database.connect().await?;
        connection.ingest(self.input(1)).await?;
        connection
            .execute_update(format!("DELETE FROM {} WHERE id = 999", self.table.as_str()).into())
            .await?;
        Ok(())
    }

    async fn cleanup(&self) {
        let mut connection = self.database.connect().await.unwrap();
        connection
            .execute_update(format!("DROP TABLE IF EXISTS {}", self.table.as_str()).into())
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires local Spiral API and SpFS; set SPIRAL_SCHEMA_TEST_CONFIG"]
async fn adbc_transactional_ingestion_stays_isolated() {
    let fixture = HostedIngestionFixture::new();
    let result = std::panic::AssertUnwindSafe(fixture.verify_isolation())
        .catch_unwind()
        .await;
    fixture.cleanup().await;
    match result {
        Ok(result) => result.unwrap(),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
#[ignore = "requires local Spiral API and SpFS; set SPIRAL_SCHEMA_TEST_CONFIG"]
async fn adbc_hosted_noop_delete_succeeds() {
    let fixture = HostedIngestionFixture::new();
    let result = fixture.verify_noop_delete().await;
    fixture.cleanup().await;
    result.unwrap();
}

#[tokio::test]
async fn retaining_nested_buffers_preserves_slices_nulls_and_allocation_addresses() {
    use arrow_array::{ArrayRef, ListArray, types::Int64Type};
    let database = tokio::task::spawn_blocking(|| DriverConfiguration::memory().load())
        .await
        .unwrap()
        .unwrap();
    let values: ArrayRef = Arc::new(ListArray::from_iter_primitive::<Int64Type, _, _>([
        Some(vec![Some(1), None, Some(3)]),
        None,
        Some(vec![Some(4)]),
    ]));
    let schema = Arc::new(Schema::new(vec![arrow_schema::Field::new(
        "items",
        values.data_type().clone(),
        true,
    )]));
    let original = RecordBatch::try_new(schema, vec![values.slice(1, 2)]).unwrap();
    let retained = super::streaming::DriverBuffers::retain(original.clone(), database);
    assert_eq!(original, retained);
    assert_eq!(
        original.column(0).to_data().buffers()[0].as_ptr(),
        retained.column(0).to_data().buffers()[0].as_ptr()
    );
    assert!(retained.column(0).is_null(0));
    let column = retained
        .column(0)
        .as_any()
        .downcast_ref::<ListArray>()
        .unwrap();
    assert_eq!(
        column
            .value(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap()
            .value(0),
        4
    );
}

#[tokio::test]
async fn query_reader_errors_are_terminal_and_release_the_connection() {
    let database = AdbcDatabase::memory();
    let mut connection = database.connect().await.unwrap();
    let mut records = connection
        .query("SELECT * FROM range(0, 1000001)".into())
        .await
        .unwrap();
    assert!(records.next().await.unwrap().is_err());
    assert!(records.next().await.is_none());
    let batches = connection
        .query("SELECT 1".into())
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
}

#[tokio::test]
async fn changed_provider_keys_reopen_the_database_for_new_connections() {
    let keys_with = |key: Option<&str>| {
        crate::ProviderKeys::default().updated(
            &spiral_robotics_common::providers::ProviderKeysUpdate::default()
                .with_llm_api_key(key.map(str::to_owned)),
        )
    };
    let (keys, follower) = tokio::sync::watch::channel(keys_with(Some("first")));
    let database = AdbcDatabase::memory().with_provider_keys(follower);
    let first = database.current().await.unwrap();
    // The same key keeps the database it opened.
    assert!(Arc::ptr_eq(&first, &database.current().await.unwrap()));
    let mut open = database.connect().await.unwrap();
    keys.send_replace(keys_with(Some("second")));
    let second = database.current().await.unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    // A connection opened before the change still answers.
    let rows = open
        .query(SqlStatement::from("SELECT 1 AS one"))
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(rows.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
    keys.send_replace(keys_with(None));
    assert!(!Arc::ptr_eq(&second, &database.current().await.unwrap()));
}
