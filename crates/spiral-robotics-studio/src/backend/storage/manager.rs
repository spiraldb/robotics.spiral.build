//! Publishing, query preview, and remote replay for backend-owned datasets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use adbc_core::error::Status as AdbcStatus;
use adbc_core::options::{
    AdbcVersion, IngestMode, ObjectDepth, OptionConnection, OptionDatabase, OptionStatement,
};
use adbc_core::{Connection as _, Database as _, Driver as _, Optionable as _, Statement as _};
use adbc_driver_manager::{ManagedConnection, ManagedDatabase, ManagedDriver};
use arrow_array::{
    Array as _, ArrayRef, BinaryArray, BinaryViewArray, Float64Array, Int64Array, LargeBinaryArray,
    LargeStringArray, RecordBatch, StringArray, StringViewArray, UInt32Array, UInt64Array,
};
use arrow_json::writer::{JsonArray, WriterBuilder};
use arrow_schema::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use spiral_robotics_core::api::{
    ConnectionTestResponse, PublicationStatus, QueryColumn, SpiralQueryResponse, TrainingSource,
};
use spiral_robotics_core::spiral::{self, Table, column as db};

use crate::backend::recording::episodes::{CompletedEpisode, EpisodeStore};
use crate::backend::recording::spiral::{
    SpiralEpisodeEncoder, VideoStreamRow, dataset_batch, episode_completion_batch,
    episode_streams_batch, joint_samples_batch, video_streams_batch,
};
use crate::backend::robotics::video::{VideoAsset, load_assets, save_assets};
use crate::backend::storage::settings::{DataSettingsStore, StoredSpiralSettings};
use crate::backend::storage::sql::{self, ReadOnlyMode};

const MAX_QUERY_ROWS: usize = 1_000;
const MAX_QUERY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_INTERNAL_QUERY_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
const QUERY_JSON_CHUNK_ROWS: usize = 64;
const MAX_REPLAY_ROWS: usize = 250_000;
const JOINT_BATCH_ROWS: usize = 512;
const SPIRAL_RECEIPT: &str = "spiral.json";
const HOSTED_ADBC_CONNECTIONS: usize = 4;

pub(crate) struct TrainingCheckpointWrite {
    pub run_id: String,
    pub run_name: String,
    pub policy: String,
    pub step: u64,
    pub created_ms: i64,
    pub validation_loss: Option<f64>,
    pub config_json: String,
    pub payload: Vec<u8>,
}

pub(crate) struct TrainingCheckpointArtifact {
    pub config_json: Vec<u8>,
    pub payload: Vec<u8>,
}

