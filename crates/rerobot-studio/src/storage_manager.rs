//! Publishing, query preview, and remote replay for backend-owned datasets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use adbc_core::error::Status as AdbcStatus;
use adbc_core::options::{
    AdbcVersion, IngestMode, OptionConnection, OptionDatabase, OptionStatement,
};
use adbc_core::{Connection as _, Database as _, Driver as _, Optionable as _, Statement as _};
use adbc_driver_manager::{ManagedConnection, ManagedDatabase, ManagedDriver};
use arrow_array::{
    Array as _, ArrayRef, BinaryArray, BinaryViewArray, LargeBinaryArray, RecordBatch, StringArray,
    UInt32Array, UInt64Array,
};
use arrow_json::writer::{JsonArray, WriterBuilder};
use arrow_schema::{DataType, Field, Schema};
use hf_hub::HFClient;
use hf_hub::repository::CommitOperation;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use rerobot_core::api::{
    ConnectionTestResponse, PublicationStatus, QueryColumn, SpiralQueryResponse,
};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::data_settings::{DataSettingsStore, StoredDataSettings, StoredSpiralSettings};
use crate::episodes::{CompletedEpisode, EpisodeStore};
use crate::spiral_episode::{
    EPISODE_STREAMS_TABLE, EPISODES_TABLE, JOINT_SAMPLES_TABLE, SpiralEpisodeEncoder,
    VIDEO_ASSETS_TABLE, VideoStreamRow, episode_completion_batch, episode_streams_batch,
    joint_samples_batch, joint_samples_schema, video_streams_batch,
};
use crate::video_capture::{VideoAsset, load_assets, save_assets};

const MAX_QUERY_ROWS: usize = 1_000;
const MAX_REPLAY_ROWS: usize = 250_000;
const JOINT_BATCH_ROWS: usize = 512;

