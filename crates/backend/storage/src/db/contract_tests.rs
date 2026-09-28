//! Driver-independent behavioral checks. Each adapter supplies a database;
//! the suite never inspects engine handles or reaches around the async traits.

use std::sync::Arc;

use arrow_array::{Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};

use super::database::{
    BatchIngestion, Database, DatabaseErrorKind, DatabaseResult, DatabaseSession, IngestMode,
    RecordBatchStream, RelationName,
};

pub(super) struct DatabaseContract {
    database: Arc<dyn Database>,
}

impl DatabaseContract {
    pub(super) fn new(database: Arc<dyn Database>) -> Self {
        Self { database }
    }

    pub(super) async fn query_streams(&self) -> DatabaseResult<()> {
        let mut connection = self.database.connect().await?;
        let mut empty = connection
            .query("SELECT CAST(1 AS BIGINT) AS id WHERE false".into())
            .await?;
        assert_eq!(empty.schema().field(0).name(), "id");
        assert_eq!(empty.schema().field(0).data_type(), &DataType::Int64);
        while let Some(batch) = empty.next().await {
            assert_eq!(batch?.num_rows(), 0);
        }
        // EOF releases the connection, even if the exhausted stream stays alive.
        let records = connection.query("SELECT 1 AS id".into()).await?;
        assert_eq!(
            connection
                .query("SELECT 2".into())
                .await
                .err()
                .unwrap()
                .kind(),
            DatabaseErrorKind::InvalidState
        );
        assert_eq!(
            connection
                .execute_update("CREATE VIEW must_not_run AS SELECT 1".into())
                .await
                .err()
                .unwrap()
                .kind(),
            DatabaseErrorKind::InvalidState
        );
        assert_eq!(
            connection
                .table_schema(RelationName::parse("missing")?)
                .await
                .err()
                .unwrap()
                .kind(),
            DatabaseErrorKind::InvalidState
        );
        assert_eq!(
            connection
                .ingest(Self::input(
                    "must_not_run",
                    IngestMode::CreateOrAppend,
                    &[1]
                )?)
                .await
                .err()
                .unwrap()
                .kind(),
            DatabaseErrorKind::InvalidState
        );

        let mut independent = self.database.connect().await?;
        assert_eq!(Self::row_count(independent.as_mut(), "SELECT 2").await?, 1);
        drop(records);
        assert_eq!(Self::row_count(connection.as_mut(), "SELECT 3").await?, 1);
        let records = connection.query("SELECT 4 AS id".into()).await?;
        drop(connection);
        assert_eq!(
            records
                .try_collect::<Vec<_>>()
                .await?
                .iter()
                .map(RecordBatch::num_rows)
                .sum::<usize>(),
            1
        );

        let mut connection = self.database.connect().await?;
        assert!(connection.query("SELECT FROM".into()).await.is_err());
        assert_eq!(Self::row_count(connection.as_mut(), "SELECT 5").await?, 1);

        let mut transaction = connection.begin().await?;
        let records = transaction.query("SELECT 6".into()).await?;
        assert_eq!(
            transaction.commit().await.err().unwrap().kind(),
            DatabaseErrorKind::InvalidState
        );
        // Rejecting completion must not destroy an already returned result.
        assert_eq!(
            records
                .try_collect::<Vec<_>>()
                .await?
                .iter()
                .map(RecordBatch::num_rows)
                .sum::<usize>(),
            1
        );
        Ok(())
    }

    /// Requires a writable, transactional catalog; each run owns unique names.
    pub(super) async fn ingestion_and_transactions(&self) -> DatabaseResult<()> {
        let table = format!("database_contract_{}", uuid::Uuid::new_v4().simple());
        let pending = format!("{table}_pending");
        let result = std::panic::AssertUnwindSafe(self.verify_persistence(&table, &pending))
            .catch_unwind()
            .await;
        let mut cleanup = self.database.connect().await?;
        for name in [&table, &pending] {
            cleanup
                .execute_update(format!("DROP TABLE IF EXISTS {name}").into())
                .await?;
        }
        match result {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    async fn verify_persistence(&self, table: &str, pending: &str) -> DatabaseResult<()> {
        let mut connection = self.database.connect().await?;
        connection
            .ingest(Self::input(table, IngestMode::CreateOrAppend, &[1, 2])?)
            .await?;
        connection
            .ingest(Self::input(table, IngestMode::CreateOrAppend, &[3])?)
            .await?;
        assert_eq!(
            Self::row_count(connection.as_mut(), &format!("SELECT * FROM {table}")).await?,
            3
        );
        connection
            .ingest(Self::input(table, IngestMode::ReplaceOrCreate, &[6, 7])?)
            .await?;
        assert_eq!(
            Self::row_count(connection.as_mut(), &format!("SELECT * FROM {table}")).await?,
            2
        );
        let schema = connection
            .table_schema(RelationName::parse(table)?)
            .await?
            .unwrap();
        assert_eq!(schema.field(0).data_type(), &DataType::Int64);

        let mut observer = self.database.connect().await?;
        let mut transaction = connection.begin().await?;
        transaction
            .ingest(Self::input(table, IngestMode::CreateOrAppend, &[8])?)
            .await?;
        transaction
            .ingest(Self::input(pending, IngestMode::ReplaceOrCreate, &[9])?)
            .await?;
        assert!(
            transaction
                .table_schema(RelationName::parse(pending)?)
                .await?
                .is_some()
        );
        assert!(
            observer
                .table_schema(RelationName::parse(pending)?)
                .await?
                .is_none()
        );
        assert_eq!(
            Self::row_count(transaction.as_mut(), &format!("SELECT * FROM {table}")).await?,
            3
        );
        assert_eq!(
            Self::row_count(observer.as_mut(), &format!("SELECT * FROM {table}")).await?,
            2
        );
        transaction.rollback().await?;
        assert!(
            observer
                .table_schema(RelationName::parse(pending)?)
                .await?
                .is_none()
        );
        assert_eq!(
            Self::row_count(observer.as_mut(), &format!("SELECT * FROM {table}")).await?,
            2
        );

        let mut transaction = self.database.connect().await?.begin().await?;
        transaction
            .ingest(Self::input(pending, IngestMode::CreateOrAppend, &[10])?)
            .await?;
        transaction.commit().await?;
        assert!(
            observer
                .table_schema(RelationName::parse(pending)?)
                .await?
                .is_some()
        );
        assert_eq!(
            Self::row_count(observer.as_mut(), &format!("SELECT * FROM {pending}")).await?,
            1
        );
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

    async fn row_count(session: &mut dyn DatabaseSession, sql: &str) -> DatabaseResult<usize> {
        let batches = session
            .query(sql.into())
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        Ok(batches.iter().map(RecordBatch::num_rows).sum())
    }
}
