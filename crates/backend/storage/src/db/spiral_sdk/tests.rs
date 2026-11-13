//! How Spiral reads Studio's tables, checked on a hosted project.
use std::sync::Arc;

use arrow_array::{RecordBatch, StringArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use datafusion::prelude::SessionConfig;

use super::{SpiralProject, SpiralSdkDatabase};
use crate::db::database::{
    BatchIngestion, Database as _, DatabaseErrorKind, IngestMode, RecordBatchStream, RelationName,
};

/// Episodes are numbered in write order, so a table holds each episode's
/// rows together and a read of one episode skips every window of the others
/// by their statistics.
#[tokio::test]
#[ignore = "requires hosted credentials; writes and drops a few-MB probe table"]
async fn an_episode_read_skips_every_window_of_other_episodes() {
    let connection = crate::datasets::HostedProject::connection();
    let project = SpiralProject::open(&connection).unwrap();
    let name = format!("probe_pruning_{}", uuid::Uuid::new_v4().simple());
    let rows = 100_000u64;
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::UInt64, false),
        Field::new("ordinal", DataType::UInt64, false),
        Field::new("payload", DataType::Utf8, false),
    ]));
    let batches = (0..rows)
        .step_by(10_000)
        .map(|start| {
            let range = start..start + 10_000;
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(UInt64Array::from_iter_values(
                        range.clone().map(|row| row / 1_000),
                    )),
                    Arc::new(UInt64Array::from_iter_values(
                        range.clone().map(|row| row % 1_000),
                    )),
                    Arc::new(StringArray::from_iter_values(
                        range.map(|row| format!("{row:064}")),
                    )),
                ],
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let snapshot = project.snapshot().await.unwrap();
    let mut transaction = snapshot.commit();
    let table = snapshot.project().write_table(batches).await.unwrap();
    transaction.create_table(&name, &table).unwrap();
    transaction.publish().await.unwrap();
    let explained = async {
        let snapshot = project.snapshot().await.map_err(|e| e.to_string())?;
        let context = project.catalog(&snapshot).session(SessionConfig::new());
        let sql = format!("EXPLAIN ANALYZE SELECT count(*) AS n, max(length(payload)) AS l FROM \"{name}\" WHERE id = 50");
        let batches = context.sql(&sql).await.map_err(|e| e.to_string())?.collect().await.map_err(|e| e.to_string())?;
        datafusion::arrow::util::pretty::pretty_format_batches(&batches).map(|text| text.to_string()).map_err(|e| e.to_string())
    }
    .await;
    let snapshot = project.snapshot().await.unwrap();
    let mut cleanup = snapshot.commit();
    cleanup
        .drop_table(&snapshot.table(&name).await.unwrap())
        .unwrap();
    cleanup.publish().await.unwrap();
    let explained = explained.unwrap();
    assert!(
        explained.contains("Rows returned by scan: 1,000"),
        "the scan returns episode 50's rows alone: {explained}"
    );
    assert!(
        explained.contains("Windows: 1 visited, 12 skipped by statistics"),
        "{explained}"
    );
}

/// Two transactions cannot both create the same table name from one snapshot.
#[tokio::test]
#[ignore = "requires hosted credentials; writes and drops a probe table"]
async fn concurrent_transactions_cannot_both_create_the_same_table() {
    let database = SpiralSdkDatabase::new(crate::datasets::HostedProject::connection());
    let name = format!("probe_conflict_{}", uuid::Uuid::new_v4().simple());
    let episode = |id: u64| {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::UInt64, false)]));
        let batch =
            RecordBatch::try_new(schema.clone(), vec![Arc::new(UInt64Array::from(vec![id]))]);
        BatchIngestion::new(
            RelationName::parse(name.clone()).unwrap(),
            IngestMode::CreateOrAppend,
            RecordBatchStream::new(
                schema,
                Box::pin(futures::stream::iter([batch.map_err(|error| {
                    crate::db::database::DatabaseError::invalid_input(error.to_string())
                })])),
            ),
        )
    };
    let mut first = database.connect().await.unwrap().begin().await.unwrap();
    let mut second = database.connect().await.unwrap().begin().await.unwrap();
    first.ingest(episode(0)).await.unwrap();
    second.ingest(episode(0)).await.unwrap();
    first.commit().await.unwrap();
    let raced = second.commit().await;
    let mut cleanup = database.connect().await.unwrap();
    cleanup
        .drop_table(RelationName::parse(name.clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(raced.unwrap_err().kind(), DatabaseErrorKind::Conflict);
}