pub struct StorageManager {
    pub settings: Arc<DataSettingsStore>,
    episodes: Arc<EpisodeStore>,
    driver_path: Option<PathBuf>,
    adbc: Arc<Mutex<Option<CachedAdbc>>>,
    publications: Mutex<BTreeMap<String, PublicationStatus>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AdbcConnectionKey {
    driver_path: PathBuf,
    uri: String,
    project_id: String,
}

struct CachedAdbc {
    key: AdbcConnectionKey,
    _driver: ManagedDriver,
    _database: ManagedDatabase,
    connection: ManagedConnection,
}

impl StorageManager {
    pub fn new(
        settings: Arc<DataSettingsStore>,
        episodes: Arc<EpisodeStore>,
        driver_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        Ok(Self {
            settings,
            episodes,
            driver_path,
            adbc: Arc::new(Mutex::new(None)),
            publications: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn statuses(&self) -> Vec<PublicationStatus> {
        lock(&self.publications).values().cloned().collect()
    }

    /// Queue completed publication after the local file and camera files have
    /// closed. Robot control is already idle before this task performs I/O.
    pub fn queue_publish(self: &Arc<Self>, episode_id: String, videos: Vec<VideoAsset>) {
        if let Err(error) = save_assets(self.episodes.directory(), &episode_id, &videos) {
            tracing::warn!(episode = %episode_id, "could not persist video publication manifest: {error}");
        }
        self.set_status(PublicationStatus {
            episode_id: episode_id.clone(),
            state: "queued".to_owned(),
            updated_ms: unix_millis(),
            ..PublicationStatus::default()
        });
        let manager = self.clone();
        tokio::spawn(async move {
            manager.publish(episode_id, videos).await;
        });
    }

    pub fn publish_existing(self: &Arc<Self>, episode_id: String) -> Result<(), String> {
        if lock(&self.publications)
            .get(&episode_id)
            .is_some_and(|status| matches!(status.state.as_str(), "queued" | "publishing"))
        {
            return Err(format!("{episode_id} is already being published"));
        }
        self.episodes
            .completed(&episode_id)
            .map_err(|error| error.to_string())?;
        let videos = load_assets(self.episodes.directory(), &episode_id)?;
        self.queue_publish(episode_id, videos);
        Ok(())
    }

    async fn publish(&self, episode_id: String, videos: Vec<VideoAsset>) {
        let mut status = PublicationStatus {
            episode_id: episode_id.clone(),
            state: "publishing".to_owned(),
            updated_ms: unix_millis(),
            ..PublicationStatus::default()
        };
        self.set_status(status.clone());
        let settings = self.settings.get();
        let completed = match self.episodes.completed(&episode_id) {
            Ok(completed) => completed,
            Err(error) => {
                status.state = "failed".to_owned();
                status.spiral = Some(error.to_string());
                status.updated_ms = unix_millis();
                self.set_status(status);
                return;
            }
        };

        if !settings.spiral.enabled && !settings.hugging_face.enabled {
            status.state = "local".to_owned();
            status.updated_ms = unix_millis();
            self.set_status(status);
            return;
        }

        // Rejected takes remain in the local recovery spool so the judgement
        // can be changed later, but they must never enter a training store.
        if completed.completion.discarded {
            status.state = "local".to_owned();
            status.updated_ms = unix_millis();
            self.set_status(status);
            return;
        }

        if settings.spiral.enabled {
            status.spiral = Some("publishing".to_owned());
            status.updated_ms = unix_millis();
            self.set_status(status.clone());
            status.spiral = Some(
                match self
                    .publish_spiral(&settings.spiral, &completed, &videos)
                    .await
                {
                    Ok(()) => "published".to_owned(),
                    Err(error) => format!("failed: {error}"),
                },
            );
        }
        if settings.hugging_face.enabled {
            status.hugging_face = Some("publishing".to_owned());
            status.updated_ms = unix_millis();
            self.set_status(status.clone());
            status.hugging_face = Some(
                match publish_hugging_face(&settings, &completed, &videos).await {
                    Ok(()) => "published".to_owned(),
                    Err(error) => format!("failed: {error}"),
                },
            );
        }
        let failed = status
            .spiral
            .iter()
            .chain(status.hugging_face.iter())
            .any(|value| value.starts_with("failed:"));
        status.state = if failed { "failed" } else { "published" }.to_owned();
        status.updated_ms = unix_millis();
        self.set_status(status);
    }

    pub async fn test_spiral(&self) -> ConnectionTestResponse {
        let settings = self.settings.get().spiral;
        match self
            .run_adbc(settings, |connection| {
                run_query(connection, "SELECT 1 AS connected", 1)
            })
            .await
        {
            Ok(_) => ConnectionTestResponse {
                ok: true,
                message: "Connected through the SpiralDB ADBC driver".to_owned(),
            },
            Err(error) => ConnectionTestResponse {
                ok: false,
                message: error,
            },
        }
    }

    pub async fn test_hugging_face(&self) -> ConnectionTestResponse {
        let settings = self.settings.get();
        match hugging_face_repository(&settings) {
            Ok(repository) => match repository.exists().send().await {
                Ok(true) => ConnectionTestResponse {
                    ok: true,
                    message: "Connected to the Hugging Face dataset".to_owned(),
                },
                Ok(false) => ConnectionTestResponse {
                    ok: false,
                    message: "Token worked, but the dataset repository was not found".to_owned(),
                },
                Err(error) => ConnectionTestResponse {
                    ok: false,
                    message: error.to_string(),
                },
            },
            Err(error) => ConnectionTestResponse {
                ok: false,
                message: error,
            },
        }
    }

    pub async fn query(&self, sql: &str, max_rows: usize) -> Result<SpiralQueryResponse, String> {
        ensure_read_only_sql(sql)?;
        let settings = self.settings.get().spiral;
        let sql = sql.to_owned();
        self.run_adbc(settings, move |connection| {
            run_query(connection, &sql, max_rows.clamp(1, MAX_QUERY_ROWS))
        })
        .await
    }

    pub async fn trajectory(
        &self,
        episode: &str,
        stream: &str,
    ) -> Result<(f64, Vec<BTreeMap<String, f64>>), String> {
        let settings = self.settings.get().spiral;
        validate_safe_value("episode", episode)?;
        validate_safe_value("stream", stream)?;
        let table = |name: &str| format!("{}.{}", settings.dataset, name);
        let sql = format!(
            "SELECT e.sample_hz, s.joint_names, \
             COALESCE(j.action_position, j.state_position) AS replay_position \
             FROM {} e JOIN {} s ON e.episode_id = s.episode_id \
             JOIN {} j ON j.episode_id = s.episode_id AND j.stream_id = s.stream_id \
             WHERE e.episode_id = '{}' AND s.stream_id = '{}' ORDER BY j.sample_index",
            table(EPISODES_TABLE),
            table(EPISODE_STREAMS_TABLE),
            table(JOINT_SAMPLES_TABLE),
            episode,
            stream,
        );
        let result = self
            .run_adbc(settings, move |connection| {
                run_query(connection, &sql, MAX_REPLAY_ROWS)
            })
            .await?;
        if result.truncated {
            return Err(format!(
                "Spiral trajectory exceeds the replay safety limit of {MAX_REPLAY_ROWS} rows"
            ));
        }
        let mut sample_hz = None;
        let mut trajectory = Vec::with_capacity(result.rows.len());
        for row in result.rows {
            sample_hz = sample_hz.or_else(|| row.get("sample_hz").and_then(JsonValue::as_f64));
            let names = row
                .get("joint_names")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| "Spiral trajectory is missing joint_names".to_owned())?;
            let values = row
                .get("replay_position")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| "Spiral trajectory is missing replay_position".to_owned())?;
            let pose = names
                .iter()
                .zip(values)
                .filter_map(|(name, value)| Some((name.as_str()?.to_owned(), value.as_f64()?)))
                .collect::<BTreeMap<_, _>>();
            if !pose.is_empty() {
                trajectory.push(pose);
            }
        }
        if trajectory.is_empty() {
            return Err("Spiral returned no commandable trajectory rows".to_owned());
        }
        Ok((sample_hz.unwrap_or(60.0), trajectory))
    }

    /// Fetch one original MP4 payload for browser preview. Spiral's managed
    /// video dataset remains the compute/index surface; the byte asset is the
    /// correct browser delivery format and avoids decoding/re-encoding frames.
    pub async fn video_asset(&self, episode: &str, stream: &str) -> Result<Vec<u8>, String> {
        let settings = self.settings.get().spiral;
        validate_safe_value("episode", episode)?;
        validate_safe_value("stream", stream)?;
        let sql = format!(
            "SELECT payload FROM {}.{} WHERE episode_id = '{}' AND stream_id = '{}' LIMIT 1",
            settings.dataset, VIDEO_ASSETS_TABLE, episode, stream
        );
        self.run_adbc(settings, move |connection| {
            run_binary_query(connection, &sql)
        })
        .await
    }

    async fn publish_spiral(
        &self,
        settings: &StoredSpiralSettings,
        completed: &CompletedEpisode,
        videos: &[VideoAsset],
    ) -> Result<(), String> {
        let settings = settings.clone();
        let completed = completed.clone();
        let videos = videos.to_vec();
        self.run_adbc(settings.clone(), move |connection| {
            publish_spiral_blocking(connection, &settings, &completed, &videos)
        })
        .await
    }

    async fn run_adbc<T, F>(
        &self,
        settings: StoredSpiralSettings,
        operation: F,
    ) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut ManagedConnection) -> Result<T, String> + Send + 'static,
    {
        let driver_path = self.driver_path.clone().ok_or_else(|| {
            "Spiral ADBC driver is not configured on this backend; start it with --spiral-adbc-driver or SPIRALDB_ADBC_DRIVER".to_owned()
        })?;
        validate_adbc_settings(&settings, &driver_path)?;
        let adbc = self.adbc.clone();
        tokio::task::spawn_blocking(move || {
            let key = AdbcConnectionKey::new(&settings, driver_path);
            let mut cached = lock(&adbc);
            if cached.as_ref().is_none_or(|cached| cached.key != key) {
                *cached = Some(open_adbc(key)?);
            }
            let connection = &mut cached
                .as_mut()
                .expect("ADBC connection was just installed")
                .connection;
            operation(connection)
        })
        .await
        .map_err(|error| format!("Spiral ADBC worker failed: {error}"))?
    }

