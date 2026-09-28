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
    fn empty_relations() -> [Vec<u8>; 2] {
        Table::ALL.map(|table| arrow_payload(&RecordBatch::new_empty(Arc::new(table.schema()))))
    }
    /// A bundle holding `relations` in `Table::ALL` order.
    fn bundle(manifest: &Manifest, studio: &[u8], relations: [Vec<u8>; 2]) -> Vec<u8> {
        let mut bundle = BundleWriter::new(manifest, studio).unwrap();
        for (table, payload) in Table::ALL.into_iter().zip(relations) {
            bundle.start(&ProjectBundle::relation_entry(table)).unwrap();
            bundle.write_all(&payload).unwrap();
        }
        bundle.finish().unwrap()
    }
    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
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
    let bytes = Fixture::bundle(
        &Manifest::current(vec!["demonstrations".into()]),
        b"sqlite validated separately",
        [arrow_payload(&sources), arrow_payload(&checkpoints)],
    );
    let decoded = ProjectBundle::decode(&bytes).unwrap();
    assert_eq!(decoded.datasets, ["demonstrations"]);
    assert_eq!(decoded.studio, b"sqlite validated separately");
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
    let [sources, _] = Fixture::empty_relations();
    let bytes = Fixture::bundle(
        &Manifest::current(vec![]),
        b"unused",
        [sources, arrow_payload(&invalid)],
    );
    let error = ProjectBundle::decode(&bytes).err().unwrap();
    assert!(
        error.contains("training_checkpoints has 1 columns; expected 8"),
        "{error}"
    );
}

#[tokio::test]
async fn export_writes_what_decode_reads() {
    let store = crate::StudioStore::in_memory();
    let studio = store.project(&Fixture::connection());
    let spiral = crate::ProjectStore::in_memory();
    let mut connection = spiral.connect_exclusive().await.unwrap();
    connection
        .create_or_append_batches(
            &Table::TrainingCheckpoints.qualified_name(),
            vec![Fixture::checkpoint()],
        )
        .await
        .unwrap();
    let bytes = ProjectBundle
        .export(&mut connection, ProjectBundleExport::new(&studio, vec![]))
        .await
        .unwrap();
    let decoded = ProjectBundle::decode(&bytes).unwrap();
    assert_eq!(decoded.tables[0].1[0].num_rows(), 0);
    // Spiral returns view types; the managed schema accepts either encoding.
    let [exported] = decoded.tables[1].1.as_slice() else {
        panic!("one exported checkpoint batch")
    };
    use arrow_array::cast::AsArray;
    let weights = exported.column_by_name("checkpoint").unwrap();
    assert_eq!(weights.as_binary_view().value(0), b"model weights");
}

#[tokio::test]
async fn format_four_is_a_manifest_sqlite_and_one_arrow_stream_per_relation() {
    let store = crate::StudioStore::in_memory();
    let source = store.project(&Fixture::connection());
    store
        .patch_backend_settings(serde_json::json!({"secret":"private-credential-123456"}))
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
    let bytes = Fixture::bundle(
        &Manifest::current(vec!["empty_dataset".into()]),
        &sqlite,
        Fixture::empty_relations(),
    );
    let zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(
        zip.file_names().collect::<Vec<_>>(),
        [
            "manifest.json",
            "studio.sqlite3",
            "dataset_source.arrows",
            "training_checkpoints.arrows"
        ]
    );
    let decoded = ProjectBundle::decode(&bytes).unwrap();
    assert_eq!(decoded.datasets, ["empty_dataset"]);
    assert_eq!(decoded.tables.len(), 2);
    StudioSnapshot::open(&decoded.studio, &source)
        .await
        .unwrap();
}

#[test]
fn older_formats_wrong_members_and_corrupt_data_are_rejected() {
    let error = |bytes: &[u8]| ProjectBundle::decode(bytes).err().unwrap();
    assert!(error(b"SPRPROJ1").contains("Not a project ZIP"));
    let format_three = Fixture::zip(&[("studio.sqlite3", b"unused"), ("spiral.data", b"unused")]);
    assert!(error(&format_three).contains("older Studio versions"));
    let manifest = Manifest {
        format_version: 3,
        ..Manifest::current(vec![])
    };
    assert!(
        error(&Fixture::bundle(
            &manifest,
            b"unused",
            Fixture::empty_relations()
        ))
        .contains("format 3 is not supported")
    );

    let manifest = serde_json::to_vec(&Manifest::current(vec![])).unwrap();
    let [sources, checkpoints] = Fixture::empty_relations();
    let members = |studio: &str, extra: &[(&str, &[u8])]| {
        let mut entries = vec![
            ("manifest.json", manifest.as_slice()),
            (studio, b"unused".as_slice()),
            ("dataset_source.arrows", sources.as_slice()),
            ("training_checkpoints.arrows", checkpoints.as_slice()),
        ];
        entries.extend_from_slice(extra);
        Fixture::zip(&entries)
    };
    assert!(ProjectBundle::decode(&members("studio.sqlite3", &[])).is_ok());
    assert!(error(&members("../studio.sqlite3", &[])).contains("missing studio.sqlite3"));
    assert!(
        error(&members("studio.sqlite3", &[("notes.txt", b"unused")]))
            .contains("must contain exactly")
    );

    let mut truncated = sources.clone();
    truncated.pop();
    let mut trailing = sources.clone();
    trailing.push(1);
    for corrupt in [truncated, trailing] {
        let bytes = Fixture::bundle(
            &Manifest::current(vec![]),
            b"unused",
            [corrupt, checkpoints.clone()],
        );
        assert!(ProjectBundle::decode(&bytes).is_err());
    }
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
    let bytes = Fixture::bundle(
        &Manifest::current(vec!["empty_dataset".into()]),
        &snapshot.bytes().unwrap(),
        Fixture::empty_relations(),
    );
    let original =
        spiral_robotics_common::robots::config::RobotConfig::so101("keep", "so101_follower")
            .unwrap();
    project.save_robot_config(&original, None).await.unwrap();
    let source = ProjectBundleImport::new(&project, &bytes).unwrap();
    let database = Arc::new(FailingSpiral::default());
    let spiral = crate::ProjectStore::new(
        crate::SpiralConnection::new(settings, crate::SpiralAccess::fixed("token".into())),
        database.clone(),
    )
    .unwrap();
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
        project.robot_config("keep").await.unwrap(),
        Some((original, 1))
    );
}
