use super::*;
use arrow_array::{ArrayRef, BinaryArray, NullArray, StringArray, UInt64Array};
use arrow_ipc::writer::IpcWriteOptions;
use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;
fn arrow_payload(batch: &RecordBatch) -> Vec<u8> {
    arrow_payload_batches(std::slice::from_ref(batch))
}
fn record_batch(fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
        .expect("the E2E fixture batch must match its schema")
}
fn compressed_arrow_payload(batch: &RecordBatch) -> Vec<u8> {
    let mut payload = Vec::new();
    let options = IpcWriteOptions::default()
        .try_with_compression(Some(arrow_ipc::CompressionType::ZSTD))
        .unwrap();
    {
        let mut writer =
            StreamWriter::try_new_with_options(&mut payload, batch.schema().as_ref(), options)
                .unwrap();
        writer.write(batch).unwrap();
        writer.finish().unwrap();
    }
    payload
}
fn arrow_payload_batches(batches: &[RecordBatch]) -> Vec<u8> {
    let mut payload = Vec::new();
    {
        let mut writer =
            StreamWriter::try_new(&mut payload, batches.first().unwrap().schema().as_ref())
                .unwrap();
        for batch in batches {
            writer.write(batch).unwrap();
        }
        writer.finish().unwrap();
    }
    payload
}
#[test]
fn arrow_decode_rejects_compressed_buffers_by_declared_size_before_decompression() {
    let repeated = "x".repeat(1_024);
    let batch = record_batch(
        vec![Field::new("value", DataType::Utf8, false)],
        vec![Arc::new(StringArray::from_iter_values(
            std::iter::repeat_n(repeated.as_str(), 2_048),
        ))],
    );
    let payload = compressed_arrow_payload(&batch);
    assert!(payload.len() < 64 * 1_024);

    let error = ArrowDecodeBudget::default()
        .decode_with_limits(
            ArrowPayload::new("compressed_bomb", payload),
            ArrowDecodeLimits {
                decoded_bytes: 64 * 1_024,
                ..ARROW_DECODE_LIMITS
            },
        )
        .unwrap_err();

    assert!(error.contains("decoded-buffer limit"), "{error}");
}
#[test]
fn arrow_decode_budget_is_shared_across_relations() {
    let repeated = "x".repeat(1_024);
    let batch = record_batch(
        vec![Field::new("value", DataType::Utf8, false)],
        vec![Arc::new(StringArray::from_iter_values(
            std::iter::repeat_n(repeated.as_str(), 32),
        ))],
    );
    let payload = compressed_arrow_payload(&batch);
    let limits = ArrowDecodeLimits {
        decoded_bytes: 48 * 1_024,
        ..ARROW_DECODE_LIMITS
    };
    let mut budget = ArrowDecodeBudget::default();
    budget
        .decode_with_limits(ArrowPayload::new("first_relation", payload.clone()), limits)
        .unwrap();

    let error = budget
        .decode_with_limits(ArrowPayload::new("second_relation", payload), limits)
        .unwrap_err();

    assert!(error.contains("decoded-buffer limit"), "{error}");
}
#[test]
fn arrow_decode_accepts_uncompressed_shared_batch_buffers_within_the_declared_limit() {
    let mut fields = vec![Field::new("payload", DataType::Binary, false)];
    let payload_value = vec![7_u8; 256 * 1_024];
    let mut columns =
        vec![Arc::new(BinaryArray::from(vec![Some(payload_value.as_slice())])) as ArrayRef];
    for index in 0..16 {
        fields.push(Field::new(
            format!("value_{index}"),
            DataType::UInt64,
            false,
        ));
        columns.push(Arc::new(UInt64Array::from(vec![index])) as ArrayRef);
    }
    let payload = arrow_payload(&record_batch(fields, columns));

    ArrowDecodeBudget::default()
        .decode_with_limits(
            ArrowPayload::new("dataset_source", payload.clone()),
            ArrowDecodeLimits {
                decoded_bytes: payload.len(),
                ..ARROW_DECODE_LIMITS
            },
        )
        .expect("declared Arrow buffers fit inside the decode limit");
}
#[test]
fn arrow_decode_rejects_oversized_batch_row_metadata_before_array_decode() {
    let batch = record_batch(
        vec![Field::new("value", DataType::Null, true)],
        vec![Arc::new(NullArray::new(1_001))],
    );
    let error = ArrowDecodeBudget::default()
        .decode_with_limits(
            ArrowPayload::new("too_many_rows", arrow_payload(&batch)),
            ArrowDecodeLimits {
                record_batch_rows: 1_000,
                ..ARROW_DECODE_LIMITS
            },
        )
        .unwrap_err();

    assert!(error.contains("1000-row batch limit"), "{error}");
}
#[test]
fn arrow_decode_rejects_cumulative_relation_rows_before_array_decode() {
    let batch = record_batch(
        vec![Field::new("value", DataType::Null, true)],
        vec![Arc::new(NullArray::new(600))],
    );
    let error = ArrowDecodeBudget::default()
        .decode_with_limits(
            ArrowPayload::new(
                "too_many_relation_rows",
                arrow_payload_batches(&[batch.clone(), batch]),
            ),
            ArrowDecodeLimits {
                relation_rows: 1_000,
                record_batch_rows: 1_000,
                ..ARROW_DECODE_LIMITS
            },
        )
        .unwrap_err();

    assert!(error.contains("1000-row relation limit"), "{error}");
}
#[test]
fn arrow_decode_rejects_oversized_schema_before_stream_reader_conversion() {
    let batch = record_batch(
        vec![
            Field::new("first", DataType::UInt64, false),
            Field::new("second", DataType::UInt64, false),
        ],
        vec![
            Arc::new(UInt64Array::from(Vec::<u64>::new())),
            Arc::new(UInt64Array::from(Vec::<u64>::new())),
        ],
    );
    let error = ArrowDecodeBudget::default()
        .decode_with_limits(
            ArrowPayload::new("too_many_columns", arrow_payload(&batch)),
            ArrowDecodeLimits {
                schema_fields: 1,
                ..ARROW_DECODE_LIMITS
            },
        )
        .unwrap_err();

    assert!(error.contains("1-column limit"), "{error}");
}