    fn set_status(&self, status: PublicationStatus) {
        lock(&self.publications).insert(status.episode_id.clone(), status);
    }
}

impl AdbcConnectionKey {
    fn new(settings: &StoredSpiralSettings, driver_path: PathBuf) -> Self {
        Self {
            driver_path,
            uri: settings.uri.clone(),
            project_id: settings.project_id.clone(),
        }
    }
}

fn open_adbc(key: AdbcConnectionKey) -> Result<CachedAdbc, String> {
    let mut driver = ManagedDriver::load_dynamic_from_filename(
        &key.driver_path,
        Some(b"AdbcDriverSpiraldbInit"),
        AdbcVersion::V110,
    )
    .map_err(|error| {
        format!(
            "loading Spiral ADBC driver {}: {error}",
            key.driver_path.display()
        )
    })?;
    let database = driver
        .new_database_with_opts([(OptionDatabase::Uri, key.uri.clone().into())])
        .map_err(|error| format!("opening Spiral ADBC database {}: {error}", key.uri))?;
    let connection = if key.project_id.is_empty() {
        database.new_connection()
    } else {
        database.new_connection_with_opts([(
            OptionConnection::CurrentSchema,
            key.project_id.clone().into(),
        )])
    }
    .map_err(|error| format!("connecting Spiral ADBC database: {error}"))?;
    Ok(CachedAdbc {
        key,
        _driver: driver,
        _database: database,
        connection,
    })
}

fn validate_adbc_settings(settings: &StoredSpiralSettings, driver: &Path) -> Result<(), String> {
    if !driver.is_absolute() || !driver.is_file() {
        return Err("Spiral ADBC driver must be an existing absolute file path".to_owned());
    }
    if !matches!(settings.uri.as_str(), "spiraldb://" | "spiral:memory:") {
        return Err("Spiral ADBC URI must be 'spiraldb://' or 'spiral:memory:'".to_owned());
    }
    if settings.uri != "spiral:memory:" {
        validate_safe_value("Spiral project", &settings.project_id)?;
    }
    validate_dataset_identifier(&settings.dataset)
}

async fn publish_hugging_face(
    settings: &StoredDataSettings,
    episode: &CompletedEpisode,
    videos: &[VideoAsset],
) -> Result<(), String> {
    let repository = hugging_face_repository(settings)?;
    let parquet = hugging_face_episode_parquet(episode)?;
    let mut operations = vec![
        CommitOperation::add_bytes(format!("data/{}.parquet", episode.start.id), parquet),
        // The lossless local format remains available for recovery and future
        // migrations, but the primary HF dataset surface is Arrow/Parquet.
        CommitOperation::add_file(
            format!("recovery/{}.jsonl", episode.start.id),
            &episode.path,
        ),
    ];
    for video in videos {
        operations.push(CommitOperation::add_file(
            format!(
                "videos/{}/{}.mp4",
                episode.start.id,
                safe_repo_component(&video.stream_id)
            ),
            &video.path,
        ));
    }
    let metadata = serde_json::to_vec_pretty(&serde_json::json!({
        "episode_id": episode.start.id,
        "started_ms": episode.start.started_ms,
        "sample_hz": episode.start.sample_hz,
        "source": episode.start.source,
        "arms": episode.start.arms,
        "frames": episode.samples.len(),
        "rows": episode.samples.len() * episode.start.arms.len(),
        "duration_seconds": episode.completion.duration_seconds,
        "video_streams": videos.iter().map(|video| &video.stream_id).collect::<Vec<_>>(),
    }))
    .map_err(|error| format!("encoding Hugging Face metadata: {error}"))?;
    operations.push(CommitOperation::add_bytes(
        format!("meta/episodes/{}.json", episode.start.id),
        metadata,
    ));
    repository
        .create_commit()
        .operations(operations)
        .commit_message(format!("Add {}", episode.start.id))
        .revision(settings.hugging_face.revision.clone())
        .send()
        .await
        .map_err(|error| format!("Hugging Face upload failed: {error}"))?;
    Ok(())
}

fn hugging_face_episode_parquet(episode: &CompletedEpisode) -> Result<Vec<u8>, String> {
    let properties = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(Vec::new(), joint_samples_schema(), Some(properties))
        .map_err(|error| format!("opening Hugging Face Parquet writer: {error}"))?;
    let mut encoder =
        SpiralEpisodeEncoder::new(episode.start.clone()).map_err(|error| error.to_string())?;
    let mut rows = Vec::with_capacity(JOINT_BATCH_ROWS);
    for sample in &episode.samples {
        rows.extend(encoder.encode(sample).map_err(|error| error.to_string())?);
        if rows.len() >= JOINT_BATCH_ROWS {
            writer
                .write(&joint_samples_batch(&rows).map_err(|error| error.to_string())?)
                .map_err(|error| format!("writing Hugging Face Parquet rows: {error}"))?;
            rows.clear();
        }
    }
    if !rows.is_empty() {
        writer
            .write(&joint_samples_batch(&rows).map_err(|error| error.to_string())?)
            .map_err(|error| format!("writing Hugging Face Parquet rows: {error}"))?;
    }
    writer
        .into_inner()
        .map_err(|error| format!("finishing Hugging Face Parquet: {error}"))
}

fn hugging_face_repository(
    settings: &StoredDataSettings,
) -> Result<hf_hub::repository::HFRepository<hf_hub::repository::RepoTypeDataset>, String> {
    let token = settings
        .hugging_face
        .token
        .as_deref()
        .ok_or_else(|| "Hugging Face token is not configured".to_owned())?;
    let (owner, name) = settings
        .hugging_face
        .repository
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .ok_or_else(|| "Hugging Face repository must use owner/name".to_owned())?;
    let client = HFClient::builder()
        .token(token)
        .build()
        .map_err(|error| format!("building Hugging Face client: {error}"))?;
    Ok(client.dataset(owner, name))
}

fn publish_spiral_blocking(
    connection: &mut ManagedConnection,
    settings: &StoredSpiralSettings,
    episode: &CompletedEpisode,
    videos: &[VideoAsset],
) -> Result<(), String> {
    validate_safe_value("episode", &episode.start.id)?;
    if settings.uri == "spiral:memory:" {
        // The driver's offline catalog intentionally has no mutable relation
        // catalog, so SQL DELETE is unavailable there. A completed retry is
        // already successful; a partial retry must restart the disposable
        // memory connection instead of silently duplicating rows.
        if table_has_episode(
            connection,
            &table_name(&settings.dataset, EPISODES_TABLE),
            &episode.start.id,
        )? {
            return Ok(());
        }
        for table in [
            JOINT_SAMPLES_TABLE,
            EPISODE_STREAMS_TABLE,
            VIDEO_ASSETS_TABLE,
        ] {
            if table_has_episode(
                connection,
                &table_name(&settings.dataset, table),
                &episode.start.id,
            )? {
                return Err(
                    "Spiral's disposable memory catalog contains a partial copy of this episode; restart the backend before retrying"
                        .to_owned(),
                );
            }
        }
    }
    // Publication is idempotent. Clear any earlier complete row and partial
    // children before writing, so a retry after a process or network failure
    // can finish without duplicating already-landed rows.
    clear_episode_rows(connection, settings, &episode.start.id)?;
    ingest_batch(
        connection,
        &table_name(&settings.dataset, EPISODE_STREAMS_TABLE),
        episode_streams_batch(&episode.start).map_err(|error| error.to_string())?,
    )?;
    if !videos.is_empty() {
        let streams = videos
            .iter()
            .map(|video| VideoStreamRow {
                episode_id: episode.start.id.clone(),
                stream_id: video.stream_id.clone(),
                camera_id: video.camera_id.clone(),
                width: video.width,
                height: video.height,
                fps: video.fps,
            })
            .collect::<Vec<_>>();
        ingest_batch(
            connection,
            &table_name(&settings.dataset, EPISODE_STREAMS_TABLE),
            video_streams_batch(&streams).map_err(|error| error.to_string())?,
        )?;
    }
    let mut encoder =
        SpiralEpisodeEncoder::new(episode.start.clone()).map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    for sample in &episode.samples {
        rows.extend(encoder.encode(sample).map_err(|error| error.to_string())?);
        if rows.len() >= JOINT_BATCH_ROWS {
            let batch = joint_samples_batch(&rows).map_err(|error| error.to_string())?;
            ingest_batch(
                connection,
                &table_name(&settings.dataset, JOINT_SAMPLES_TABLE),
                batch,
            )?;
            rows.clear();
        }
    }
    if !rows.is_empty() {
        let batch = joint_samples_batch(&rows).map_err(|error| error.to_string())?;
        ingest_batch(
            connection,
            &table_name(&settings.dataset, JOINT_SAMPLES_TABLE),
            batch,
        )?;
    }
    if !videos.is_empty() {
        let batch = video_assets_batch(&episode.start.id, videos)?;
        ingest_batch(
            connection,
            &table_name(&settings.dataset, VIDEO_ASSETS_TABLE),
            batch,
        )?;
        // The view turns durable binary MP4 payloads into Spiral Blob values;
        // the managed dataset then persists the MP4/video index used by
        // `spql.video.*` frame selection and decode.
        let sources = format!("{}.video_sources", settings.dataset);
        let assets = format!("{}.{}", settings.dataset, VIDEO_ASSETS_TABLE);
        let dataset = format!("{}.video", settings.dataset);
        execute_update_sql(
            connection,
            &format!(
                "CREATE OR REPLACE VIEW {sources} AS SELECT episode_id, stream_id, \
             spql.blob.inline(payload, byte_size, 'video/mp4') AS source FROM {assets}"
            ),
        )
        .map_err(|error| format!("creating Spiral video source view: {error}"))?;
        // The offline ADBC catalog supports tables, views, and all ad-hoc MP4
        // functions, but managed Dataset resources require a Spiral project
        // catalog. Hosted connections get the durable reusable video index;
        // memory-mode tests exercise the same MP4 plugin directly below.
        if settings.uri != "spiral:memory:" {
            if !table_exists(connection, &format!("{}.video.assets", settings.dataset))? {
                let created = execute_update_sql(
                    connection,
                    &format!(
                        "CREATE DATASET {dataset} USING spql.mp4.dataset FROM {sources} \
                     WITH (blob = 'source', index_tier = 'dependency')"
                    ),
                );
                // A previous process may have committed the Dataset definition
                // but stopped before its first refresh produced `assets`.
                if let Err(error) = created
                    && !error.contains("already exists")
                {
                    return Err(format!("creating Spiral MP4 dataset: {error}"));
                }
            }
            execute_update_sql(connection, &format!("REFRESH DATASET {dataset}"))
                .map_err(|error| format!("refreshing Spiral MP4 dataset: {error}"))?;
        }
    }
    // Inserted last: readers never see an episode whose streams or media only
    // partially arrived.
    ingest_batch(
        connection,
        &table_name(&settings.dataset, EPISODES_TABLE),
        episode_completion_batch(&episode.start, &episode.completion, encoder.frames())
            .map_err(|error| error.to_string())?,
    )?;
    Ok(())
}

fn clear_episode_rows(
    connection: &mut ManagedConnection,
    settings: &StoredSpiralSettings,
    episode_id: &str,
) -> Result<(), String> {
    if settings.uri == "spiral:memory:" {
        return Ok(());
    }
    for table in [
        EPISODES_TABLE,
        JOINT_SAMPLES_TABLE,
        EPISODE_STREAMS_TABLE,
        VIDEO_ASSETS_TABLE,
    ] {
        let target = table_name(&settings.dataset, table);
        if !table_exists(connection, &target)? {
            continue;
        }
        execute_update_sql(
            connection,
            &format!("DELETE FROM {target} WHERE episode_id = '{episode_id}'"),
        )
        .map_err(|error| format!("clearing prior Spiral rows in {target}: {error}"))?;
    }
    Ok(())
}

fn table_has_episode(
    connection: &mut ManagedConnection,
    target: &str,
    episode_id: &str,
) -> Result<bool, String> {
    if !table_exists(connection, target)? {
        return Ok(false);
    }
    let result = run_query(
        connection,
        &format!("SELECT episode_id FROM {target} WHERE episode_id = '{episode_id}' LIMIT 1"),
        1,
    )?;
    Ok(!result.rows.is_empty())
}

fn ingest_batch(
    connection: &mut ManagedConnection,
    target: &str,
    batch: RecordBatch,
) -> Result<(), String> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening ADBC ingest for {target}: {error}"))?;
    statement
        .set_option(OptionStatement::TargetTable, target.into())
        .map_err(|error| format!("configuring ADBC ingest target {target}: {error}"))?;
    statement
        .set_option(OptionStatement::IngestMode, IngestMode::CreateAppend.into())
        .map_err(|error| format!("configuring ADBC ingest mode for {target}: {error}"))?;
    statement
        .bind(batch)
        .map_err(|error| format!("binding Spiral table {target}: {error}"))?;
    statement
        .execute_update()
        .map_err(|error| format!("ingesting Spiral table {target}: {error}"))?;
    Ok(())
}