pub(crate) struct StorageManager {
    pub settings: Arc<DataSettingsStore>,
    episodes: Arc<EpisodeStore>,
    driver_path: Option<PathBuf>,
    /// The disposable memory catalog exists only inside one connection, so it
    /// deliberately remains serialized and cached for its whole lifetime.
    memory_adbc: Arc<Mutex<Option<CachedAdbc>>>,
    /// Hosted work checks a connection out only for the duration of one
    /// operation. The pool mutex is never held across network or query work.
    hosted_adbc: Arc<Mutex<Vec<CachedAdbc>>>,
    hosted_adbc_slots: Arc<tokio::sync::Semaphore>,
    publications: Mutex<BTreeMap<String, PublicationStatus>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SpiralPublicationReceipt {
    uri: String,
    project_id: String,
    dataset: String,
    updated_ms: i64,
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
    pub(crate) fn new(
        settings: Arc<DataSettingsStore>,
        episodes: Arc<EpisodeStore>,
        driver_path: Option<PathBuf>,
    ) -> Result<Self, String> {
        Ok(Self {
            settings,
            episodes,
            driver_path,
            memory_adbc: Arc::new(Mutex::new(None)),
            hosted_adbc: Arc::new(Mutex::new(Vec::new())),
            hosted_adbc_slots: Arc::new(tokio::sync::Semaphore::new(HOSTED_ADBC_CONNECTIONS)),
            publications: Mutex::new(BTreeMap::new()),
        })
    }

    pub(crate) fn statuses(&self) -> Vec<PublicationStatus> {
        let episodes = self.episodes.list();
        let mut statuses = lock(&self.publications).clone();
        statuses.retain(|episode_id, _| episodes.iter().any(|episode| &episode.id == episode_id));
        let destination = self.settings.get().spiral;
        for episode in episodes {
            let existing = statuses.remove(&episode.id);
            let active_or_failed = existing.as_ref().is_some_and(|status| {
                matches!(status.state.as_str(), "queued" | "publishing")
                    || status
                        .spiral
                        .as_deref()
                        .is_some_and(|value| value.starts_with("failed:"))
            });
            let status = if active_or_failed {
                existing.expect("active publication status must exist")
            } else if load_spiral_receipt(self.episodes.directory(), &episode.id)
                .ok()
                .flatten()
                .is_some_and(|receipt| receipt.matches(&destination))
            {
                PublicationStatus {
                    episode_id: episode.id,
                    state: "published".to_owned(),
                    spiral: Some("published".to_owned()),
                    updated_ms: 0,
                }
            } else {
                PublicationStatus {
                    episode_id: episode.id,
                    state: "local".to_owned(),
                    updated_ms: 0,
                    ..PublicationStatus::default()
                }
            };
            statuses.insert(status.episode_id.clone(), status);
        }
        statuses.into_values().collect()
    }

    /// Finish the backend-owned recording transaction without crossing the
    /// network boundary. Completed JSONL and camera assets form the local
    /// staging buffer until the operator explicitly pushes the episode.
    pub(crate) fn stage_local(self: &Arc<Self>, episode_id: String, videos: Vec<VideoAsset>) {
        if let Err(error) = save_assets(self.episodes.directory(), &episode_id, &videos) {
            tracing::warn!(episode = %episode_id, "could not persist local video manifest: {error}");
        }
        self.set_status(PublicationStatus {
            episode_id: episode_id.clone(),
            state: "local".to_owned(),
            updated_ms: unix_millis(),
            ..PublicationStatus::default()
        });
    }

    fn queue_publish(self: &Arc<Self>, episode_id: String, videos: Vec<VideoAsset>) {
        let status = PublicationStatus {
            episode_id: episode_id.clone(),
            state: "queued".to_owned(),
            spiral: Some("queued".to_owned()),
            updated_ms: unix_millis(),
        };
        self.set_status(status);
        let manager = self.clone();
        tokio::spawn(async move {
            manager.publish(episode_id, videos).await;
        });
    }

    pub(crate) fn sync_spiral_existing(self: &Arc<Self>, episode_id: String) -> Result<(), String> {
        let outcome = (|| {
            self.ensure_not_publishing(&episode_id)?;
            if !self.settings.get().spiral.configured() {
                return Err(
                    "Configure a Spiral catalog, project, and namespace in Settings before pushing"
                        .to_owned(),
                );
            }
            let videos = self.prepare_existing(&episode_id)?;
            self.queue_publish(episode_id.clone(), videos);
            Ok(())
        })();
        if let Err(error) = &outcome {
            self.set_spiral_failed(&episode_id, error);
        }
        outcome
    }

    fn ensure_not_publishing(&self, episode_id: &str) -> Result<(), String> {
        if lock(&self.publications)
            .get(episode_id)
            .is_some_and(|status| matches!(status.state.as_str(), "queued" | "publishing"))
        {
            return Err(format!("{episode_id} is already being published"));
        }
        Ok(())
    }

    fn prepare_existing(&self, episode_id: &str) -> Result<Vec<VideoAsset>, String> {
        self.episodes
            .completed(episode_id)
            .map_err(|error| error.to_string())?;
        load_assets(self.episodes.directory(), episode_id)
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

        if !settings.spiral.configured() {
            status.state = "failed".to_owned();
            status.spiral = Some("failed: SpiralDB is not configured".to_owned());
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

        status.spiral = Some("publishing".to_owned());
        status.updated_ms = unix_millis();
        self.set_status(status.clone());
        status.spiral = Some(
            match self
                .publish_spiral(&settings.spiral, &completed, &videos)
                .await
            {
                Ok(()) => {
                    let receipt = SpiralPublicationReceipt::new(&settings.spiral);
                    if let Err(error) =
                        save_spiral_receipt(self.episodes.directory(), &episode_id, &receipt)
                    {
                        tracing::warn!(episode = %episode_id, "could not persist SpiralDB sync receipt: {error}");
                    }
                    "published".to_owned()
                }
                Err(error) => format!("failed: {error}"),
            },
        );
        let failed = status
            .spiral
            .iter()
            .any(|value| value.starts_with("failed:"));
        status.state = if failed { "failed" } else { "published" }.to_owned();
        status.updated_ms = unix_millis();
        self.set_status(status);
    }

    pub(crate) async fn test_spiral(&self) -> ConnectionTestResponse {
        let settings = self.settings.get().spiral;
        if !settings.configured() {
            return ConnectionTestResponse {
                ok: false,
                message:
                    "Choose a Spiral catalog, project, and namespace before testing the connection"
                        .to_owned(),
            };
        }
        if settings.uri == "spiral:memory:" {
            return match self
                .run_adbc(settings, |connection| {
                    run_query(connection, "SELECT 1 AS connected", 1).map(|_| ())
                })
                .await
            {
                Ok(()) => ConnectionTestResponse {
                    ok: true,
                    message: "Temporary in-memory ADBC session is ready".to_owned(),
                },
                Err(error) => ConnectionTestResponse {
                    ok: false,
                    message: error,
                },
            };
        }
        let endpoint = spiral_server_endpoint();
        let project = settings.project_id.clone();
        let mut probe_settings = settings.clone();
        // Project listing is currently paginated incompatibly between the
        // local service and driver. An exact legacy metadata lookup uses the
        // targeted project route instead, performs no writes, and succeeds
        // even before this dataset has created its first table.
        probe_settings.uri = "spiraldb://?catalog=legacy".to_owned();
        let probe_project = project.clone();
        if let Err(error) = self
            .run_adbc(probe_settings, move |connection| {
                let reader = connection
                    .get_objects(
                        ObjectDepth::Tables,
                        Some("legacy"),
                        Some(&probe_project),
                        Some("__spiral_robotics_connection_probe_missing__"),
                        Some(vec!["TABLE"]),
                        None,
                    )
                    .map_err(|error| format!("checking Spiral project access: {error}"))?;
                for batch in reader {
                    batch.map_err(|error| format!("reading Spiral project metadata: {error}"))?;
                }
                Ok(())
            })
            .await
        {
            return ConnectionTestResponse {
                ok: false,
                message: error,
            };
        }

        // Authorization alone is not enough for the managed ADBC path: an
        // older/local project may exist in the Tables Service without a
        // readable project catalog. Pin the actual catalog without requiring
        // any Spiral Robotics table to exist so Test connection predicts Query and
        // publication instead of returning a false positive.
        if uses_managed_catalog(&settings) {
            let catalog_project = project.clone();
            let table_filter = sql::relation(&settings.dataset, Table::Episodes);
            if let Err(error) = self
                .run_adbc(settings, move |connection| {
                    let reader = connection
                        .get_objects(
                            ObjectDepth::Tables,
                            Some("spiraldb"),
                            Some(&catalog_project),
                            Some(&table_filter),
                            Some(vec!["TABLE"]),
                            None,
                        )
                        .map_err(|error| format!("pinning the managed project catalog: {error}"))?;
                    for batch in reader {
                        batch.map_err(|error| {
                            format!("reading the managed project catalog: {error}")
                        })?;
                    }
                    Ok(())
                })
                .await
            {
                return ConnectionTestResponse {
                    ok: false,
                    message: format!(
                        "Project access succeeded at {endpoint}, but its managed catalog is not ready: {error}"
                    ),
                };
            }
        }

        ConnectionTestResponse {
            ok: true,
            message: format!("ADBC connected to {endpoint} · project {project}"),
        }
    }

    pub(crate) async fn query(
        &self,
        sql: &str,
        max_rows: usize,
    ) -> Result<SpiralQueryResponse, String> {
        sql::ensure_read_only(sql, ReadOnlyMode::Preview)?;
        let settings = self.settings.get().spiral;
        if !settings.configured() {
            return Err(
                "Configure a Spiral catalog, project, and namespace in Settings before running Query."
                    .to_owned(),
            );
        }
        let sql = sql.to_owned();
        self.run_adbc(settings, move |connection| {
            run_query_with_byte_limit(
                connection,
                &sql,
                max_rows.clamp(1, MAX_QUERY_ROWS),
                MAX_QUERY_RESPONSE_BYTES,
            )
        })
        .await
    }

    /// Resolve and freeze the hosted Spiral source for one training run.
    ///
    /// The disposable memory catalog is intentionally rejected: it is scoped
    /// to one ADBC connection, so a dedicated streaming connection could never
    /// observe rows published through the Studio's existing connection.
    pub(crate) fn training_source(
        &self,
        dataset_id: &str,
    ) -> Result<(StoredSpiralSettings, TrainingSource), String> {
        let settings = self.settings.get().spiral;
        if !settings.configured() {
            return Err("Configure a Spiral project and namespace before training.".to_owned());
        }
        if settings.uri == "spiral:memory:" {
            return Err(
                "Training requires a hosted Spiral project; the in-memory catalog is isolated to one connection."
                    .to_owned(),
            );
        }
        let driver = self
            .driver_path
            .as_deref()
            .ok_or_else(|| "Configure the Spiral ADBC driver before training.".to_owned())?;
        validate_adbc_settings(&settings, driver)?;
        let checkpoint_table = sql::relation(&settings.dataset, Table::TrainingCheckpoints);
        Ok((
            settings.clone(),
            TrainingSource {
                project: settings.project_id,
                dataset: settings.dataset,
                dataset_id: dataset_id.to_owned(),
                checkpoint_table,
            },
        ))
    }

    /// Resolve the query and its Arrow contract before starting the trainer.
    /// Unknown relations and renamed fields therefore fail the start request,
    /// rather than becoming an asynchronous training-task failure.
    pub(crate) async fn validate_training_query(
        &self,
        settings: StoredSpiralSettings,
        query: &str,
        state_column: &str,
        action_column: &str,
    ) -> Result<(), String> {
        sql::ensure_read_only(query, ReadOnlyMode::Training)?;
        let query = query.to_owned();
        let state_column = state_column.to_owned();
        let action_column = action_column.to_owned();
        let dataset = settings.dataset.clone();
        self.run_adbc(settings, move |connection| {
            let mut statement = connection
                .new_statement()
                .map_err(|error| format!("opening the Spiral training preflight: {error}"))?;
            statement
                .set_sql_query(&query)
                .map_err(|error| format!("binding the Spiral training preflight: {error}"))?;
            let reader = statement.execute().map_err(|error| {
                format!(
                    "training query does not resolve in configured namespace {dataset:?}: {error}"
                )
            })?;
            validate_training_schema(reader.schema().as_ref(), &state_column, &action_column)
        })
        .await
    }

    /// Run one training session on its own ADBC connection.
    ///
    /// Training performs long-running frame and action reads. Keeping this
    /// connection separate prevents those reads from blocking Studio query
    /// previews or checkpoint publication on the cached control connection.
    pub(crate) async fn run_training_adbc<T, F>(
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
        tokio::task::spawn_blocking(move || {
            let key = AdbcConnectionKey::new(&settings, driver_path);
            let mut adbc = open_adbc(key)?;
            operation(&mut adbc.connection)
        })
        .await
        .map_err(|error| format!("Spiral training ADBC task failed: {error}"))?
    }

    /// Publish a trainer-produced artifact through the backend's trusted ADBC
    /// control path. Checkpoint writes remain separate from the training read
    /// connection, so publication cannot deadlock a streaming batch read.
    pub(crate) async fn publish_training_checkpoint(
        &self,
        settings: StoredSpiralSettings,
        target: String,
        checkpoint: TrainingCheckpointWrite,
    ) -> Result<(), String> {
        self.run_adbc(settings, move |connection| {
            let batch = training_checkpoint_batch(checkpoint)?;
            ingest_batch(connection, &target, batch)
                .map_err(|error| format!("publishing training checkpoint to {target}: {error}"))
        })
        .await
    }

    pub(crate) async fn training_checkpoint(
        &self,
        run_id: &str,
        step: u64,
    ) -> Result<TrainingCheckpointArtifact, String> {
        let settings = self.settings.get().spiral;
        if !settings.configured() {
            return Err("Configure the Spiral project containing this checkpoint.".to_owned());
        }
        let query = sql::training_checkpoint(&settings.dataset, run_id, step);
        self.run_adbc(settings, move |connection| {
            run_training_checkpoint_query(connection, &query)
        })
        .await
    }

    pub(crate) async fn trajectory(
        &self,
        episode: &str,
        stream: &str,
    ) -> Result<(f64, Vec<BTreeMap<String, f64>>), String> {
        let settings = self.settings.get().spiral;
        let sql = sql::trajectory(&settings.dataset, episode, stream);
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
            sample_hz = sample_hz.or_else(|| row.get(db::SAMPLE_HZ).and_then(JsonValue::as_f64));
            let names = row
                .get(db::JOINT_NAMES)
                .and_then(JsonValue::as_array)
                .ok_or_else(|| format!("Spiral trajectory is missing {}", db::JOINT_NAMES))?;
            let values = row
                .get(db::REPLAY_POSITION)
                .and_then(JsonValue::as_array)
                .ok_or_else(|| format!("Spiral trajectory is missing {}", db::REPLAY_POSITION))?;
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
    pub(crate) async fn video_asset(
        &self,
        episode: &str,
        stream: &str,
        segment: Option<u32>,
    ) -> Result<Vec<u8>, String> {
        let settings = self.settings.get().spiral;
        let sql = sql::video_asset(&settings.dataset, episode, stream, segment);
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
        if settings.uri == "spiral:memory:" {
            return self.run_memory_adbc(settings, driver_path, operation).await;
        }

        let permit = self
            .hosted_adbc_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "Spiral ADBC connection pool is shutting down".to_owned())?;
        let pool = self.hosted_adbc.clone();
        tokio::task::spawn_blocking(move || {
            let key = AdbcConnectionKey::new(&settings, driver_path);
            let mut adbc = {
                let mut available = lock(&pool);
                available.retain(|cached| cached.key == key);
                available.pop()
            }
            .map(Ok)
            .unwrap_or_else(|| open_adbc(key))?;
            let outcome = operation(&mut adbc.connection);
            lock(&pool).push(adbc);
            drop(permit);
            outcome
        })
        .await
        .map_err(|error| format!("Spiral ADBC task failed: {error}"))?
    }

    async fn run_memory_adbc<T, F>(
        &self,
        settings: StoredSpiralSettings,
        driver_path: PathBuf,
        operation: F,
    ) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut ManagedConnection) -> Result<T, String> + Send + 'static,
    {
        let adbc = self.memory_adbc.clone();
        tokio::task::spawn_blocking(move || {
            let key = AdbcConnectionKey::new(&settings, driver_path);
            let mut cached = lock(&adbc);
            if cached.as_ref().is_none_or(|cached| cached.key != key) {
                *cached = Some(open_adbc(key)?);
            }
            operation(
                &mut cached
                    .as_mut()
                    .expect("ADBC memory connection was just installed")
                    .connection,
            )
        })
        .await
        .map_err(|error| format!("Spiral memory ADBC task failed: {error}"))?
    }

    fn set_status(&self, status: PublicationStatus) {
        lock(&self.publications).insert(status.episode_id.clone(), status);
    }

    fn set_spiral_failed(&self, episode_id: &str, error: &str) {
        self.set_status(PublicationStatus {
            episode_id: episode_id.to_owned(),
            state: "failed".to_owned(),
            spiral: Some(format!("failed: {error}")),
            updated_ms: unix_millis(),
        });
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

impl SpiralPublicationReceipt {
    fn new(settings: &StoredSpiralSettings) -> Self {
        Self {
            uri: settings.uri.clone(),
            project_id: settings.project_id.clone(),
            dataset: settings.dataset.clone(),
            updated_ms: unix_millis(),
        }
    }

    fn matches(&self, settings: &StoredSpiralSettings) -> bool {
        self.uri == settings.uri
            && self.project_id == settings.project_id
            && self.dataset == settings.dataset
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
    // Give the driver the project at database construction as well as through
    // CurrentSchema. The hosted driver uses the URI project to initialize its
    // managed catalog before the first schema lookup; CurrentSchema remains
    // the standard ADBC namespace seen by statements and metadata calls.
    let database_uri = if key.uri == "spiraldb://" && !key.project_id.is_empty() {
        format!("spiraldb://?project={}", key.project_id)
    } else {
        key.uri.clone()
    };
    let database = driver
        .new_database_with_opts([(OptionDatabase::Uri, database_uri.clone().into())])
        .map_err(|error| format!("opening Spiral ADBC database {database_uri}: {error}"))?;
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
    if !matches!(
        settings.uri.as_str(),
        "spiraldb://" | "spiraldb://?catalog=legacy" | "spiral:memory:"
    ) {
        return Err("Spiral ADBC URI must be 'spiraldb://' or 'spiral:memory:'".to_owned());
    }
    if settings.uri != "spiral:memory:" {
        if settings.project_id.trim().is_empty() {
            return Err("Choose a Spiral project in Settings before connecting".to_owned());
        }
        validate_safe_value("Spiral project", &settings.project_id)?;
    }
    validate_dataset_identifier(&settings.dataset)
}

fn spiral_server_endpoint() -> String {
    std::env::var("SPIRAL__SERVER__URL")
        .or_else(|_| std::env::var("SPIRALDB_API_URL"))
        .unwrap_or_else(|_| "the configured hosted SpiralDB service".to_owned())
}

fn uses_managed_catalog(settings: &StoredSpiralSettings) -> bool {
    settings.uri == "spiraldb://"
}

fn publish_spiral_blocking(
    connection: &mut ManagedConnection,
    settings: &StoredSpiralSettings,
    episode: &CompletedEpisode,
    videos: &[VideoAsset],
) -> Result<(), String> {
    if uses_managed_catalog(settings) {
        ensure_dataset_namespace(connection, &settings.dataset)?;
    }
    if settings.uri == "spiral:memory:" {
        // The driver's offline catalog intentionally has no mutable relation
        // catalog, so SQL DELETE is unavailable there. A completed retry is
        // already successful; a partial retry must restart the disposable
        // memory connection instead of silently duplicating rows.
        if table_has_episode(
            connection,
            &sql::relation(&settings.dataset, Table::Episodes),
            &episode.start.id,
        )? {
            return Ok(());
        }
        for table in [
            Table::JointSamples,
            Table::EpisodeStreams,
            Table::VideoAssets,
        ] {
            if table_has_episode(
                connection,
                &sql::relation(&settings.dataset, table),
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
    let datasets = sql::relation(&settings.dataset, Table::Datasets);
    if !table_has_dataset(connection, &datasets, &episode.start.dataset_id)? {
        ingest_batch(
            connection,
            &datasets,
            dataset_batch(&episode.start).map_err(|error| error.to_string())?,
        )?;
    }
    ingest_batch(
        connection,
        &sql::relation(&settings.dataset, Table::EpisodeStreams),
        episode_streams_batch(&episode.start).map_err(|error| error.to_string())?,
    )?;
    if !videos.is_empty() {
        let streams = videos
            .iter()
            .map(|video| {
                (
                    video.stream_id.clone(),
                    VideoStreamRow {
                        episode_id: episode.start.id.clone(),
                        stream_id: video.stream_id.clone(),
                        camera_id: video.camera_id.clone(),
                        width: video.width,
                        height: video.height,
                        fps: video.fps,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect::<Vec<_>>();
        ingest_batch(
            connection,
            &sql::relation(&settings.dataset, Table::EpisodeStreams),
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
                &sql::relation(&settings.dataset, Table::JointSamples),
                batch,
            )?;
            rows.clear();
        }
    }
    if !rows.is_empty() {
        let batch = joint_samples_batch(&rows).map_err(|error| error.to_string())?;
        ingest_batch(
            connection,
            &sql::relation(&settings.dataset, Table::JointSamples),
            batch,
        )?;
    }
    if !videos.is_empty() {
        let media_relation = sql::video_dataset_output(&settings.dataset, &episode.start.id);
        let batch = video_assets_batch(&episode.start.id, videos, &media_relation)?;
        ingest_batch(
            connection,
            &sql::relation(&settings.dataset, Table::VideoAssets),
            batch,
        )?;
        // The view turns durable binary MP4 payloads into Spiral Blob values;
        // the managed dataset then persists the MP4/video index used by
        // `spql.video.*` frame selection and decode.
        let managed = uses_managed_catalog(settings);
        let sources = sql::video_sources_relation(&settings.dataset, &episode.start.id, managed);
        let assets = sql::relation(&settings.dataset, Table::VideoAssets);
        let dataset = sql::video_dataset_relation(&settings.dataset, &episode.start.id);
        execute_update_sql(
            connection,
            &sql::create_video_sources(
                &sources,
                &assets,
                managed.then_some(episode.start.id.as_str()),
            ),
        )
        .map_err(|error| format!("creating Spiral video source view: {error}"))?;
        // The offline ADBC catalog supports tables, views, and all ad-hoc MP4
        // functions, but managed Dataset resources require a Spiral project
        // catalog. Service connections get a durable index per immutable
        // episode; memory-mode tests exercise the same MP4 plugin directly.
        if uses_managed_catalog(settings) {
            // Completed episodes are immutable, so each has one immutable
            // managed MP4 Dataset. This also avoids flattening the native
            // VideoIndex extension through ADBC metadata or relying on output
            // replacement, which REFRESH does not yet support.
            let created =
                execute_update_sql(connection, &sql::create_video_dataset(&dataset, &sources));
            let needs_refresh = match created {
                Ok(()) => true,
                Err(error) if error.contains("already exists") => {
                    let output = sql::video_dataset_output(&settings.dataset, &episode.start.id);
                    match run_query(connection, &sql::count_rows(&output), 1) {
                        Ok(_) => false,
                        Err(error) if error.contains("unknown table") => true,
                        Err(error) => {
                            return Err(format!(
                                "checking Spiral MP4 dataset output {output}: {error}"
                            ));
                        }
                    }
                }
                Err(error) => return Err(format!("creating Spiral MP4 dataset: {error}")),
            };
            if needs_refresh {
                execute_update_sql(connection, &sql::refresh_dataset(&dataset))
                    .map_err(|error| format!("refreshing Spiral MP4 dataset: {error}"))?;
            }
        }
    }
    // Inserted last: readers never see an episode whose streams or media only
    // partially arrived.
    ingest_batch(
        connection,
        &sql::relation(&settings.dataset, Table::Episodes),
        episode_completion_batch(&episode.start, &episode.completion, encoder.frames())
            .map_err(|error| error.to_string())?,
    )?;
    Ok(())
}

/// Spiral's managed catalog is a resource tree: `spiral_robotics.episodes` can only
/// be created after a live `spiral_robotics` parent exists. Until the catalog exposes
/// a standalone namespace resource, a tiny durable root table is the parent
/// container for every Spiral Robotics table, view, and managed Dataset.
fn ensure_dataset_namespace(
    connection: &mut ManagedConnection,
    dataset: &str,
) -> Result<(), String> {
    match connection.get_table_schema(None, None, dataset) {
        Ok(schema)
            if schema
                .field_with_name(spiral::DATASET_NAMESPACE_MARKER)
                .is_ok() =>
        {
            return Ok(());
        }
        Ok(_) => {
            return Err(format!(
                "SpiralDB namespace {dataset} exists but is not managed by Spiral Robotics; choose a new namespace or remove that resource"
            ));
        }
        Err(error) if error.status == AdbcStatus::NotFound => {}
        Err(error) => {
            return Err(format!("checking SpiralDB namespace {dataset}: {error}"));
        }
    }
    execute_update_sql(connection, &sql::create_dataset_namespace(dataset))
        .map_err(|error| format!("creating SpiralDB namespace {dataset}: {error}"))
}

fn clear_episode_rows(
    connection: &mut ManagedConnection,
    settings: &StoredSpiralSettings,
    episode_id: &str,
) -> Result<(), String> {
    if settings.uri == "spiral:memory:" {
        return Ok(());
    }
    for table in Table::EPISODE_RELATIONS {
        let target = sql::relation(&settings.dataset, table);
        if !table_exists(connection, &target)? {
            continue;
        }
        let cleared = execute_update_sql(connection, &sql::delete_episode(&target, episode_id));
        if let Err(error) = cleared
            && !error.contains("finished without any affected rows")
        {
            return Err(format!("clearing prior Spiral rows in {target}: {error}"));
        }
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
    let result = run_query(connection, &sql::episode_exists(target, episode_id), 1)?;
    Ok(!result.rows.is_empty())
}

fn table_has_dataset(
    connection: &mut ManagedConnection,
    target: &str,
    dataset_id: &str,
) -> Result<bool, String> {
    if !table_exists(connection, target)? {
        return Ok(false);
    }
    let result = run_query(connection, &sql::dataset_exists(target, dataset_id), 1)?;
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

fn validate_training_schema(
    schema: &Schema,
    state_column: &str,
    action_column: &str,
) -> Result<(), String> {
    let missing = [
        db::EPISODE_ID,
        db::STREAM_ID,
        db::SAMPLE_INDEX,
        db::EPISODE_TIME_NS,
        db::SAMPLE_HZ,
        db::EPISODE_FRAMES,
        db::ROBOT_TYPE,
        db::JOINT_NAMES,
        state_column,
        action_column,
    ]
    .into_iter()
    .filter(|name| schema.field_with_name(name).is_err())
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        Err(format!(
            "the Spiral training query is missing required columns: {}",
            missing.join(", ")
        ))?;
    }

    let expect = |name: &str, accepted: &dyn Fn(&DataType) -> bool, label: &str| {
        let field = schema
            .field_with_name(name)
            .expect("required training field was checked above");
        if accepted(field.data_type()) {
            Ok(())
        } else {
            Err(format!(
                "training column {name:?} must be {label}, got {}",
                field.data_type()
            ))
        }
    };
    // Spiral preserves its zero-copy string representation through ADBC as
    // Arrow Utf8View. It has the same logical UTF-8 contract as the offset-
    // based Utf8 and LargeUtf8 arrays, so schema validation must accept all
    // three physical representations.
    let utf8 = |kind: &DataType| {
        matches!(
            kind,
            DataType::Utf8 | DataType::Utf8View | DataType::LargeUtf8
        )
    };
    let integer = |kind: &DataType| {
        matches!(
            kind,
            DataType::Int8
                | DataType::Int16
                | DataType::Int32
                | DataType::Int64
                | DataType::UInt8
                | DataType::UInt16
                | DataType::UInt32
                | DataType::UInt64
        )
    };
    let floating = |kind: &DataType| {
        matches!(
            kind,
            DataType::Float16 | DataType::Float32 | DataType::Float64
        )
    };
    let numeric_vector = |kind: &DataType| match kind {
        DataType::List(item) | DataType::LargeList(item) | DataType::FixedSizeList(item, _) => {
            matches!(
                item.data_type(),
                DataType::Float16 | DataType::Float32 | DataType::Float64
            )
        }
        _ => false,
    };
    let string_list = |kind: &DataType| match kind {
        DataType::List(item) | DataType::LargeList(item) | DataType::FixedSizeList(item, _) => {
            utf8(item.data_type())
        }
        _ => false,
    };

    expect(db::EPISODE_ID, &utf8, "UTF-8 text")?;
    expect(db::STREAM_ID, &utf8, "UTF-8 text")?;
    expect(db::SAMPLE_INDEX, &integer, "an integer")?;
    expect(db::EPISODE_TIME_NS, &integer, "an integer")?;
    expect(db::SAMPLE_HZ, &floating, "a floating-point rate")?;
    expect(db::EPISODE_FRAMES, &integer, "an integer")?;
    expect(db::ROBOT_TYPE, &utf8, "UTF-8 text")?;
    expect(db::JOINT_NAMES, &string_list, "a list of UTF-8 joint names")?;
    expect(state_column, &numeric_vector, "a floating-point vector")?;
    expect(action_column, &numeric_vector, "a floating-point vector")?;
    Ok(())
}

fn training_checkpoint_batch(checkpoint: TrainingCheckpointWrite) -> Result<RecordBatch, String> {
    let schema = training_checkpoint_schema();
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![checkpoint.run_id])),
        Arc::new(StringArray::from(vec![checkpoint.run_name])),
        Arc::new(StringArray::from(vec![checkpoint.policy])),
        Arc::new(UInt64Array::from(vec![checkpoint.step])),
        Arc::new(Int64Array::from(vec![checkpoint.created_ms])),
        Arc::new(Float64Array::from(vec![checkpoint.validation_loss])),
        Arc::new(StringArray::from(vec![checkpoint.config_json])),
        Arc::new(LargeBinaryArray::from_iter_values([checkpoint
            .payload
            .as_slice()])),
    ];
    RecordBatch::try_new(schema, columns)
        .map_err(|error| format!("building the training checkpoint batch: {error}"))
}

fn training_checkpoint_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(db::RUN_ID, DataType::Utf8, false),
        Field::new(db::RUN_NAME, DataType::Utf8, false),
        Field::new(db::POLICY, DataType::Utf8, false),
        Field::new(db::STEP, DataType::UInt64, false),
        Field::new(db::CREATED_MS, DataType::Int64, false),
        Field::new(db::VALIDATION_LOSS, DataType::Float64, true),
        Field::new(db::CONFIG_JSON, DataType::Utf8, false),
        Field::new(db::CHECKPOINT, DataType::LargeBinary, false),
    ]))
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

fn video_assets_batch(
    episode_id: &str,
    videos: &[VideoAsset],
    media_relation: &str,
) -> Result<RecordBatch, String> {
    let payloads = videos
        .iter()
        .map(|video| {
            std::fs::read(&video.path)
                .map_err(|error| format!("could not read {}: {error}", video.path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let timeline_offsets = videos
        .iter()
        .map(|video| {
            video
                .timeline_offset_seconds
                .map(|seconds| {
                    if !seconds.is_finite() || seconds < 0.0 {
                        return Err(format!(
                            "video timeline offset for {} must be finite and non-negative",
                            video.stream_id
                        ));
                    }
                    let nanos = seconds * 1_000_000_000.0;
                    if nanos > u64::MAX as f64 {
                        return Err(format!(
                            "video timeline offset for {} is too large",
                            video.stream_id
                        ));
                    }
                    Ok(nanos.round() as u64)
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let schema = video_assets_schema();
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(
            videos.iter().map(|_| episode_id),
        )),
        Arc::new(StringArray::from_iter_values(
            videos.iter().map(|video| video.stream_id.as_str()),
        )),
        Arc::new(UInt32Array::from_iter_values(
            videos.iter().map(|video| video.segment_index),
        )),
        Arc::new(UInt64Array::from(timeline_offsets)),
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
        Arc::new(StringArray::from_iter_values(
            videos.iter().map(|_| media_relation),
        )),
    ];
    RecordBatch::try_new(schema, columns)
        .map_err(|error| format!("building Spiral video batch: {error}"))
}

fn video_assets_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(db::EPISODE_ID, DataType::Utf8, false),
        Field::new(db::STREAM_ID, DataType::Utf8, false),
        Field::new(db::SEGMENT_INDEX, DataType::UInt32, false),
        Field::new(db::TIMELINE_OFFSET_NS, DataType::UInt64, true),
        Field::new(db::CAMERA_ID, DataType::Utf8, false),
        Field::new(db::PAYLOAD, DataType::Binary, false),
        Field::new(db::BYTE_SIZE, DataType::UInt64, false),
        Field::new(db::WIDTH, DataType::UInt32, false),
        Field::new(db::HEIGHT, DataType::UInt32, false),
        Field::new(db::FPS, DataType::UInt32, false),
        Field::new(db::FRAMES, DataType::UInt64, false),
        Field::new(db::MEDIA_RELATION, DataType::Utf8, false),
    ]))
}

fn run_query(
    connection: &mut ManagedConnection,
    sql: &str,
    max_rows: usize,
) -> Result<SpiralQueryResponse, String> {
    run_query_with_byte_limit(connection, sql, max_rows, MAX_INTERNAL_QUERY_RESPONSE_BYTES)
}

fn run_query_with_byte_limit(
    connection: &mut ManagedConnection,
    sql: &str,
    max_rows: usize,
    max_bytes: usize,
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
    let mut rows = Vec::with_capacity(max_rows.min(QUERY_JSON_CHUNK_ROWS));
    let mut encoded_bytes = 2_usize;
    let mut inspected = 0_usize;
    let mut truncated = false;
    'result: for batch in &mut reader {
        let batch = batch.map_err(|error| format!("reading Spiral result: {error}"))?;
        let mut offset = 0_usize;
        while offset < batch.num_rows() && inspected < fetch {
            let count = QUERY_JSON_CHUNK_ROWS
                .min(batch.num_rows() - offset)
                .min(fetch - inspected);
            let chunk = batch.slice(offset, count);
            for row in batch_to_json(&chunk)? {
                inspected += 1;
                if rows.len() >= max_rows {
                    truncated = true;
                    break 'result;
                }
                if !push_query_row(&mut rows, &mut encoded_bytes, row, max_bytes)? {
                    truncated = true;
                    break 'result;
                }
            }
            offset += count;
        }
    }
    Ok(SpiralQueryResponse {
        columns,
        rows,
        truncated,
        elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
    })
}

fn push_query_row(
    rows: &mut Vec<JsonMap<String, JsonValue>>,
    encoded_bytes: &mut usize,
    row: JsonMap<String, JsonValue>,
    max_bytes: usize,
) -> Result<bool, String> {
    let row_bytes = serde_json::to_vec(&row)
        .map_err(|error| format!("measuring Spiral row: {error}"))?
        .len()
        .saturating_add(usize::from(!rows.is_empty()));
    if encoded_bytes.saturating_add(row_bytes) > max_bytes {
        return Ok(false);
    }
    *encoded_bytes = encoded_bytes.saturating_add(row_bytes);
    rows.push(row);
    Ok(true)
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
            .column_by_name(db::PAYLOAD)
            .ok_or_else(|| format!("Spiral video query returned no {} column", db::PAYLOAD))?;
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

fn run_training_checkpoint_query(
    connection: &mut ManagedConnection,
    sql: &str,
) -> Result<TrainingCheckpointArtifact, String> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening Spiral checkpoint query: {error}"))?;
    statement
        .set_sql_query(sql)
        .map_err(|error| format!("binding Spiral checkpoint query: {error}"))?;
    let reader = statement
        .execute()
        .map_err(|error| format!("executing Spiral checkpoint query: {error}"))?;
    let mut artifact = None;
    for batch in reader {
        let batch = batch.map_err(|error| format!("reading Spiral checkpoint: {error}"))?;
        for row in 0..batch.num_rows() {
            if artifact.is_some() {
                return Err("Spiral returned duplicate rows for that checkpoint".to_owned());
            }
            let config = batch.column_by_name(db::CONFIG_JSON).ok_or_else(|| {
                format!(
                    "Spiral checkpoint query returned no UTF-8 {} column",
                    db::CONFIG_JSON
                )
            })?;
            let checkpoint = batch.column_by_name(db::CHECKPOINT).ok_or_else(|| {
                format!(
                    "Spiral checkpoint query returned no {} column",
                    db::CHECKPOINT
                )
            })?;
            let payload = if let Some(values) = checkpoint.as_any().downcast_ref::<BinaryArray>() {
                values.value(row).to_vec()
            } else if let Some(values) = checkpoint.as_any().downcast_ref::<LargeBinaryArray>() {
                values.value(row).to_vec()
            } else if let Some(values) = checkpoint.as_any().downcast_ref::<BinaryViewArray>() {
                values.value(row).to_vec()
            } else {
                return Err(format!(
                    "Spiral checkpoint payload has unexpected type {}",
                    checkpoint.data_type()
                ));
            };
            let config_json = if let Some(values) = config.as_any().downcast_ref::<StringArray>() {
                values.value(row).as_bytes().to_vec()
            } else if let Some(values) = config.as_any().downcast_ref::<LargeStringArray>() {
                values.value(row).as_bytes().to_vec()
            } else if let Some(values) = config.as_any().downcast_ref::<StringViewArray>() {
                values.value(row).as_bytes().to_vec()
            } else {
                return Err(format!(
                    "Spiral checkpoint config has unexpected type {}",
                    config.data_type()
                ));
            };
            artifact = Some(TrainingCheckpointArtifact {
                config_json,
                payload,
            });
        }
    }
    artifact.ok_or_else(|| "No checkpoint matched that training run and step".to_owned())
}

fn batch_to_json(batch: &RecordBatch) -> Result<Vec<JsonMap<String, JsonValue>>, String> {
    let mut bytes = Vec::new();
    let mut writer = WriterBuilder::new()
        .with_explicit_nulls(true)
        .build::<_, JsonArray>(&mut bytes);
    writer
        .write_batches(&[batch])
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
    if !spiral::is_identifier(value) {
        return Err(
            "Spiral namespace must start with a letter or '_' and contain only letters, numbers, or '_'"
                .to_owned(),
        );
    }
    Ok(())
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn save_spiral_receipt(
    root: &Path,
    episode_id: &str,
    receipt: &SpiralPublicationReceipt,
) -> Result<(), String> {
    let directory = root.join("videos").join(episode_id);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    let path = directory.join(SPIRAL_RECEIPT);
    let temporary = directory.join("spiral.json.tmp");
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("could not encode SpiralDB sync receipt: {error}"))?;
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("could not publish {}: {error}", path.display()))
}

fn load_spiral_receipt(
    root: &Path,
    episode_id: &str,
) -> Result<Option<SpiralPublicationReceipt>, String> {
    let path = root.join("videos").join(episode_id).join(SPIRAL_RECEIPT);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use spiral_robotics_core::api::EpisodeArm;

    use super::*;

    #[test]
    fn backend_owned_arrow_schemas_match_the_shared_contract() {
        for (table, schema) in [
            (Table::VideoAssets, video_assets_schema()),
            (Table::TrainingCheckpoints, training_checkpoint_schema()),
        ] {
            let actual = schema
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, table.columns(), "{table}");
        }
    }

    #[test]
    fn query_preview_rejects_mutation_and_multiple_statements() {
        let read_only = |query| sql::ensure_read_only(query, ReadOnlyMode::Preview);
        assert!(read_only("SELECT * FROM spiral_robotics.episodes;").is_ok());
        assert!(read_only("WITH recent AS (SELECT 1) SELECT * FROM recent").is_ok());
        assert!(read_only("SELECT ';' AS punctuation; -- okay").is_ok());
        assert!(read_only("DELETE FROM spiral_robotics.episodes").is_err());
        assert!(read_only("SHOW TABLES").is_err());
        assert!(
            read_only("WITH removed AS (DELETE FROM spiral_robotics.episodes) SELECT 1").is_err()
        );
        assert!(read_only("SELECT 1; DROP TABLE episodes").is_err());
    }

    #[test]
    fn query_preview_byte_budget_stops_before_an_oversized_row() {
        let mut rows = Vec::new();
        let mut bytes = 2;
        let small = JsonMap::from_iter([("value".to_owned(), JsonValue::from("ok"))]);
        assert!(push_query_row(&mut rows, &mut bytes, small, 64).unwrap());
        let huge = JsonMap::from_iter([("value".to_owned(), JsonValue::from("x".repeat(128)))]);
        assert!(!push_query_row(&mut rows, &mut bytes, huge, 64).unwrap());
        assert_eq!(rows.len(), 1);
        assert!(bytes <= 64);
    }

    #[test]
    fn hosted_adbc_work_has_a_small_bounded_connection_budget() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Arc::new(DataSettingsStore::open(directory.path()).unwrap());
        let episodes = Arc::new(EpisodeStore::open(directory.path().to_path_buf()).unwrap());
        let manager = StorageManager::new(settings, episodes, None).unwrap();
        assert_eq!(
            manager.hosted_adbc_slots.available_permits(),
            HOSTED_ADBC_CONNECTIONS
        );
        assert!(manager.hosted_adbc.lock().unwrap().is_empty());
    }

    #[test]
    fn training_schema_drift_is_rejected_before_training() {
        let schema = Schema::new(vec![
            Field::new(db::EPISODE_ID, DataType::Utf8, false),
            Field::new(db::STREAM_ID, DataType::Utf8, false),
            Field::new(db::SAMPLE_INDEX, DataType::UInt64, false),
            Field::new(db::EPISODE_TIME_NS, DataType::UInt64, false),
            Field::new(db::STATE, DataType::Utf8, false),
        ]);
        let error = validate_training_schema(&schema, db::STATE, db::ACTION).unwrap_err();
        assert!(error.contains(db::ACTION));
    }

    #[test]
    fn training_schema_accepts_spiral_utf8_views() {
        let string_list = DataType::List(Arc::new(Field::new("item", DataType::Utf8View, false)));
        let vector =
            || DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float64, false)), 6);
        let schema = Schema::new(vec![
            Field::new(db::EPISODE_ID, DataType::Utf8View, false),
            Field::new(db::STREAM_ID, DataType::Utf8View, false),
            Field::new(db::SAMPLE_INDEX, DataType::UInt64, false),
            Field::new(db::EPISODE_TIME_NS, DataType::UInt64, false),
            Field::new(db::SAMPLE_HZ, DataType::Float64, false),
            Field::new(db::EPISODE_FRAMES, DataType::UInt64, false),
            Field::new(db::ROBOT_TYPE, DataType::Utf8View, false),
            Field::new(db::JOINT_NAMES, string_list, false),
            Field::new(db::STATE, vector(), false),
            Field::new(db::ACTION, vector(), false),
        ]);

        validate_training_schema(&schema, db::STATE, db::ACTION).unwrap();
    }