struct Fixture;
impl Fixture {
    fn checkpoint() -> RecordBatch {
        use arrow_array::{Float64Array, Int64Array, LargeBinaryArray};
        RecordBatch::try_new(
            Arc::new(Table::TrainingCheckpoints.schema()),
            vec![
                Arc::new(StringArray::from(vec!["run-1"])),
                Arc::new(StringArray::from(vec!["Pick and place"])),
                Arc::new(StringArray::from(vec!["act"])),
                Arc::new(UInt64Array::from(vec![100])),
                Arc::new(Int64Array::from(vec![123456])),
                Arc::new(Float64Array::from(vec![Some(0.25)])),
                Arc::new(StringArray::from(vec!["{}"])),
                Arc::new(LargeBinaryArray::from(vec![b"model weights".as_slice()])),
            ],
        )
        .unwrap()
    }
    fn connection() -> crate::SpiralConnectionSettings {
        crate::SpiralConnectionSettings::new(
            "http://localhost:1".into(),
            "http://localhost:2".into(),
        )
        .unwrap()
        .with_project("source")
        .unwrap()
    }
    fn data() -> Vec<u8> {
        let entries = Table::ALL
            .into_iter()
            .map(|table| {
                (
                    table.name().to_owned(),
                    arrow_payload(&RecordBatch::new_empty(Arc::new(table.schema()))),
                )
            })
            .collect();
        ProjectBundle
            .build(entries, vec!["empty_dataset".into()])
            .unwrap()
    }
}

