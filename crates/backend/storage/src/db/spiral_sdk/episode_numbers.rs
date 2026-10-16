//! One-off (October 2026): number every dataset's episodes in place of their
//! UUIDs. Each table that names episodes (`episodes` and every stream's
//! messages by `id`, every answer column by `episode_id`) is written again
//! under its own name with the episode's number, every other column and the
//! row order as they were; `topics` and `files` name no episode and stay. An
//! episode's number is its place in the `episodes` table, which Studio wrote
//! in the order it wrote every other table, so each table holds an episode's
//! rows together. A stream's messages are read with parallel lanes and sorted
//! back into that order by episode and ordinal: Studio's episode UUIDs are
//! version 7, so they sort in the order they were written, and a read in
//! stored order would go through one lane, each request waiting on the last.
//! A dataset moves in one commit; one whose episodes are
//! numbered already is left alone. Run it with Studio stopped: a table
//! created after the run lists a dataset's tables would keep its UUIDs.
//! Delete this module once the hosted project has moved.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow_array::{
    Array as _, ArrayRef, FixedSizeBinaryArray, RecordBatch, RecordBatchReader, UInt64Array,
};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use datafusion::catalog::CatalogProvider as _;
use datafusion::execution::SessionStateBuilder;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use futures::StreamExt as _;
use spiral::CommitOutcome;
use spiral_datafusion::SpiralCatalogProvider;
use spiral_robotics_common::datasets::{DatasetSelection, DatasetTable};

use super::{SpiralProject, SpiralSdkDatabase};
use crate::SpiralConnection;

impl SpiralSdkDatabase {
    /// Number the episodes of every dataset of `connection`'s project still
    /// naming them by UUID, or of `only`; one line of report per dataset.
    pub async fn number_episodes(
        connection: &SpiralConnection,
        only: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let project = SpiralProject::open(connection).map_err(|error| error.to_string())?;
        let catalog = crate::datasets::DatasetCatalogClient::new(connection)?
            .datasets()
            .await?;
        let mut report = Vec::new();
        for summary in catalog.summaries() {
            let dataset = summary.selection();
            if only.is_some_and(|only| only != dataset.id()) {
                continue;
            }
            let numbering = EpisodeNumbering {
                project: &project,
                dataset,
                tables: catalog.tables(dataset)?,
            };
            let line = numbering.run().await?;
            println!("{line}");
            report.push(line);
        }
        Ok(report)
    }
}

/// One dataset's tables as they are numbered.
struct EpisodeNumbering<'a> {
    project: &'a SpiralProject,
    dataset: &'a DatasetSelection,
    tables: Vec<DatasetTable>,
}