fn table_exists(connection: &ManagedConnection, target: &str) -> Result<bool, String> {
    match connection.get_table_schema(None, None, target) {
        Ok(_) => Ok(true),
        Err(error) if error.status == AdbcStatus::NotFound => Ok(false),
        Err(error) => Err(format!("checking Spiral table {target}: {error}")),
    }
}

fn execute_update_sql(connection: &mut ManagedConnection, sql: &str) -> Result<(), String> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening Spiral SQL statement: {error}"))?;
    statement
        .set_sql_query(sql)
        .and_then(|()| statement.execute_update())
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn table_name(dataset: &str, table: &str) -> String {
    format!("{dataset}.{table}")
}

fn video_assets_batch(episode_id: &str, videos: &[VideoAsset]) -> Result<RecordBatch, String> {
    let payloads = videos
        .iter()
        .map(|video| {
            std::fs::read(&video.path)
                .map_err(|error| format!("could not read {}: {error}", video.path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let schema = Arc::new(Schema::new(vec![
        Field::new("episode_id", DataType::Utf8, false),
        Field::new("stream_id", DataType::Utf8, false),
        Field::new("camera_id", DataType::Utf8, false),
        Field::new("payload", DataType::Binary, false),
        Field::new("byte_size", DataType::UInt64, false),
        Field::new("width", DataType::UInt32, false),
        Field::new("height", DataType::UInt32, false),
        Field::new("fps", DataType::UInt32, false),
        Field::new("frames", DataType::UInt64, false),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(
            videos.iter().map(|_| episode_id),
        )),
        Arc::new(StringArray::from_iter_values(
            videos.iter().map(|video| video.stream_id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            videos.iter().map(|video| video.camera_id.as_str()),
        )),
        Arc::new(BinaryArray::from_iter_values(
            payloads.iter().map(Vec::as_slice),
        )),
        Arc::new(UInt64Array::from_iter_values(
            payloads.iter().map(|payload| payload.len() as u64),
        )),
        Arc::new(UInt32Array::from_iter_values(
            videos.iter().map(|video| video.width),
        )),
        Arc::new(UInt32Array::from_iter_values(
            videos.iter().map(|video| video.height),
        )),
        Arc::new(UInt32Array::from_iter_values(
            videos.iter().map(|video| video.fps),
        )),
        Arc::new(UInt64Array::from_iter_values(
            videos.iter().map(|video| video.frames),
        )),
    ];
    RecordBatch::try_new(schema, columns)
        .map_err(|error| format!("building Spiral video batch: {error}"))
}

fn run_query(
    connection: &mut ManagedConnection,
    sql: &str,
    max_rows: usize,
) -> Result<SpiralQueryResponse, String> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err("Spiral query cannot be empty".to_owned());
    }
    if sql.len() > 64 * 1024 {
        return Err("Spiral query is too large".to_owned());
    }
    let started = Instant::now();
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening Spiral ADBC query: {error}"))?;
    statement
        .set_sql_query(sql)
        .map_err(|error| format!("binding Spiral query: {error}"))?;
    let mut reader = statement
        .execute()
        .map_err(|error| format!("executing Spiral query: {error}"))?;
    let columns = reader
        .schema()
        .fields()
        .iter()
        .map(|field| QueryColumn {
            name: field.name().clone(),
            data_type: field.data_type().to_string(),
            nullable: field.is_nullable(),
        })
        .collect();
    let fetch = max_rows.saturating_add(1);
    let mut batches = Vec::new();
    let mut collected = 0;
    for batch in &mut reader {
        let batch = batch.map_err(|error| format!("reading Spiral result: {error}"))?;
        let remaining = fetch.saturating_sub(collected);
        if remaining == 0 {
            break;
        }
        let rows = batch.num_rows().min(remaining);
        batches.push(batch.slice(0, rows));
        collected += rows;
        if collected >= fetch {
            break;
        }
    }
    let mut rows = batches_to_json(&batches)?;
    let truncated = rows.len() > max_rows;
    rows.truncate(max_rows);
    Ok(SpiralQueryResponse {
        columns,
        rows,
        truncated,
        elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
    })
}

fn run_binary_query(connection: &mut ManagedConnection, sql: &str) -> Result<Vec<u8>, String> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening Spiral ADBC video query: {error}"))?;
    statement
        .set_sql_query(sql)
        .map_err(|error| format!("binding Spiral video query: {error}"))?;
    let reader = statement
        .execute()
        .map_err(|error| format!("executing Spiral video query: {error}"))?;
    for batch in reader {
        let batch = batch.map_err(|error| format!("reading Spiral video: {error}"))?;
        if batch.num_rows() == 0 {
            continue;
        }
        let payload = batch
            .column_by_name("payload")
            .ok_or_else(|| "Spiral video query returned no payload column".to_owned())?;
        if payload.is_null(0) {
            return Err("Spiral video payload is null".to_owned());
        }
        if let Some(payload) = payload.as_any().downcast_ref::<BinaryArray>() {
            return Ok(payload.value(0).to_vec());
        }
        if let Some(payload) = payload.as_any().downcast_ref::<LargeBinaryArray>() {
            return Ok(payload.value(0).to_vec());
        }
        if let Some(payload) = payload.as_any().downcast_ref::<BinaryViewArray>() {
            return Ok(payload.value(0).to_vec());
        }
        return Err(format!(
            "Spiral video payload has unexpected type {}",
            payload.data_type()
        ));
    }
    Err("No video matched that episode and stream".to_owned())
}

fn ensure_read_only_sql(sql: &str) -> Result<(), String> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err("Spiral query cannot be empty".to_owned());
    }
    let (tokens, semicolons) = sql_tokens(sql)?;
    if semicolons > 1 {
        return Err("Spiral preview accepts one read-only statement at a time".to_owned());
    }
    let first = tokens.first().map(String::as_str).unwrap_or_default();
    if !matches!(first, "SELECT" | "WITH" | "EXPLAIN" | "SHOW" | "DESCRIBE") {
        return Err(
            "Spiral preview accepts SELECT, WITH, EXPLAIN, SHOW, or DESCRIBE queries only"
                .to_owned(),
        );
    }
    const MUTATING_KEYWORDS: &[&str] = &[
        "ALTER", "CALL", "COPY", "CREATE", "DELETE", "DROP", "EXECUTE", "GRANT", "INSERT", "MERGE",
        "REFRESH", "REPLACE", "REVOKE", "TRUNCATE", "UPDATE",
    ];
    if tokens
        .iter()
        .any(|token| MUTATING_KEYWORDS.contains(&token.as_str()))
    {
        return Err("Spiral preview does not allow data or schema changes".to_owned());
    }
    Ok(())
}

/// Extract unquoted SQL words while rejecting extra statements. This is not a
/// SQL parser; it is a conservative guard in front of the driver's real parser
/// so a `WITH ... DELETE` cannot pass merely because its first word is `WITH`.
fn sql_tokens(sql: &str) -> Result<(Vec<String>, usize), String> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        LineComment,
        BlockComment,
    }

    let bytes = sql.as_bytes();
    let mut state = State::Normal;
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut semicolons = 0;
    let mut statement_token_count = None;
    let mut index = 0;
    let flush = |token: &mut String, tokens: &mut Vec<String>| {
        if !token.is_empty() {
            tokens.push(std::mem::take(token).to_ascii_uppercase());
        }
    };
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        match state {
            State::Normal => match (byte, next) {
                (b'-', Some(b'-')) => {
                    flush(&mut token, &mut tokens);
                    state = State::LineComment;
                    index += 1;
                }
                (b'/', Some(b'*')) => {
                    flush(&mut token, &mut tokens);
                    state = State::BlockComment;
                    index += 1;
                }
                (b'\'', _) => {
                    flush(&mut token, &mut tokens);
                    state = State::SingleQuote;
                }
                (b'"', _) => {
                    flush(&mut token, &mut tokens);
                    state = State::DoubleQuote;
                }
                (b';', _) => {
                    flush(&mut token, &mut tokens);
                    semicolons += 1;
                    statement_token_count.get_or_insert(tokens.len());
                }
                _ if byte.is_ascii_alphanumeric() || byte == b'_' => {
                    token.push(char::from(byte));
                }
                _ => flush(&mut token, &mut tokens),
            },
            State::SingleQuote => {
                if byte == b'\'' {
                    if next == Some(b'\'') {
                        index += 1;
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::DoubleQuote => {
                if byte == b'"' {
                    if next == Some(b'"') {
                        index += 1;
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::LineComment => {
                if matches!(byte, b'\n' | b'\r') {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                if byte == b'*' && next == Some(b'/') {
                    state = State::Normal;
                    index += 1;
                }
            }
        }
        index += 1;
    }
    flush(&mut token, &mut tokens);
    if !matches!(state, State::Normal | State::LineComment) {
        return Err("Spiral query contains an unterminated quote or comment".to_owned());
    }
    if statement_token_count.is_some_and(|count| tokens.len() != count) {
        return Err("Spiral preview accepts one read-only statement at a time".to_owned());
    }
    Ok((tokens, semicolons))
}

fn batches_to_json(batches: &[RecordBatch]) -> Result<Vec<JsonMap<String, JsonValue>>, String> {
    let refs = batches.iter().collect::<Vec<_>>();
    let mut bytes = Vec::new();
    let mut writer = WriterBuilder::new()
        .with_explicit_nulls(true)
        .build::<_, JsonArray>(&mut bytes);
    writer
        .write_batches(&refs)
        .map_err(|error| format!("encoding Spiral rows: {error}"))?;
    writer
        .finish()
        .map_err(|error| format!("finishing Spiral rows: {error}"))?;
    drop(writer);
    let values: Vec<JsonValue> =
        serde_json::from_slice(&bytes).map_err(|error| format!("decoding Spiral rows: {error}"))?;
    values
        .into_iter()
        .map(|value| match value {
            JsonValue::Object(row) => Ok(row),
            _ => Err("Spiral returned a non-object row".to_owned()),
        })
        .collect()
}

fn validate_safe_value(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(format!(
            "{label} must contain only letters, numbers, '_', '-' or '.'"
        ));
    }
    Ok(())
}

fn validate_dataset_identifier(value: &str) -> Result<(), String> {
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(
            "Spiral dataset must start with a letter or '_' and contain only letters, numbers, or '_'"
                .to_owned(),
        );
    }
    Ok(())
}

fn safe_repo_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use rerobot_core::api::EpisodeArm;

    use super::*;

    #[test]
    fn query_preview_rejects_mutation_and_multiple_statements() {
        assert!(ensure_read_only_sql("SELECT * FROM rerobot.episodes;").is_ok());
        assert!(ensure_read_only_sql("WITH recent AS (SELECT 1) SELECT * FROM recent").is_ok());
        assert!(ensure_read_only_sql("SELECT ';' AS punctuation; -- okay").is_ok());
        assert!(ensure_read_only_sql("DELETE FROM rerobot.episodes").is_err());
        assert!(
            ensure_read_only_sql("WITH removed AS (DELETE FROM rerobot.episodes) SELECT 1")
                .is_err()
        );
        assert!(ensure_read_only_sql("SELECT 1; DROP TABLE episodes").is_err());
    }

    /// Loads the real Spiral ADBC shared library but keeps all database state
    /// in process memory. Run explicitly with `SPIRALDB_ADBC_DRIVER` and a
    /// valid MP4 in `REROBOT_TEST_MP4`.
    #[test]
    #[ignore = "requires a built Spiral ADBC driver and FFmpeg-generated test MP4"]
    fn spiral_adbc_memory_round_trips_episode_and_video() {
        let driver_path = std::env::var("SPIRALDB_ADBC_DRIVER").unwrap();
        let video_path = PathBuf::from(std::env::var("REROBOT_TEST_MP4").unwrap());
        let settings = StoredSpiralSettings {
            enabled: true,
            uri: "spiral:memory:".to_owned(),
            project_id: String::new(),
            dataset: "rerobot".to_owned(),
        };
        let mut adbc = open_adbc(AdbcConnectionKey::new(
            &settings,
            PathBuf::from(driver_path),
        ))
        .unwrap();
        let episode = test_episode();
        let video = VideoAsset {
            stream_id: "follower.camera.wrist".to_owned(),
            camera_id: "camera-1".to_owned(),
            path: video_path,
            width: 32,
            height: 24,
            fps: 10,
            frames: 2,
        };

        publish_spiral_blocking(&mut adbc.connection, &settings, &episode, &[video.clone()])
            .unwrap();
        // A completed retry is idempotent even though Spiral's disposable
        // memory catalog intentionally does not support SQL DELETE.
        publish_spiral_blocking(&mut adbc.connection, &settings, &episode, &[video]).unwrap();

        let episodes = run_query(
            &mut adbc.connection,
            "SELECT COUNT(*) AS rows FROM rerobot.episodes",
            10,
        )
        .unwrap();
        assert_eq!(episodes.rows[0]["rows"], serde_json::json!(1));
        let video_rows = run_query(
            &mut adbc.connection,
            "SELECT COUNT(*) AS rows FROM rerobot.video_assets",
            10,
        )
        .unwrap();
        assert_eq!(video_rows.rows[0]["rows"], serde_json::json!(1));
        let mp4_tracks = run_query(
            &mut adbc.connection,
            "SELECT v.episode_id, v.stream_id, tracks.* FROM rerobot.video_sources v \
             CROSS JOIN LATERAL spql.mp4.tracks(v.source) AS tracks LIMIT 20",
            10,
        )
        .unwrap();
        assert_eq!(mp4_tracks.rows.len(), 1);
        let payload = run_binary_query(
            &mut adbc.connection,
            "SELECT payload FROM rerobot.video_assets LIMIT 1",
        )
        .unwrap();
        assert!(!payload.is_empty());
    }

    #[test]
    fn hugging_face_payload_is_columnar_and_keeps_each_stream() {
        let episode = test_episode();

        let bytes = hugging_face_episode_parquet(&episode).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(bytes))
            .unwrap()
            .build()
            .unwrap();
        let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 2);
        assert!(batches[0].column_by_name("action_position").is_some());
    }

    fn test_episode() -> CompletedEpisode {
        let start = crate::episodes::EpisodeStart {
            id: "episode_000001".to_owned(),
            started_ms: 1,
            sample_hz: 30.0,
            source: "test".to_owned(),
            note: None,
            arms: vec![EpisodeArm {
                id: "follower".to_owned(),
                joints: vec!["joint".to_owned()],
            }],
            condition: Vec::new(),
        };
        let sample = crate::episodes::Sample {
            seconds: 0.0,
            normalized: vec![vec![Some(0.2)]],
            raw: vec![vec![2]],
            follow_error_degrees: None,
            velocity: None,
            effort: None,
            action: vec![Some(vec![Some(0.25)])],
        };
        CompletedEpisode {
            start,
            samples: vec![sample.clone(), sample],
            completion: crate::episodes::EpisodeCompletion {
                duration_seconds: 1.0 / 30.0,
                max_follow_error_degrees: None,
                discarded: false,
                condition: Vec::new(),
            },
            path: std::path::PathBuf::new(),
        }
    }
}