#[test]
fn data_archive_preserves_recordings_and_checkpoint_bytes() {
    let sources = RecordBatch::try_new(
        Arc::new(Table::DatasetSources.schema()),
        vec![
            Arc::new(StringArray::from(vec!["demonstrations"])),
            Arc::new(StringArray::from(vec!["take.mcap"])),
            Arc::new(BinaryArray::from(vec![
                b"opaque recording bytes".as_slice(),
            ])),
        ],
    )
    .unwrap();
    let checkpoints = Fixture::checkpoint();
    let entries = vec![
        (Table::DatasetSources.name().into(), arrow_payload(&sources)),
        (
            Table::TrainingCheckpoints.name().into(),
            arrow_payload(&checkpoints),
        ),
    ];
    let data = ProjectBundle
        .build(entries, vec!["demonstrations".into()])
        .unwrap();
    let decoded =
        ProjectBundle::decode(&ProjectBundle::zip(b"sqlite validated separately", &data).unwrap())
            .unwrap();
    assert_eq!(decoded.datasets, ["demonstrations"]);
    assert_eq!(
        decoded.tables,
        vec![
            (Table::DatasetSources, vec![sources]),
            (Table::TrainingCheckpoints, vec![checkpoints]),
        ]
    );
}

#[test]
fn data_archive_rejects_changed_checkpoint_schema() {
    let invalid = record_batch(
        vec![Field::new("checkpoint", DataType::Utf8, false)],
        vec![Arc::new(StringArray::from(vec!["not a checkpoint schema"]))],
    );
    let entries = vec![
        (
            Table::DatasetSources.name().into(),
            arrow_payload(&RecordBatch::new_empty(Arc::new(
                Table::DatasetSources.schema(),
            ))),
        ),
        (
            Table::TrainingCheckpoints.name().into(),
            arrow_payload(&invalid),
        ),
    ];
    let data = ProjectBundle.build(entries, vec![]).unwrap();
    let error = ProjectBundle::decode(&ProjectBundle::zip(b"unused", &data).unwrap())
        .err()
        .unwrap();
    assert!(
        error.contains("training_checkpoints has 1 columns; expected 8"),
        "{error}"
    );
}

#[tokio::test]
async fn version_three_is_exactly_sqlite_and_spiral_data() {
    let store = crate::StudioStore::in_memory();
    let source = store.project(&Fixture::connection());
    store
        .save_backend_settings(serde_json::json!({"secret":"private-credential-123456"}))
        .await
        .unwrap();
    let snapshot = StudioSnapshot::capture(&source).await.unwrap();
    let sqlite = snapshot.bytes().unwrap();
    assert!(sqlite.starts_with(b"SQLite format 3\0"));
    assert!(
        !sqlite
            .windows(b"private-credential-123456".len())
            .any(|window| window == b"private-credential-123456")
    );
    let bytes = ProjectBundle::zip(&sqlite, &Fixture::data()).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(zip.len(), 2);
    assert_eq!(zip.by_index(0).unwrap().name(), "studio.sqlite3");
    assert_eq!(zip.by_index(1).unwrap().name(), "spiral.data");
    let decoded = ProjectBundle::decode(&bytes).unwrap();
    assert_eq!(decoded.datasets, ["empty_dataset"]);
    assert_eq!(decoded.tables.len(), 2);
    StudioSnapshot::open(&decoded.studio, &source)
        .await
        .unwrap();
}

#[test]
fn old_versions_wrong_members_and_corrupt_data_are_rejected() {
    assert!(ProjectBundle::decode(b"SPRPROJ1").is_err());
    let data = Fixture::data();
    let mut bad_version = data.clone();
    let text = std::str::from_utf8(
        &data[12..12 + u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize],
    )
    .unwrap();
    let offset = 12 + text.find("\"format_version\":3").unwrap() + "\"format_version\":".len();
    bad_version[offset] = b'2';
    assert!(
        ProjectBundle::decode(&ProjectBundle::zip(b"unused", &bad_version).unwrap())
            .err()
            .unwrap()
            .contains("unsupported format")
    );
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["../studio.sqlite3", "spiral.data"] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"unused").unwrap();
    }
    assert!(ProjectBundle::decode(&zip.finish().unwrap().into_inner()).is_err());
    let mut truncated = data.clone();
    truncated.pop();
    assert!(ProjectBundle::decode(&ProjectBundle::zip(b"unused", &truncated).unwrap()).is_err());
    let mut trailing = data;
    trailing.push(1);
    assert!(ProjectBundle::decode(&ProjectBundle::zip(b"unused", &trailing).unwrap()).is_err());
}