impl EpisodeNumbering<'_> {
    /// The lanes a stream's messages are read with.
    const LANES: usize = 32;
    /// What sorting a stream's messages may hold in memory before spilling.
    const SORT_MEMORY: usize = 4 << 30;

    async fn run(&self) -> Result<String, String> {
        let label = self.dataset.label();
        let snapshot = self
            .project
            .snapshot()
            .await
            .map_err(|error| error.to_string())?;
        let catalog = SpiralCatalogProvider::new(snapshot.table_session());
        // One partition reads a table in the order it was written: the
        // episodes, whose order numbers them, and the small answer columns.
        let mut config = SessionConfig::new().with_target_partitions(1);
        config.options_mut().sql_parser.enable_ident_normalization = false;
        let session = catalog.session(config);
        let parallel = Self::parallel_session(&catalog)?;
        let Some(numbers) = self.numbers(&session).await? else {
            return Ok(format!("{label}: numbered already"));
        };
        let mut transaction = snapshot.transaction().map_err(|error| error.to_string())?;
        let disorder = Arc::new(AtomicU64::new(0));
        let mut rewritten = 0;
        for table in &self.tables {
            let source = snapshot
                .table(table.name())
                .await
                .map_err(|error| format!("{}: {error}", table.name()))?;
            let all = session
                .sql(&format!("SELECT * FROM {table}"))
                .await
                .map_err(|error| error.to_string())?;
            let stream = if all.schema().has_column_with_unqualified_name("ordinal") {
                parallel
                    .sql(&format!("SELECT * FROM {table} ORDER BY id, ordinal"))
                    .await
                    .map_err(|error| error.to_string())?
            } else {
                all
            }
            .execute_stream()
            .await
            .map_err(|error| error.to_string())?;
            let Some(rows) = NumberedRows::of(stream, numbers.clone(), disorder.clone())? else {
                continue;
            };
            transaction
                .drop_table(&source)
                .map_err(|error| error.to_string())?;
            transaction
                .create_table(
                    table.name(),
                    Box::new(rows) as Box<dyn RecordBatchReader + Send>,
                )
                .await
                .map_err(|error| format!("{}: {error}", table.name()))?;
            rewritten += 1;
        }
        match transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?
        {
            CommitOutcome::Committed(_) => Ok(format!(
                "{label}: {} episodes numbered across {rewritten} tables; {} rows out of number order",
                numbers.len(),
                disorder.load(Ordering::Relaxed)
            )),
            CommitOutcome::Conflict(_) => {
                Err(format!("{label}: the project changed while numbering"))
            }
        }
    }

    /// A session over `catalog` that reads a table with many lanes, in no
    /// order, and sorts with at most [`Self::SORT_MEMORY`], spilling the rest.
    fn parallel_session(catalog: &SpiralCatalogProvider) -> Result<SessionContext, String> {
        const CATALOG: &str = "project";
        let schema = catalog
            .schema_names()
            .into_iter()
            .next()
            .ok_or("the Spiral catalog has no schema")?;
        let mut config = SessionConfig::new()
            .with_target_partitions(Self::LANES)
            .with_default_catalog_and_schema(CATALOG, schema);
        config.options_mut().sql_parser.enable_ident_normalization = false;
        let runtime = RuntimeEnvBuilder::new()
            .with_memory_limit(Self::SORT_MEMORY, 1.0)
            .build_arc()
            .map_err(|error| error.to_string())?;
        let state = spiral_datafusion::with_spiral(
            SessionStateBuilder::new()
                .with_default_features()
                .with_config(config)
                .with_runtime_env(runtime),
        )
        .build();
        let session = SessionContext::new_with_state(state);
        session.register_catalog(CATALOG, Arc::new(catalog.clone()));
        Ok(session)
    }

    /// Each episode's number, by its UUID: its place in the `episodes`
    /// table. `None` when the dataset's episodes are numbered already.
    async fn numbers(
        &self,
        session: &SessionContext,
    ) -> Result<Option<Arc<HashMap<[u8; 16], u64>>>, String> {
        let ids = session
            .sql(&format!("SELECT id FROM {}", self.dataset.episodes()))
            .await
            .map_err(|error| error.to_string())?;
        match ids.schema().field(0).data_type() {
            DataType::UInt64 => return Ok(None),
            DataType::FixedSizeBinary(16) => {}
            other => {
                return Err(format!(
                    "{}: episodes are named by {other}, neither UUIDs nor numbers",
                    self.dataset.label()
                ));
            }
        }
        let batches = ids.collect().await.map_err(|error| error.to_string())?;
        let mut numbers = HashMap::new();
        for batch in &batches {
            let ids = batch
                .column(0)
                .as_any()
                .downcast_ref::<FixedSizeBinaryArray>()
                .ok_or("an episode column of UUIDs")?;
            for row in 0..ids.len() {
                let uuid =
                    <[u8; 16]>::try_from(ids.value(row)).map_err(|error| error.to_string())?;
                let number = numbers.len() as u64;
                if numbers.insert(uuid, number).is_some() {
                    return Err(format!("{} lists an episode twice", self.dataset.label()));
                }
            }
        }
        Ok(Some(Arc::new(numbers)))
    }
}

/// A table's rows with its episode column numbered, streamed to Spiral as
/// they are read.
struct NumberedRows {
    schema: SchemaRef,
    batches: tokio::sync::mpsc::Receiver<Result<RecordBatch, ArrowError>>,
}

