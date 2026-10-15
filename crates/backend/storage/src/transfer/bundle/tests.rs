use super::*;
use arrow_array::{ArrayRef, BinaryArray, NullArray, StringArray, UInt64Array};
use arrow_ipc::writer::IpcWriteOptions;
use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    fn connection() -> crate::SpiralConnectionSettings {
        crate::SpiralConnectionSettings::new(
            "http://localhost:1".into(),
            "http://localhost:2".into(),
        )
        .unwrap()
        .with_project("source")
        .unwrap()
    }
    fn empty_relations() -> [Vec<u8>; 1] {
        Table::ALL.map(|table| arrow_payload(&RecordBatch::new_empty(Arc::new(table.schema()))))
    }
    /// A bundle holding `relations` in `Table::ALL` order.
    fn bundle(manifest: &Manifest, studio: &[u8], relations: [Vec<u8>; 1]) -> Vec<u8> {
        let mut bundle = BundleWriter::new(manifest, studio).unwrap();
        for (table, payload) in Table::ALL.into_iter().zip(relations) {
            bundle.start(&ProjectBundle::relation_entry(table)).unwrap();
            bundle.write_all(&payload).unwrap();
        }
        bundle.finish().unwrap()
    }
    /// The `dataset_source` rows of `files`, each its dataset, key and
    /// source.
    fn sources(files: &[(&str, &str, BundledSource<'_>)]) -> RecordBatch {
        let (mut held, mut uris, mut sizes) = (Vec::new(), Vec::new(), Vec::new());
        for (_, _, source) in files {
            let (bytes, uri, size) = match *source {
                BundledSource::Held(bytes) => (Some(bytes), None, bytes.len() as u64),
                BundledSource::Pinned { uri, size } => (None, Some(uri), size),
            };
            held.push(bytes);
            uris.push(uri);
            sizes.push(size);
        }
        RecordBatch::try_new(
            Arc::new(Table::DatasetSources.schema()),
            vec![
                Arc::new(StringArray::from_iter_values(
                    files.iter().map(|(dataset, _, _)| *dataset),
                )),
                Arc::new(StringArray::from_iter_values(
                    files.iter().map(|(_, key, _)| *key),
                )),
                Arc::new(BinaryArray::from(held)),
                Arc::new(StringArray::from(uris)),
                Arc::new(UInt64Array::from(sizes)),
            ],
        )
        .unwrap()
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
fn data_archive_preserves_held_bytes_and_pinned_uris() {
    let sources = Fixture::sources(&[
        (
            "demonstrations",
            "take.mcap",
            BundledSource::Held(b"opaque recording bytes"),
        ),
        (
            "demonstrations",
            "episodes/take.mcap",
            BundledSource::Pinned {
                uri: "hf://datasets/org/recordings@abcdef/episodes/take.mcap",
                size: 9,
            },
        ),
    ]);
    let bytes = Fixture::bundle(
        &Manifest::current(vec!["demonstrations".into()]),
        b"sqlite validated separately",
        [arrow_payload(&sources)],
    );
    let decoded = ProjectBundle::decode(&bytes).unwrap();
    assert_eq!(
        decoded.datasets,
        [DatasetSelection::new("demonstrations").unwrap()]
    );
    assert_eq!(decoded.studio, b"sqlite validated separately");
    assert_eq!(decoded.tables, vec![(Table::DatasetSources, vec![sources])]);
}

#[test]
fn data_archive_rejects_changed_source_schema() {
    let invalid = record_batch(
        vec![Field::new("recording", DataType::Utf8, false)],
        vec![Arc::new(StringArray::from(vec!["not a source schema"]))],
    );
    let bytes = Fixture::bundle(
        &Manifest::current(vec![]),
        b"unused",
        [arrow_payload(&invalid)],
    );
    let error = ProjectBundle::decode(&bytes).err().unwrap();
    assert!(
        error.contains("dataset_source has 1 columns; expected 5"),
        "{error}"
    );
}

#[tokio::test]
async fn export_writes_what_decode_reads() {
    let store = crate::StudioStore::in_memory();
    let studio = store.project(&Fixture::connection());
    let spiral = crate::ProjectStore::in_memory();
    let mut connection = spiral.connect_exclusive().await.unwrap();
    let bytes = ProjectBundle
        .export(&mut connection, ProjectBundleExport::new(&studio, vec![]))
        .await
        .unwrap();
    let decoded = ProjectBundle::decode(&bytes).unwrap();
    assert_eq!(decoded.tables.len(), 1);
    assert_eq!(decoded.tables[0].1[0].num_rows(), 0);
}

#[tokio::test]
async fn format_six_is_a_manifest_sqlite_and_one_arrow_stream_per_relation() {
    let store = crate::StudioStore::in_memory();
    let source = store.project(&Fixture::connection());
    crate::SessionStore::new(store.clone())
        .save(
            "browser",
            &crate::StoredSession::new(
                "access".into(),
                "private-credential-123456".into(),
                "source".into(),
            ),
        )
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
        ["manifest.json", "studio.sqlite3", "dataset_source.arrows"]
    );
    let decoded = ProjectBundle::decode(&bytes).unwrap();
    assert_eq!(
        decoded.datasets,
        [DatasetSelection::new("empty_dataset").unwrap()]
    );
    assert_eq!(decoded.tables.len(), 1);
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
        format_version: 5,
        ..Manifest::current(vec![])
    };
    assert!(
        error(&Fixture::bundle(
            &manifest,
            b"unused",
            Fixture::empty_relations()
        ))
        .contains("format 5 is not supported")
    );

    let manifest = serde_json::to_vec(&Manifest::current(vec![])).unwrap();
    let [sources] = Fixture::empty_relations();
    let members = |studio: &str, extra: &[(&str, &[u8])]| {
        let mut entries = vec![
            ("manifest.json", manifest.as_slice()),
            (studio, b"unused".as_slice()),
            ("dataset_source.arrows", sources.as_slice()),
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
        let bytes = Fixture::bundle(&Manifest::current(vec![]), b"unused", [corrupt]);
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
        panic!("the project holds no tables to drop")
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
    let original = spiral_robotics_common::curation::CurationDocument::new("keep", "Keep", 1, "{}");
    assert!(
        crate::curations::CurationPersistence::new(&project)
            .save(&original)
            .await
            .unwrap()
    );
    let source = ProjectBundleImport::new(&project, &bytes, PinnedMcapFiles::new(None)).unwrap();
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
        crate::curations::CurationPersistence::new(&project)
            .load("keep")
            .await
            .unwrap(),
        Some(original.with_revision(1))
    );
}

/// A project over the in-memory engine whose journal names an MCAP dataset
/// `old` holding one episode and an empty LeRobot dataset `svla`.
struct HeldDatasets {
    /// Serves the catalog while the project lives.
    _journal: crate::datasets::TestJournal,
    spiral: Arc<crate::ProjectStore>,
    studio: crate::StudioStore,
}

impl HeldDatasets {
    async fn new() -> Self {
        use crate::datasets::fixtures::RolloutFixture;
        use crate::datasets::{DatasetImport, DatasetWrite, DecodedEpisode, McapFile};
        let journal = crate::datasets::TestJournal::start().await;
        let (spiral, _) = crate::db::UnquotedMemory::store_with_catalog(journal.endpoint()).await;
        let old = DatasetSelection::new("old").unwrap();
        let take = RolloutFixture::mcap("take", false);
        DatasetImport::new(&spiral, &old, DatasetFormat::Mcap)
            .commit(vec![
                DecodedEpisode::of_mcap(McapFile::held("take.mcap", &take).unwrap(), &take)
                    .unwrap(),
            ])
            .await
            .unwrap();
        let svla = DatasetSelection::new("svla").unwrap();
        let mut connection = spiral.connect_exclusive().await.unwrap();
        DatasetWrite::create(&mut connection, &svla, DatasetFormat::LeRobot)
            .await
            .unwrap()
            .finish(&mut connection)
            .await
            .unwrap();
        drop(connection);
        journal.name(&[
            old.episodes(),
            old.topics(),
            old.files(),
            svla.episodes(),
            svla.topics(),
        ]);
        Self {
            _journal: journal,
            spiral: Arc::new(spiral),
            studio: crate::StudioStore::in_memory(),
        }
    }

    fn storage(&self) -> crate::StorageManager {
        crate::StorageManager::new(
            self.spiral.clone(),
            self.studio.project(&self.spiral.connection_settings()),
        )
    }

    /// A bundle of this project's Studio records whose `files` rows hold
    /// one take in `demo`, and whose manifest names `datasets`.
    async fn bundle(&self, datasets: &[&str]) -> Vec<u8> {
        use crate::datasets::fixtures::RolloutFixture;
        let studio = self.studio.project(&self.spiral.connection_settings());
        let snapshot = StudioSnapshot::capture(&studio).await.unwrap();
        let take = RolloutFixture::mcap("take", true);
        let sources = Fixture::sources(&[("demo", "take.mcap", BundledSource::Held(&take))]);
        Fixture::bundle(
            &Manifest::current(datasets.iter().map(|name| (*name).to_owned()).collect()),
            &snapshot.bytes().unwrap(),
            [arrow_payload(&sources)],
        )
    }

    /// How many rows `table` holds, or why it cannot be read.
    async fn rows(&self, table: DatasetTable) -> Result<usize, String> {
        let batches = self
            .spiral
            .connect()
            .await?
            .read_batches(&format!("SELECT * FROM {table}"))
            .await?;
        Ok(batches.iter().map(RecordBatch::num_rows).sum())
    }
}

#[tokio::test]
async fn an_import_replaces_the_mcap_datasets_and_leaves_lerobot_ones() {
    let project = HeldDatasets::new().await;
    let bytes = project.bundle(&["demo", "empty"]).await;
    let summary = project
        .storage()
        .transfer()
        .import_project(&bytes, None)
        .await
        .unwrap();
    assert_eq!((summary.relations(), summary.rows()), (1, 1));
    let demo = DatasetSelection::new("demo").unwrap();
    let empty = DatasetSelection::new("empty").unwrap();
    let svla = DatasetSelection::new("svla").unwrap();
    for (table, expected) in [
        (demo.episodes(), 1),
        (demo.files(), 1),
        (empty.episodes(), 0),
        (empty.topics(), 0),
        (empty.files(), 0),
        (svla.episodes(), 0),
    ] {
        assert_eq!(
            project.rows(table.clone()).await,
            Ok(expected),
            "{}",
            table.target()
        );
    }
    let old = DatasetSelection::new("old").unwrap();
    assert!(
        project.rows(old.episodes()).await.is_err(),
        "the project's MCAP datasets are dropped"
    );
    assert!(project.rows(old.files()).await.is_err());
}

#[tokio::test]
async fn an_import_refuses_a_bundle_naming_a_lerobot_dataset() {
    let project = HeldDatasets::new().await;
    let bytes = project.bundle(&["demo", "svla"]).await;
    let error = project
        .storage()
        .transfer()
        .import_project(&bytes, None)
        .await
        .unwrap_err();
    assert!(
        error.contains("datasets.svla is no MCAP dataset"),
        "{error}"
    );
    let old = DatasetSelection::new("old").unwrap();
    assert_eq!(project.rows(old.episodes()).await, Ok(1), "nothing changed");
}

/// A stand-in for Hugging Face serving one file at a commit, counting its
/// downloads.
struct FakeHuggingFace {
    origin: String,
    downloads: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeHuggingFace {
    const FILE: &str = "episodes/take.mcap";
    const URI: &str = "hf://datasets/org/recordings@abcdef/episodes/take.mcap";

    /// `bytes` as [`Self::URI`]'s file.
    async fn serving(bytes: Vec<u8>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let downloads = Arc::new(AtomicUsize::new(0));
        let counted = downloads.clone();
        let app = axum::Router::new().route(
            "/datasets/org/recordings/resolve/abcdef/episodes/take.mcap",
            axum::routing::get(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                std::future::ready(bytes.clone())
            }),
        );
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            origin,
            downloads,
            task,
        }
    }

    fn pinned(&self) -> PinnedMcapFiles {
        PinnedMcapFiles::at(&self.origin)
    }

    fn downloads(&self) -> usize {
        self.downloads.load(Ordering::SeqCst)
    }
}

impl Drop for FakeHuggingFace {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// `dataset`'s `files` rows in `spiral`, as a bundle carries them.
async fn bundled_rows(
    spiral: &crate::ProjectStore,
    dataset: &DatasetSelection,
) -> Vec<RecordBatch> {
    let mut connection = spiral.connect_exclusive().await.unwrap();
    let batches = connection
        .read_batches(&McapSources::export_sql(dataset))
        .await
        .unwrap();
    batches
        .iter()
        .map(|batch| McapSources::bundled(dataset, batch).unwrap())
        .collect()
}

#[tokio::test]
async fn a_bundle_points_at_pinned_files_and_its_import_downloads_them() {
    use crate::datasets::fixtures::RolloutFixture;
    use crate::datasets::{DatasetImport, DecodedEpisode};
    let take = RolloutFixture::mcap("take", false);
    let (source, _) = crate::db::UnquotedMemory::store().await;
    let held = DatasetSelection::new("held").unwrap();
    let pinned = DatasetSelection::new("pinned").unwrap();
    for (dataset, file) in [
        (&held, McapFile::held("take.mcap", &take)),
        (
            &pinned,
            McapFile::pinned(FakeHuggingFace::FILE, FakeHuggingFace::URI.into(), &take),
        ),
    ] {
        DatasetImport::new(&source, dataset, DatasetFormat::Mcap)
            .commit(vec![DecodedEpisode::of_mcap(file.unwrap(), &take).unwrap()])
            .await
            .unwrap();
    }
    let studio = crate::StudioStore::in_memory();
    let bytes = ProjectBundle
        .export(
            &mut source.connect_exclusive().await.unwrap(),
            ProjectBundleExport::new(
                &studio.project(&source.connection_settings()),
                vec![held.clone(), pinned.clone()],
            ),
        )
        .await
        .unwrap();
    let decoded = ProjectBundle::decode(&bytes).unwrap();
    let carried = McapSources::files(&decoded.tables[0].1, &decoded.datasets).unwrap();
    let files = |dataset: &DatasetSelection| {
        carried[dataset]
            .iter()
            .map(|file| (file.key(), file.source()))
            .collect::<Vec<_>>()
    };
    let pinned_take = BundledSource::Pinned {
        uri: FakeHuggingFace::URI,
        size: take.len() as u64,
    };
    assert_eq!(files(&held), [("take.mcap", BundledSource::Held(&take))]);
    assert_eq!(files(&pinned), [(FakeHuggingFace::FILE, pinned_take)]);

    let huggingface = FakeHuggingFace::serving(take.clone()).await;
    let (target, _) = crate::db::UnquotedMemory::store().await;
    let studio = crate::StudioStore::in_memory();
    let summary = ProjectBundle
        .import(
            &mut target.connect_exclusive().await.unwrap(),
            ProjectBundleImport::new(
                &studio.project(&target.connection_settings()),
                &bytes,
                huggingface.pinned(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!((summary.relations(), summary.rows()), (1, 2));
    assert_eq!(huggingface.downloads(), 1);
    for (dataset, expected) in [(&held, BundledSource::Held(&take)), (&pinned, pinned_take)] {
        let rows = bundled_rows(&target, dataset).await;
        let restored = McapSources::files(&rows, std::slice::from_ref(dataset)).unwrap();
        assert_eq!(
            restored[dataset]
                .iter()
                .map(BundledFile::source)
                .collect::<Vec<_>>(),
            [expected],
            "{}",
            dataset.label()
        );
        let episodes = target
            .connect()
            .await
            .unwrap()
            .read_batches(&format!("SELECT * FROM {}", dataset.episodes()))
            .await
            .unwrap();
        assert_eq!(episodes.iter().map(RecordBatch::num_rows).sum::<usize>(), 1);
    }
}

#[tokio::test]
async fn an_import_refuses_a_pinned_file_of_another_size() {
    use crate::datasets::fixtures::RolloutFixture;
    let take = RolloutFixture::mcap("take", false);
    let huggingface = FakeHuggingFace::serving(take.clone()).await;
    let (target, _) = crate::db::UnquotedMemory::store().await;
    let studio = crate::StudioStore::in_memory();
    let project = studio.project(&target.connection_settings());
    let sources = Fixture::sources(&[(
        "pinned",
        FakeHuggingFace::FILE,
        BundledSource::Pinned {
            uri: FakeHuggingFace::URI,
            size: take.len() as u64 + 1,
        },
    )]);
    let bytes = Fixture::bundle(
        &Manifest::current(vec!["pinned".into()]),
        &StudioSnapshot::capture(&project)
            .await
            .unwrap()
            .bytes()
            .unwrap(),
        [arrow_payload(&sources)],
    );
    let error = ProjectBundle
        .import(
            &mut target.connect_exclusive().await.unwrap(),
            ProjectBundleImport::new(&project, &bytes, huggingface.pinned()).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("its dataset recorded"), "{error}");
}