#[derive(Clone, Default)]
struct FailingSpiral {
    rolled_back: Arc<std::sync::atomic::AtomicBool>,
}
impl crate::Database for FailingSpiral {
    fn connect(&self) -> crate::DatabaseFuture<'_, Box<dyn crate::DatabaseConnection>> {
        use futures::FutureExt;
        let connection = self.clone();
        async move { Ok(Box::new(connection) as Box<dyn crate::DatabaseConnection>) }.boxed()
    }
}
impl crate::DatabaseSession for FailingSpiral {
    fn query(
        &mut self,
        _: crate::SqlStatement,
    ) -> crate::DatabaseFuture<'_, crate::RecordBatchStream> {
        panic!("import must fail before querying")
    }
    fn query_schema(&mut self, _: crate::SqlStatement) -> crate::DatabaseFuture<'_, Arc<Schema>> {
        panic!("import does not plan queries")
    }
    fn execute_update(
        &mut self,
        _: crate::SqlStatement,
    ) -> crate::DatabaseFuture<'_, crate::ExecutionSummary> {
        panic!("no datasets to retire")
    }
    fn ingest(
        &mut self,
        _: crate::BatchIngestion,
    ) -> crate::DatabaseFuture<'_, crate::ExecutionSummary> {
        use futures::FutureExt;
        async {
            Err(crate::DatabaseError::invalid_input(
                "forced Spiral ingestion failure",
            ))
        }
        .boxed()
    }
    fn table_schema(
        &mut self,
        _: crate::RelationName,
    ) -> crate::DatabaseFuture<'_, Option<Arc<Schema>>> {
        use futures::FutureExt;
        async { Ok(None) }.boxed()
    }
    fn relations(&mut self) -> crate::DatabaseFuture<'_, Vec<crate::RelationMetadata>> {
        panic!("import does not enumerate relations")
    }
}
impl crate::DatabaseConnection for FailingSpiral {
    fn begin(
        self: Box<Self>,
    ) -> crate::DatabaseFuture<'static, Box<dyn crate::DatabaseTransaction>> {
        use futures::FutureExt;
        async move { Ok(self as Box<dyn crate::DatabaseTransaction>) }.boxed()
    }
}
impl crate::DatabaseTransaction for FailingSpiral {
    fn commit(self: Box<Self>) -> crate::DatabaseFuture<'static, crate::ExecutionSummary> {
        panic!("failed data import must not commit")
    }
    fn rollback(self: Box<Self>) -> crate::DatabaseFuture<'static, ()> {
        use futures::FutureExt;
        async move {
            self.rolled_back
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        .boxed()
    }
}

#[tokio::test]
async fn spiral_failure_rolls_back_staged_sqlite_replacements() {
    let store = crate::StudioStore::in_memory();
    let settings = Fixture::connection();
    let project = store.project(&settings);
    let snapshot = StudioSnapshot::capture(&project).await.unwrap();
    let bytes = ProjectBundle::zip(&snapshot.bytes().unwrap(), &Fixture::data()).unwrap();
    let original = serde_json::json!({"schema_version":1,"robot_id":"keep","motors":{"joint":{"id":1}},"port":{"path":"/dev/keep","baud_rate":1000000}});
    project
        .save_robot_document(crate::RobotDocumentWrite::new(
            "keep".into(),
            original.clone(),
            None,
        ))
        .await
        .unwrap();
    let source = ProjectBundleImport::new("studio", &project, &bytes).unwrap();
    let database = Arc::new(FailingSpiral::default());
    let spiral = crate::ProjectStore::new(settings, "studio".into(), database.clone()).unwrap();
    let mut connection = spiral.connect_exclusive().await.unwrap();
    let error = ProjectBundle
        .import(&mut connection, source)
        .await
        .unwrap_err();
    assert!(error.contains("forced Spiral ingestion failure"), "{error}");
    assert!(
        database
            .rolled_back
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    assert_eq!(
        project.robot_document("keep").await.unwrap(),
        Some(original)
    );
}