impl NumberedRows {
    /// `stream`'s rows numbered by `numbers`; `None` when it names no
    /// episode. Rows whose number is below the one before count in
    /// `disorder`.
    fn of(
        stream: datafusion::physical_plan::SendableRecordBatchStream,
        numbers: Arc<HashMap<[u8; 16], u64>>,
        disorder: Arc<AtomicU64>,
    ) -> Result<Option<Self>, String> {
        let source = stream.schema();
        let Some(place) = ["id", "episode_id"]
            .iter()
            .find_map(|name| source.index_of(name).ok())
        else {
            return Ok(None);
        };
        let fields = source
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| match index == place {
                true => Arc::new(Field::new(field.name(), DataType::UInt64, false)),
                false => field.clone(),
            })
            .collect::<Vec<_>>();
        let schema = Arc::new(Schema::new_with_metadata(fields, source.metadata().clone()));
        let (sender, batches) = tokio::sync::mpsc::channel(4);
        let numbered = schema.clone();
        tokio::spawn(async move {
            let mut stream = stream;
            let mut last = 0u64;
            while let Some(batch) = stream.next().await {
                let batch = batch
                    .map_err(|error| ArrowError::ExternalError(Box::new(error)))
                    .and_then(|batch| {
                        Self::number(&batch, place, &numbers, &numbered, &mut last, &disorder)
                    });
                if sender.send(batch).await.is_err() {
                    return;
                }
            }
        });
        Ok(Some(Self { schema, batches }))
    }

    fn number(
        batch: &RecordBatch,
        place: usize,
        numbers: &HashMap<[u8; 16], u64>,
        schema: &SchemaRef,
        last: &mut u64,
        disorder: &AtomicU64,
    ) -> Result<RecordBatch, ArrowError> {
        let ids = batch
            .column(place)
            .as_any()
            .downcast_ref::<FixedSizeBinaryArray>()
            .ok_or_else(|| ArrowError::SchemaError("an episode column of UUIDs".into()))?;
        let mut numbered = Vec::with_capacity(ids.len());
        for row in 0..ids.len() {
            let uuid = <[u8; 16]>::try_from(ids.value(row))
                .map_err(|error| ArrowError::InvalidArgumentError(error.to_string()))?;
            let number = *numbers.get(&uuid).ok_or_else(|| {
                ArrowError::InvalidArgumentError(
                    "a row of an episode the dataset does not list".into(),
                )
            })?;
            if number < *last {
                disorder.fetch_add(1, Ordering::Relaxed);
            }
            *last = number;
            numbered.push(number);
        }
        let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
        columns[place] = Arc::new(UInt64Array::from(numbered));
        RecordBatch::try_new(schema.clone(), columns)
    }
}

impl Iterator for NumberedRows {
    type Item = Result<RecordBatch, ArrowError>;

    /// Spiral reads its input on a blocking thread, which may wait here.
    fn next(&mut self) -> Option<Self::Item> {
        self.batches.blocking_recv()
    }
}

impl RecordBatchReader for NumberedRows {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{StringArray, UInt64Array};
    use spiral_robotics_common::curation::AnswerColumn;
    use spiral_robotics_common::datasets::TopicRevision;

    /// A dataset as Studio wrote it with UUIDs: three episodes, a stream with
    /// two rows each, and an answer column holding the middle one.
    fn uuid_tables(dataset: &DatasetSelection) -> Vec<(DatasetTable, RecordBatch)> {
        let uuid = |byte: u8| [byte; 16];
        let ids = |rows: &[u8]| -> ArrayRef {
            Arc::new(
                FixedSizeBinaryArray::try_from_iter(rows.iter().map(|byte| uuid(*byte))).unwrap(),
            )
        };
        let id = |name: &str| {
            Field::new(name, DataType::FixedSizeBinary(16), false)
                .with_extension_type(arrow_schema::extension::Uuid)
        };
        let batch = |fields: Vec<Field>, columns: Vec<ArrayRef>| {
            RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
        };
        let stream = TopicRevision::new("joints_0a1b2c3d").unwrap();
        let column = AnswerColumn::try_from("check_rates_3f9a2c".to_owned()).unwrap();
        vec![
            (
                dataset.episodes(),
                batch(
                    vec![id("id"), Field::new("key", DataType::Utf8, false)],
                    vec![
                        ids(&[7, 3, 9]),
                        Arc::new(StringArray::from(vec!["a", "b", "c"])),
                    ],
                ),
            ),
            (
                dataset.messages(&stream),
                batch(
                    vec![id("id"), Field::new("ordinal", DataType::UInt64, false)],
                    vec![
                        ids(&[7, 7, 3, 3, 9, 9]),
                        Arc::new(UInt64Array::from(vec![0, 1, 0, 1, 0, 1])),
                    ],
                ),
            ),
            (
                dataset.answers(&column),
                batch(
                    vec![
                        id("episode_id"),
                        Field::new("value", DataType::UInt64, true),
                    ],
                    vec![ids(&[3]), Arc::new(UInt64Array::from(vec![42]))],
                ),
            ),
        ]
    }

