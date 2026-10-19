//! Behaviour every Studio database keeps, checked through the async traits
//! alone: on DataFusion's in-memory tables here, and on a hosted Spiral
//! project by the ignored test.

use std::sync::Arc;

use arrow_array::{Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};

use super::database::{
    BatchIngestion, Database, DatabaseResult, DatabaseSession, IngestMode, RecordBatchStream,
    RelationName,
};

struct DatabaseContract {
    database: Arc<dyn Database>,
}

impl DatabaseContract {
    async fn query_streams(&self) -> DatabaseResult<()> {
        let mut connection = self.database.connect().await?;
        let mut empty = connection
            .query("SELECT CAST(1 AS BIGINT) AS id WHERE false".into())
            .await?;
        assert_eq!(empty.schema().field(0).name(), "id");
        assert_eq!(empty.schema().field(0).data_type(), &DataType::Int64);
        while let Some(batch) = empty.next().await {
            assert_eq!(batch?.num_rows(), 0);
        }
        // One connection holds several results at once, each outliving it.
        let first = connection.query("SELECT 1 AS id".into()).await?;
        let second = connection.query("SELECT 2 AS id".into()).await?;
        drop(connection);
        for records in [first, second] {
            assert_eq!(Self::rows(records).await?, 1);
        }
        let mut connection = self.database.connect().await?;
        assert!(connection.query("SELECT FROM".into()).await.is_err());
        assert_eq!(Self::row_count(connection.as_mut(), "SELECT 5").await?, 1);
        Ok(())
    }

    /// Requires a writable project; each run owns unique names.
    async fn ingestion_and_transactions(&self) -> DatabaseResult<()> {
        let table = format!("database_contract_{}", uuid::Uuid::new_v4().simple());
        let pending = format!("{table}.pending");
        let result = std::panic::AssertUnwindSafe(self.verify_persistence(&table, &pending))
            .catch_unwind()
            .await;
        let mut cleanup = self.database.connect().await?.begin().await?;
        for name in [&table, &pending] {
            cleanup
                .drop_table(RelationName::parse(name.as_str())?)
                .await?;
        }
        cleanup.commit().await?;
        match result {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    async fn verify_persistence(&self, table: &str, pending: &str) -> DatabaseResult<()> {
        let select = |name: &str| format!("SELECT * FROM \"{name}\"");
        let mut connection = self.database.connect().await?;
        connection
            .ingest(Self::input(table, IngestMode::CreateOrAppend, &[1, 2])?)
            .await?;
        connection
            .ingest(Self::input(table, IngestMode::CreateOrAppend, &[3])?)
            .await?;
        assert_eq!(
            Self::row_count(connection.as_mut(), &select(table)).await?,
            3
        );
        connection
            .ingest(Self::input(table, IngestMode::ReplaceOrCreate, &[6, 7])?)
            .await?;
        assert_eq!(
            Self::row_count(connection.as_mut(), &select(table)).await?,
            2
        );
        let schema = connection
            .table_schema(RelationName::parse(table)?)
            .await?
            .unwrap();
        assert_eq!(schema.field(0).data_type(), &DataType::Int64);

        // A transaction holds its writes, two to one table folded into one,
        // and shows them only to itself until it commits.
        let mut observer = self.database.connect().await?;
        let mut transaction = connection.begin().await?;
        for values in [[8], [9]] {
            transaction
                .ingest(Self::input(table, IngestMode::CreateOrAppend, &values)?)
                .await?;
        }
        transaction
            .ingest(Self::input(pending, IngestMode::ReplaceOrCreate, &[9])?)
            .await?;
        let pending_name = RelationName::parse(pending)?;
        assert!(
            transaction
                .table_schema(pending_name.clone())
                .await?
                .is_some()
        );
        assert!(observer.table_schema(pending_name.clone()).await?.is_none());
        assert_eq!(Self::row_count(observer.as_mut(), &select(table)).await?, 2);
        transaction.rollback().await?;
        assert!(observer.table_schema(pending_name.clone()).await?.is_none());
        assert_eq!(Self::row_count(observer.as_mut(), &select(table)).await?, 2);

        let mut transaction = self.database.connect().await?.begin().await?;
        for values in [[8], [9]] {
            transaction
                .ingest(Self::input(table, IngestMode::CreateOrAppend, &values)?)
                .await?;
        }
        transaction
            .ingest(Self::input(pending, IngestMode::CreateOrAppend, &[10])?)
            .await?;
        transaction.commit().await?;
        assert_eq!(Self::row_count(observer.as_mut(), &select(table)).await?, 4);
        assert_eq!(
            Self::row_count(observer.as_mut(), &select(pending)).await?,
            1
        );

        // A drop is held like any write.
        let mut transaction = self.database.connect().await?.begin().await?;
        transaction.drop_table(pending_name.clone()).await?;
        assert!(
            transaction
                .table_schema(pending_name.clone())
                .await?
                .is_none()
        );
        assert!(observer.table_schema(pending_name.clone()).await?.is_some());
        transaction.commit().await?;
        assert!(observer.table_schema(pending_name).await?.is_none());
        Ok(())
    }

    fn input(table: &str, mode: IngestMode, values: &[i64]) -> DatabaseResult<BatchIngestion> {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(values.to_vec()))],
        )
        .unwrap();
        Ok(BatchIngestion::new(
            RelationName::parse(table)?,
            mode,
            RecordBatchStream::new(schema, futures::stream::iter([Ok(batch)]).boxed()),
        ))
    }

    async fn rows(records: RecordBatchStream) -> DatabaseResult<usize> {
        let batches = records.try_collect::<Vec<_>>().await?;
        Ok(batches.iter().map(RecordBatch::num_rows).sum())
    }

    async fn row_count(session: &mut dyn DatabaseSession, sql: &str) -> DatabaseResult<usize> {
        Self::rows(session.query(sql.into()).await?).await
    }
}

#[tokio::test]
async fn the_in_memory_database_keeps_the_contract() {
    let contract = DatabaseContract {
        database: Arc::new(super::MemoryDatabase::default()),
    };
    contract.query_streams().await.unwrap();
    contract.ingestion_and_transactions().await.unwrap();
}

#[tokio::test]
#[ignore = "requires SPIRAL_MCAP_TEST_PROJECT, SPIRAL_ACCESS_TOKEN and SPIRAL_DATA_PLANE_LIBRARY; creates and drops uniquely named tables"]
async fn a_hosted_spiral_project_keeps_the_contract() {
    let (store, _) = crate::datasets::HostedProject::storage();
    let contract = DatabaseContract {
        database: store.database(),
    };
    contract.query_streams().await.unwrap();
    contract.ingestion_and_transactions().await.unwrap();
}