    #[test]
    fn completed_recordings_stage_locally_until_an_explicit_push() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Arc::new(DataSettingsStore::open(directory.path()).unwrap());
        let episodes = Arc::new(EpisodeStore::open(directory.path().to_path_buf()).unwrap());
        let manager = Arc::new(StorageManager::new(settings, episodes, None).unwrap());

        manager.stage_local("episode_000024".to_owned(), Vec::new());

        let publications = lock(&manager.publications);
        let status = publications.get("episode_000024").unwrap();
        assert_eq!(status.state, "local");
        assert_eq!(status.spiral, None);
        assert!(
            directory
                .path()
                .join("videos/episode_000024/assets.json")
                .is_file()
        );
    }

    #[test]
    fn spiral_sync_receipts_survive_restart_and_are_destination_specific() {
        let directory = tempfile::tempdir().unwrap();
        let destination = StoredSpiralSettings {
            enabled: true,
            uri: "spiraldb://".to_owned(),
            project_id: "project-a".to_owned(),
            dataset: "robot_data".to_owned(),
        };
        let receipt = SpiralPublicationReceipt::new(&destination);

        save_spiral_receipt(directory.path(), "episode_000024", &receipt).unwrap();
        let restored = load_spiral_receipt(directory.path(), "episode_000024")
            .unwrap()
            .unwrap();

        assert_eq!(restored, receipt);
        assert!(restored.matches(&destination));
        assert!(!restored.matches(&StoredSpiralSettings {
            dataset: "other_dataset".to_owned(),
            ..destination
        }));
    }

    #[test]
    fn spiral_push_preflight_is_retained_as_a_row_level_failure() {
        let directory = tempfile::tempdir().unwrap();
        let settings = Arc::new(DataSettingsStore::open(directory.path()).unwrap());
        let episodes = Arc::new(EpisodeStore::open(directory.path().to_path_buf()).unwrap());
        let manager = Arc::new(StorageManager::new(settings, episodes, None).unwrap());

        let error = manager
            .sync_spiral_existing("episode_000024".to_owned())
            .unwrap_err();

        assert!(error.contains("Configure a Spiral"));
        let publications = lock(&manager.publications);
        let status = publications.get("episode_000024").unwrap();
        assert_eq!(status.state, "failed");
        assert!(
            status
                .spiral
                .as_deref()
                .is_some_and(|message| message.contains("Configure a Spiral"))
        );
    }

    /// Loads the real Spiral ADBC shared library but keeps all database state
    /// in process memory. Run explicitly with `SPIRALDB_ADBC_DRIVER` and a
    /// valid MP4 in `SPIRAL_ROBOTICS_TEST_MP4`.
    #[test]
    #[ignore = "requires a built Spiral ADBC driver and FFmpeg-generated test MP4"]
    fn spiral_adbc_memory_round_trips_episode_and_video() {
        let driver_path = std::env::var("SPIRALDB_ADBC_DRIVER").unwrap();
        let video_path = PathBuf::from(std::env::var("SPIRAL_ROBOTICS_TEST_MP4").unwrap());
        let settings = StoredSpiralSettings {
            enabled: true,
            uri: "spiral:memory:".to_owned(),
            project_id: String::new(),
            dataset: "spiral_robotics".to_owned(),
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
            segment_index: 0,
            timeline_offset_seconds: Some(0.0),
        };

        publish_spiral_blocking(
            &mut adbc.connection,
            &settings,
            &episode,
            std::slice::from_ref(&video),
        )
        .unwrap();
        // A completed retry is idempotent even though Spiral's disposable
        // memory catalog intentionally does not support SQL DELETE.
        publish_spiral_blocking(&mut adbc.connection, &settings, &episode, &[video]).unwrap();

        let episodes = run_query(
            &mut adbc.connection,
            &sql::count_rows(&sql::relation("spiral_robotics", Table::Episodes)),
            10,
        )
        .unwrap();
        assert_eq!(episodes.rows[0]["rows"], serde_json::json!(1));
        let video_rows = run_query(
            &mut adbc.connection,
            &sql::count_rows(&sql::relation("spiral_robotics", Table::VideoAssets)),
            10,
        )
        .unwrap();
        assert_eq!(video_rows.rows[0]["rows"], serde_json::json!(1));
        let training_rows = run_query(
            &mut adbc.connection,
            &spiral::default_training_sql("spiral_robotics"),
            10,
        )
        .unwrap();
        assert_eq!(training_rows.rows.len(), 2);
        let mp4_tracks = run_query(
            &mut adbc.connection,
            &sql::video_tracks("spiral_robotics.video_sources", &episode.start.id),
            10,
        )
        .unwrap();
        assert_eq!(mp4_tracks.rows.len(), 1);
        let payload = run_binary_query(
            &mut adbc.connection,
            &sql::video_asset(
                "spiral_robotics",
                &episode.start.id,
                "follower.camera.wrist",
                None,
            ),
        )
        .unwrap();
        assert!(!payload.is_empty());
    }

    /// Reads one already-published managed MP4 through the exact frame path
    /// used by native ACT training. This is read-only; it never republishes or
    /// mutates the selected project.
    #[test]
    #[ignore = "requires a running SpiralDB service with a published video episode"]
    fn spiral_adbc_service_decodes_native_training_tensor() {
        let driver_path = PathBuf::from(std::env::var("SPIRALDB_ADBC_DRIVER").unwrap());
        let project_id = std::env::var("SPIRAL_ROBOTICS_SPIRAL_PROJECT").unwrap();
        let dataset = std::env::var("SPIRAL_ROBOTICS_SPIRAL_DATASET")
            .unwrap_or_else(|_| "spiral_robotics".to_owned());
        let episode_id = std::env::var("SPIRAL_ROBOTICS_TEST_EPISODE").unwrap();
        validate_safe_value("episode ID", &episode_id).unwrap();
        validate_dataset_identifier(&dataset).unwrap();
        let settings = StoredSpiralSettings {
            enabled: true,
            uri: "spiraldb://".to_owned(),
            project_id,
            dataset: dataset.clone(),
        };
        let mut adbc = open_adbc(AdbcConnectionKey::new(&settings, driver_path)).unwrap();
        let assets = run_query(
            &mut adbc.connection,
            &format!(
                "SELECT media_relation, stream_id, segment_index FROM {dataset}.video_assets \
                 WHERE episode_id = '{}' ORDER BY stream_id, segment_index LIMIT 1",
                episode_id.replace('\'', "''")
            ),
            1,
        )
        .unwrap();
        let asset = assets
            .rows
            .first()
            .expect("the selected episode must have a published video asset");
        let media_relation = asset["media_relation"].as_str().unwrap();
        assert!(media_relation.split('.').all(spiral::is_identifier));
        let stream_id = asset["stream_id"].as_str().unwrap();
        let segment_index = asset["segment_index"].as_u64().unwrap();
        let tensor_sql = format!(
            "SELECT f.request_index, \
             spql.image.to_tensor(\
               spql.image.resize(\
                 spql.video.decode(f.frame, 'auto'), $1, $2, 'bilinear'\
               ), 'chw', 'imagenet'\
             ) AS image \
             FROM {media_relation} AS a CROSS JOIN LATERAL \
             spql.video.frames(a.media, spql.video.frame_indices(ARRAY[0])) AS f \
             WHERE a.episode_id = '{}' AND a.stream_id = '{}' \
             AND a.segment_index = {segment_index} ORDER BY f.request_index",
            episode_id.replace('\'', "''"),
            stream_id.replace('\'', "''"),
        );
        let batches = crate::backend::training::data::query_image_batches(
            &mut adbc.connection,
            &tensor_sql,
            32,
            32,
        )
        .unwrap();
        let mut decoded = 0;
        for batch in &batches {
            let images = batch.column_by_name("image").unwrap();
            for row in 0..batch.num_rows() {
                let (values, shape) =
                    crate::backend::training::data::tensor_value(images, row).unwrap();
                assert_eq!(shape, [3, 32, 32]);
                assert_eq!(values.len(), 3 * 32 * 32);
                assert!(values.iter().all(|value| value.is_finite()));
                decoded += 1;
            }
        }
        assert_eq!(decoded, 1);

        let request = spiral_robotics_core::api::StartTrainingRequest {
            sql: format!(
                "{}\n  AND j.episode_id = '{}'",
                spiral::default_training_sql(&dataset),
                episode_id.replace('\'', "''"),
            ),
            batch_size: 1,
            chunk_size: 4,
            act: spiral_robotics_core::api::ActTrainingConfig {
                camera_streams: vec![stream_id.to_owned()],
                image_width: 32,
                image_height: 32,
                ..spiral_robotics_core::api::ActTrainingConfig::default()
            },
            ..spiral_robotics_core::api::StartTrainingRequest::default()
        };
        let mut loader = crate::backend::training::data::SpiralActLoader::open(
            &mut adbc.connection,
            &request,
            &dataset,
        )
        .unwrap();
        let loaded = loader.fetch(1, false).unwrap();
        assert_eq!(loaded.batch.batch_size, 1);
        assert_eq!(loaded.batch.states.len(), loader.state_dimensions());
        assert_eq!(
            loaded.batch.actions.len(),
            request.chunk_size * loader.action_dimensions()
        );
        assert_eq!(loaded.batch.images.len(), 3 * 32 * 32);
        assert_eq!(loaded.metrics.requested_frames, 1);

        let training = spiral_robotics_inference::act::VisionActTrainingConfig {
            state_dimensions: loader.state_dimensions(),
            action_dimensions: loader.action_dimensions(),
            camera_streams: vec![stream_id.to_owned()],
            image_width: 32,
            image_height: 32,
            chunk_size: request.chunk_size,
            hidden_dimensions: 64,
            latent_dimensions: 8,
            attention_heads: 8,
            feedforward_dimensions: 128,
            encoder_layers: 1,
            vae_encoder_layers: 1,
            decoder_layers: 1,
            kl_weight: request.act.kl_weight,
            learning_rate: request.learning_rate,
            total_steps: 1,
            seed: request.seed,
        };
        let mut trainer =
            spiral_robotics_inference::act::NativeActTrainer::new(training, "cpu", None).unwrap();
        let loss = trainer.train_step(loaded.batch, 1).unwrap();
        assert!(loss.total.is_finite());
    }

    /// Publishes one real, locally recorded episode to a running SpiralDB
    /// service, then reads its relational rows, MP4 metadata/index, and
    /// original browser payload back through the same ADBC connection.
    ///
    /// Run explicitly from `crates/spiral-robotics-studio` with the service endpoints,
    /// driver, project, episode directory, and episode ID in the environment.
    #[test]
    #[ignore = "requires a running SpiralDB service and mutates the selected project dataset"]
    fn spiral_adbc_service_round_trips_recorded_episode_and_video() {
        let driver_path = PathBuf::from(std::env::var("SPIRALDB_ADBC_DRIVER").unwrap());
        let project_id = std::env::var("SPIRAL_ROBOTICS_SPIRAL_PROJECT").unwrap();
        let dataset = std::env::var("SPIRAL_ROBOTICS_SPIRAL_DATASET")
            .unwrap_or_else(|_| "spiral_robotics".to_owned());
        let episode_id = std::env::var("SPIRAL_ROBOTICS_TEST_EPISODE").unwrap();
        let episode_directory = std::env::var("SPIRAL_ROBOTICS_TEST_EPISODES")
            .unwrap_or_else(|_| "episodes".to_owned());
        let episode_directory = PathBuf::from(episode_directory);
        let store = EpisodeStore::open(episode_directory.clone()).unwrap();
        let episode = store.completed(&episode_id).unwrap();
        let videos = load_assets(store.directory(), &episode_id).unwrap();
        assert!(
            !videos.is_empty(),
            "the service round trip needs recorded MP4 media"
        );
        let settings = StoredSpiralSettings {
            enabled: true,
            uri: "spiraldb://".to_owned(),
            project_id,
            dataset: dataset.clone(),
        };
        let mut adbc = open_adbc(AdbcConnectionKey::new(&settings, driver_path.clone())).unwrap();

        publish_spiral_blocking(&mut adbc.connection, &settings, &episode, &videos).unwrap();

        let query_count = |connection: &mut ManagedConnection, table: Table| {
            let relation = sql::relation(&dataset, table);
            run_query(
                connection,
                &sql::count_episode_rows(&relation, &episode_id),
                1,
            )
            .unwrap()
            .rows[0]["rows"]
                .as_u64()
                .unwrap()
        };
        assert_eq!(query_count(&mut adbc.connection, Table::Episodes), 1);
        assert_eq!(
            query_count(&mut adbc.connection, Table::JointSamples),
            (episode.samples.len() * episode.start.arms.len()) as u64
        );
        assert_eq!(
            query_count(&mut adbc.connection, Table::VideoAssets),
            videos.len() as u64
        );

        let sources = sql::video_sources_relation(
            &settings.dataset,
            &episode_id,
            uses_managed_catalog(&settings),
        );
        let tracks = run_query(
            &mut adbc.connection,
            &sql::video_tracks(&sources, &episode_id),
            100,
        )
        .unwrap();
        assert!(tracks.rows.len() >= videos.len());

        if uses_managed_catalog(&settings) {
            let indexed = run_query(
                &mut adbc.connection,
                &sql::count_rows(&sql::video_dataset_output(&settings.dataset, &episode_id)),
                1,
            )
            .unwrap();
            assert!(indexed.rows[0]["rows"].as_u64().unwrap() >= videos.len() as u64);
        }

        let selected = &videos[0];
        let payload = run_binary_query(
            &mut adbc.connection,
            &sql::video_asset(
                &dataset,
                &episode_id,
                &selected.stream_id,
                Some(selected.segment_index),
            ),
        )
        .unwrap();
        assert_eq!(payload, std::fs::read(&selected.path).unwrap());

        // Exercise the same settings, connection cache, query/replay, and
        // browser-video methods used by the HTTP route layer.
        let data_settings = Arc::new(DataSettingsStore::open(&episode_directory).unwrap());
        let configured = data_settings.get().spiral;
        assert_eq!(configured.project_id, settings.project_id);
        assert_eq!(configured.dataset, settings.dataset);
        let manager = Arc::new(
            StorageManager::new(
                data_settings,
                Arc::new(EpisodeStore::open(episode_directory).unwrap()),
                Some(driver_path),
            )
            .unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let connection = manager.test_spiral().await;
            assert!(connection.ok, "{}", connection.message);

            // This is the exact asynchronous path used by an explicit
            // publication request for an already staged local episode.
            manager.sync_spiral_existing(episode_id.clone()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                loop {
                    let status = manager
                        .statuses()
                        .into_iter()
                        .find(|status| status.episode_id == episode_id)
                        .unwrap();
                    if status.state == "published" {
                        break;
                    }
                    assert_ne!(status.state, "failed", "{status:?}");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("queued Spiral publication timed out");

            let queried = manager
                .query(
                    &sql::episode_exists(&sql::relation(&dataset, Table::Episodes), &episode_id),
                    10,
                )
                .await
                .unwrap();
            assert_eq!(queried.rows.len(), 1);
            assert_eq!(queried.rows[0][db::EPISODE_ID], episode_id);

            let stream = &episode.start.arms[0].id;
            let (_, trajectory) = manager.trajectory(&episode_id, stream).await.unwrap();
            assert_eq!(trajectory.len(), episode.samples.len());

            let delivered = manager
                .video_asset(
                    &episode_id,
                    &selected.stream_id,
                    Some(selected.segment_index),
                )
                .await
                .unwrap();
            assert_eq!(delivered, std::fs::read(&selected.path).unwrap());
        });
    }

    fn test_episode() -> CompletedEpisode {
        let start = crate::backend::recording::episodes::EpisodeStart {
            dataset_id: "default".to_owned(),
            id: "episode_000001".to_owned(),
            started_ms: 1,
            sample_hz: 30.0,
            source: "test".to_owned(),
            note: None,
            arms: vec![EpisodeArm {
                id: "follower".to_owned(),
                robot_type: Some("so101_follower".to_owned()),
                joints: vec!["joint".to_owned()],
            }],
            condition: Vec::new(),
        };
        let sample = crate::backend::recording::episodes::Sample {
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
            completion: crate::backend::recording::episodes::EpisodeCompletion {
                duration_seconds: 1.0 / 30.0,
                recording_gaps: Vec::new(),
                max_follow_error_degrees: None,
                discarded: false,
                condition: Vec::new(),
            },
        }
    }
}