    #[tokio::test]
    #[ignore = "requires SPIRAL_MCAP_TEST_PROJECT, SPIRAL_ACCESS_TOKEN and SPIRAL_DATA_PLANE_LIBRARY; numbers and drops a probe dataset"]
    async fn a_dataset_is_numbered_in_place_in_write_order() {
        let connection = crate::datasets::HostedProject::connection();
        let project = SpiralProject::open(&connection).unwrap();
        let name = format!("probe_numbering_{}", uuid::Uuid::new_v4().simple());
        let dataset = DatasetSelection::new(name.clone()).unwrap();
        let tables = uuid_tables(&dataset);
        let snapshot = project.snapshot().await.unwrap();
        let mut transaction = snapshot.transaction().unwrap();
        for (table, rows) in &tables {
            transaction
                .create_table(table.name(), rows.clone())
                .await
                .unwrap();
        }
        assert!(matches!(
            transaction.commit().await.unwrap(),
            CommitOutcome::Committed(_)
        ));

        let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
            let report = SpiralSdkDatabase::number_episodes(&connection, Some(&name)).await?;
            assert_eq!(report.len(), 1);
            assert!(
                report[0].contains("3 episodes numbered across 3 tables; 0 rows"),
                "{}",
                report[0]
            );
            let read = |sql: String| {
                let project = project.clone();
                async move {
                    let snapshot = project
                        .snapshot()
                        .await
                        .map_err(|error| error.to_string())?;
                    let context = project.catalog(&snapshot).session(SessionConfig::new());
                    context
                        .sql(&sql)
                        .await
                        .map_err(|error| error.to_string())?
                        .collect()
                        .await
                        .map_err(|error| error.to_string())
                }
            };
            let numbers = |batches: &[RecordBatch]| -> Vec<u64> {
                batches
                    .iter()
                    .flat_map(|batch| {
                        let column = batch
                            .column(0)
                            .as_any()
                            .downcast_ref::<UInt64Array>()
                            .unwrap();
                        column.values().to_vec()
                    })
                    .collect()
            };
            let (episodes, messages, answers) = (&tables[0].0, &tables[1].0, &tables[2].0);
            assert_eq!(
                numbers(&read(format!("SELECT id FROM {episodes} ORDER BY key")).await?),
                [0, 1, 2]
            );
            assert_eq!(
                numbers(&read(format!("SELECT id FROM {messages} ORDER BY id, ordinal")).await?),
                [0, 0, 1, 1, 2, 2]
            );
            assert_eq!(
                numbers(&read(format!("SELECT episode_id FROM {answers}")).await?),
                [1]
            );
            let again = SpiralSdkDatabase::number_episodes(&connection, Some(&name)).await?;
            assert!(again[0].ends_with("numbered already"), "{}", again[0]);
            Ok::<_, String>(())
        }))
        .await;
        let snapshot = project.snapshot().await.unwrap();
        let mut cleanup = snapshot.transaction().unwrap();
        for (table, _) in &tables {
            cleanup
                .drop_table(&snapshot.table(table.name()).await.unwrap())
                .unwrap();
        }
        cleanup.commit().await.unwrap();
        match outcome {
            Ok(outcome) => outcome.unwrap(),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}
