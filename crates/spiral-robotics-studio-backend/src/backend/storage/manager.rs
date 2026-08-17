//! SpiralDB persistence, query preview, and remote replay for Studio data.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arrow_array::{
    Array as _, ArrayRef, BinaryArray, BinaryViewArray, Float64Array, Int64Array, LargeBinaryArray,
    LargeStringArray, RecordBatch, StringArray, StringViewArray, UInt32Array, UInt64Array,
};
use arrow_json::writer::{JsonArray, WriterBuilder};
use arrow_schema::{DataType, Field, Schema};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map as JsonMap, Value as JsonValue};
use spiral_robotics_contracts::api::{
    ConnectionTestResponse, DataSettings, QueryColumn, SpiralQueryResponse, SpiralSettings,
};
use spiral_robotics_contracts::spiral::{self, Table, column as db};

use crate::backend::application::checkpoints::{
    CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointRepository, CheckpointWrite,
    validate_checkpoint_artifact_size,
};
use crate::backend::recording::episodes::CompletedEpisode;
use crate::backend::recording::spiral::{
    SpiralEpisodeEncoder, VideoStreamRow, dataset_batch, episode_completion_batch,
    episode_streams_batch, joint_samples_batch, video_streams_batch,
};
use crate::backend::robotics::video::VideoAsset;
use crate::backend::storage::bundle;
use crate::backend::storage::repository::{RepositoryConnection, SpiralRepository};
use crate::backend::storage::settings::StoredStudioPreferences;
use crate::backend::storage::sql::{self, ReadOnlyMode};

const MAX_QUERY_ROWS: usize = 1_000;
const MAX_QUERY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_INTERNAL_QUERY_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
const QUERY_JSON_CHUNK_ROWS: usize = 64;
const MAX_REPLAY_ROWS: usize = 250_000;
const JOINT_BATCH_ROWS: usize = 512;
pub(crate) struct StorageManager {
    repository: Arc<SpiralRepository>,
}

impl StorageManager {
    pub(crate) fn new(repository: Arc<SpiralRepository>) -> Self {
        Self { repository }
    }

    pub(crate) fn data_settings(&self, preferences: &StoredStudioPreferences) -> DataSettings {
        DataSettings {
            spiral: SpiralSettings {
                endpoint: self.repository.endpoint().to_owned(),
                spfs_endpoint: self.repository.spfs_endpoint().to_owned(),
                project_id: self.repository.project_id(),
                client_id: self
                    .repository
                    .credentials()
                    .map(|credentials| credentials.client_id)
                    .unwrap_or_default(),
                client_secret_configured: self.repository.credentials().is_some(),
                namespace: self.repository.namespace().to_owned(),
                endpoint_restart_required: false,
            },
            training: preferences.training.clone(),
            video: preferences.video.clone(),
        }
    }

    pub(crate) fn project_id(&self) -> String {
        self.repository.project_id()
    }

    pub(crate) fn spiral_endpoint(&self) -> String {
        self.repository.endpoint()
    }

    pub(crate) fn spiral_spfs_endpoint(&self) -> String {
        self.repository.spfs_endpoint()
    }

    pub(crate) fn spiral_credentials(
        &self,
    ) -> Option<crate::backend::storage::settings::SpiralCredentials> {
        self.repository.credentials()
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.repository.is_configured()
    }

    pub(crate) async fn prewarm(&self) -> Result<(), String> {
        self.repository.prewarm().await
    }

    pub(crate) fn configure_project(&self, project_id: &str) -> Result<(), String> {
        self.repository.configure_project(project_id)
    }

    pub(crate) fn configure_connection(
        &self,
        endpoint: String,
        spfs_endpoint: String,
        project_id: &str,
        credentials: Option<crate::backend::storage::settings::SpiralCredentials>,
    ) -> Result<(), String> {
        self.repository
            .configure_connection(endpoint, spfs_endpoint, project_id, credentials)
    }

    pub(crate) fn invalidate_idle_connections(&self) {
        self.repository.invalidate_idle_connections();
    }

    pub(crate) fn namespace(&self) -> &str {
        self.repository.namespace()
    }

    /// Commit a complete episode transaction directly to SpiralDB. This is
    /// also used once more when finalized camera segments arrive after the
    /// joint writer has inserted the episode's initial catalog row.
    pub(crate) fn commit_episode_blocking(
        &self,
        episode: &CompletedEpisode,
        videos: &[VideoAsset],
    ) -> Result<(), String> {
        let namespace = self.repository.namespace().to_owned();
        self.repository.run_blocking(|connection| {
            replace_episode_blocking(connection, &namespace, episode, videos)
        })
    }

    pub(crate) fn load_training_snapshot<T>(&self) -> Result<T, String>
    where
        T: DeserializeOwned + Default,
    {
        if !self.is_configured() {
            return Ok(T::default());
        }
        self.load_json_document(Table::TrainingRuns, "training_history")
    }

    pub(crate) fn save_training_snapshot<T: Serialize>(&self, value: &T) -> Result<(), String> {
        self.save_json_document(Table::TrainingRuns, "training_history", value)
    }

    pub(crate) fn load_rollout_snapshot<T>(&self) -> Result<T, String>
    where
        T: DeserializeOwned + Default,
    {
        if !self.is_configured() {
            return Ok(T::default());
        }
        self.load_json_document(Table::RolloutRuns, "rollout_history")
    }

    pub(crate) fn save_rollout_snapshot<T: Serialize>(&self, value: &T) -> Result<(), String> {
        self.save_json_document(Table::RolloutRuns, "rollout_history", value)
    }

    pub(crate) fn load_studio_preferences(&self) -> Result<StoredStudioPreferences, String> {
        if !self.is_configured() {
            return Ok(StoredStudioPreferences::default());
        }
        let relation = sql::relation(self.repository.namespace(), Table::StudioSettings);
        self.repository.run_blocking(move |connection| {
            if !table_exists(connection, &relation)? {
                return Ok(StoredStudioPreferences::default());
            }
            let result = run_query(
                connection,
                &format!(
                    "SELECT {} FROM {relation} WHERE {} = 'preferences' ORDER BY {} DESC LIMIT 1",
                    db::VALUE_JSON,
                    db::KEY,
                    db::UPDATED_MS,
                ),
                1,
            )?;
            let Some(value) = result
                .rows
                .first()
                .and_then(|row| row.get(db::VALUE_JSON))
                .and_then(JsonValue::as_str)
            else {
                return Ok(StoredStudioPreferences::default());
            };
            serde_json::from_str(value)
                .map_err(|error| format!("decoding Studio preferences: {error}"))
        })
    }

    pub(crate) fn save_studio_preferences(
        &self,
        preferences: &StoredStudioPreferences,
    ) -> Result<(), String> {
        let namespace = self.repository.namespace().to_owned();
        let relation = sql::relation(&namespace, Table::StudioSettings);
        let json = serde_json::to_string(preferences)
            .map_err(|error| format!("encoding Studio preferences: {error}"))?;
        self.repository.run_blocking(move |connection| {
            ensure_dataset_namespace(connection, &namespace)?;
            if table_exists(connection, &relation)? {
                execute_delete_sql(
                    connection,
                    &format!("DELETE FROM {relation} WHERE {} = 'preferences'", db::KEY),
                )?;
            }
            let schema = Arc::new(Schema::new(vec![
                Field::new(db::KEY, DataType::Utf8, false),
                Field::new(db::VALUE_JSON, DataType::Utf8, false),
                Field::new(db::UPDATED_MS, DataType::Int64, false),
            ]));
            let batch = RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(StringArray::from(vec!["preferences"])) as ArrayRef,
                    Arc::new(StringArray::from(vec![json])) as ArrayRef,
                    Arc::new(Int64Array::from(vec![unix_millis()])) as ArrayRef,
                ],
            )
            .map_err(|error| format!("building Studio preferences batch: {error}"))?;
            ingest_batch(connection, &relation, batch)
        })
    }

    pub(crate) fn export_project_bundle(&self) -> Result<Vec<u8>, String> {
        let namespace = self.repository.namespace().to_owned();
        self.repository
            .run_exclusive_blocking(move |connection| bundle::export(connection, &namespace))
    }

    pub(crate) fn import_project_bundle(
        &self,
        bytes: &[u8],
    ) -> Result<spiral_robotics_contracts::api::ProjectImportSummary, String> {
        let namespace = self.repository.namespace().to_owned();
        self.repository
            .run_exclusive_blocking(move |connection| bundle::import(connection, &namespace, bytes))
    }

    pub(crate) fn export_episode_bundle(&self, dataset_id: &str) -> Result<Vec<u8>, String> {
        let namespace = self.repository.namespace().to_owned();
        let dataset_id = dataset_id.to_owned();
        self.repository.run_exclusive_blocking(move |connection| {
            bundle::export_episodes(connection, &namespace, &dataset_id)
        })
    }

    pub(crate) fn import_episode_bundle(
        &self,
        dataset_id: &str,
        bytes: &[u8],
    ) -> Result<spiral_robotics_contracts::api::EpisodeImportSummary, String> {
        let namespace = self.repository.namespace().to_owned();
        let dataset_id = dataset_id.to_owned();
        self.repository.run_exclusive_blocking(move |connection| {
            bundle::import_episodes(connection, &namespace, &dataset_id, bytes)
        })
    }

    fn load_json_document<T>(&self, table: Table, key: &str) -> Result<T, String>
    where
        T: DeserializeOwned + Default,
    {
        let relation = sql::relation(self.repository.namespace(), table);
        let key = sql::string_literal(key);
        self.repository.run_blocking(move |connection| {
            if !table_exists(connection, &relation)? {
                return Ok(T::default());
            }
            let result = run_query(
                connection,
                &format!(
                    "SELECT {} FROM {relation} WHERE {} = {key} ORDER BY {} DESC LIMIT 1",
                    db::SNAPSHOT_JSON,
                    db::RUN_ID,
                    db::UPDATED_MS,
                ),
                1,
            )?;
            let Some(value) = result
                .rows
                .first()
                .and_then(|row| row.get(db::SNAPSHOT_JSON))
                .and_then(JsonValue::as_str)
            else {
                return Ok(T::default());
            };
            serde_json::from_str(value)
                .map_err(|error| format!("decoding {table} snapshot: {error}"))
        })
    }

    fn save_json_document<T: Serialize>(
        &self,
        table: Table,
        key: &str,
        value: &T,
    ) -> Result<(), String> {
        let namespace = self.repository.namespace().to_owned();
        let relation = sql::relation(&namespace, table);
        let key = key.to_owned();
        let key_sql = sql::string_literal(&key);
        let json = serde_json::to_string(value)
            .map_err(|error| format!("encoding {table} snapshot: {error}"))?;
        self.repository.run_blocking(move |connection| {
            ensure_dataset_namespace(connection, &namespace)?;
            if table_exists(connection, &relation)? {
                execute_delete_sql(
                    connection,
                    &format!("DELETE FROM {relation} WHERE {} = {key_sql}", db::RUN_ID),
                )?;
            }
            let schema = Arc::new(Schema::new(vec![
                Field::new(db::RUN_ID, DataType::Utf8, false),
                Field::new(db::SNAPSHOT_JSON, DataType::Utf8, false),
                Field::new(db::UPDATED_MS, DataType::Int64, false),
            ]));
            let batch = RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(StringArray::from(vec![key])) as ArrayRef,
                    Arc::new(StringArray::from(vec![json])) as ArrayRef,
                    Arc::new(Int64Array::from(vec![unix_millis()])) as ArrayRef,
                ],
            )
            .map_err(|error| format!("building {table} snapshot batch: {error}"))?;
            ingest_batch(connection, &relation, batch)
        })
    }

    pub(crate) async fn test_spiral(&self) -> ConnectionTestResponse {
        let endpoint = self.repository.endpoint().to_owned();
        let project = self.repository.project_id();
        if project.is_empty() {
            return ConnectionTestResponse {
                ok: false,
                message: "Choose a Spiral project in Settings before testing the connection."
                    .to_owned(),
            };
        }
        let catalog_project = project.clone();
        let table_filter = sql::relation(self.repository.namespace(), Table::Episodes);
        if let Err(error) = self
            .repository
            .run(move |connection| {
                connection
                    .check_catalog_table(&catalog_project, &table_filter)
                    .map_err(|error| format!("checking the managed project catalog: {error}"))
            })
            .await
        {
            return ConnectionTestResponse {
                ok: false,
                message: error,
            };
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
        let sql = sql.to_owned();
        self.repository
            .run(move |connection| {
                run_query_with_byte_limit(
                    connection,
                    &sql,
                    max_rows.clamp(1, MAX_QUERY_ROWS),
                    MAX_QUERY_RESPONSE_BYTES,
                )
            })
            .await
    }

    /// Resolve the query and its Arrow contract before starting the trainer.
    /// Unknown relations and renamed fields therefore fail the start request,
    /// rather than becoming an asynchronous training-task failure.
    pub(crate) async fn validate_training_query(
        &self,
        query: &str,
        state_column: &str,
        action_column: &str,
    ) -> Result<(), String> {
        sql::ensure_read_only(query, ReadOnlyMode::Training)?;
        let query = query.to_owned();
        let state_column = state_column.to_owned();
        let action_column = action_column.to_owned();
        let dataset = self.repository.namespace().to_owned();
        self.repository
            .run(move |connection| {
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
    /// previews or checkpoint writes on the cached control connection.
    pub(crate) async fn run_training_adbc<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut RepositoryConnection) -> Result<T, String> + Send + 'static,
    {
        self.repository.run_dedicated(operation).await
    }

    /// Save a trainer-produced artifact through the backend's trusted ADBC
    /// control path. Checkpoint writes remain separate from the training read
    /// connection, so persistence cannot deadlock a streaming batch read.
    async fn save_checkpoint_artifact(&self, write: CheckpointWrite) -> Result<(), String> {
        let target = sql::relation(self.repository.namespace(), Table::TrainingCheckpoints);
        self.repository
            .run(move |connection| {
                let batch = training_checkpoint_batch(write)?;
                ingest_batch(connection, &target, batch)
                    .map_err(|error| format!("saving training checkpoint to {target}: {error}"))
            })
            .await
    }

    async fn load_checkpoint_artifact(
        &self,
        key: CheckpointKey,
    ) -> Result<CheckpointArtifact, String> {
        let query = sql::training_checkpoint(self.repository.namespace(), &key.run_id, key.step);
        self.repository
            .run(move |connection| run_training_checkpoint_query(connection, &query))
            .await
    }

    async fn delete_checkpoint_artifacts(&self, run_id: String) -> Result<(), String> {
        let target = sql::relation(self.repository.namespace(), Table::TrainingCheckpoints);
        self.repository
            .run(move |connection| {
                if !table_exists(connection, &target)? {
                    return Ok(());
                }
                execute_delete_sql(connection, &sql::delete_training_run(&target, &run_id)).map_err(
                    |error| {
                        format!(
                            "deleting training run {run_id:?} checkpoints from {target}: {error}"
                        )
                    },
                )
            })
            .await
    }

    pub(crate) async fn trajectory(
        &self,
        episode: &str,
        stream: &str,
    ) -> Result<(f64, Vec<BTreeMap<String, f64>>), String> {
        let sql = sql::trajectory(self.repository.namespace(), episode, stream);
        let result = self
            .repository
            .run(move |connection| run_query(connection, &sql, MAX_REPLAY_ROWS))
            .await?;
        if result.truncated {
            return Err(format!(
                "Spiral trajectory exceeds the replay safety limit of {MAX_REPLAY_ROWS} rows"
            ));
        }
        decode_trajectory(result.rows)
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
        let sql = sql::video_asset(self.repository.namespace(), episode, stream, segment);
        self.repository
            .run(move |connection| run_binary_query(connection, &sql))
            .await
    }
}

/// Decode remote replay data without dropping malformed joints or inventing a
/// clock. Physical replay is all-or-nothing: every row must declare the same
/// finite sample rate and one exact, unique, finite pose.
fn decode_trajectory(
    rows: Vec<JsonMap<String, JsonValue>>,
) -> Result<(f64, Vec<BTreeMap<String, f64>>), String> {
    let mut sample_hz: Option<f64> = None;
    let mut trajectory = Vec::with_capacity(rows.len());
    for (row_index, row) in rows.into_iter().enumerate() {
        let row_hz = row
            .get(db::SAMPLE_HZ)
            .and_then(JsonValue::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0)
            .ok_or_else(|| {
                format!(
                    "Spiral trajectory row {row_index} has no finite positive {}",
                    db::SAMPLE_HZ
                )
            })?;
        if sample_hz.is_some_and(|expected| expected.to_bits() != row_hz.to_bits()) {
            return Err(format!(
                "Spiral trajectory row {row_index} changed {}",
                db::SAMPLE_HZ
            ));
        }
        sample_hz.get_or_insert(row_hz);

        let names = row
            .get(db::JOINT_NAMES)
            .and_then(JsonValue::as_array)
            .ok_or_else(|| {
                format!(
                    "Spiral trajectory row {row_index} is missing {}",
                    db::JOINT_NAMES
                )
            })?;
        let values = row
            .get(db::REPLAY_POSITION)
            .and_then(JsonValue::as_array)
            .ok_or_else(|| {
                format!(
                    "Spiral trajectory row {row_index} is missing {}",
                    db::REPLAY_POSITION
                )
            })?;
        if names.is_empty() || names.len() != values.len() {
            return Err(format!(
                "Spiral trajectory row {row_index} has {} joint names but {} positions",
                names.len(),
                values.len()
            ));
        }
        let mut pose = BTreeMap::new();
        for (joint_index, (name, value)) in names.iter().zip(values).enumerate() {
            let name = name
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| {
                    format!(
                        "Spiral trajectory row {row_index} joint {joint_index} has no valid name"
                    )
                })?;
            let value = value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    format!(
                        "Spiral trajectory row {row_index} joint {name:?} has no finite position"
                    )
                })?;
            if pose.insert(name.to_owned(), value).is_some() {
                return Err(format!(
                    "Spiral trajectory row {row_index} repeats joint {name:?}"
                ));
            }
        }
        trajectory.push(pose);
    }
    let sample_hz =
        sample_hz.ok_or_else(|| "Spiral returned no commandable trajectory rows".to_owned())?;
    Ok((sample_hz, trajectory))
}

impl CheckpointRepository for StorageManager {
    fn save_checkpoint(&self, write: CheckpointWrite) -> CheckpointFuture<'_, ()> {
        Box::pin(async move { self.save_checkpoint_artifact(write).await })
    }

    fn load_checkpoint(&self, key: CheckpointKey) -> CheckpointFuture<'_, CheckpointArtifact> {
        Box::pin(async move { self.load_checkpoint_artifact(key).await })
    }

    fn delete_run_checkpoints(&self, run_id: String) -> CheckpointFuture<'_, ()> {
        Box::pin(async move { self.delete_checkpoint_artifacts(run_id).await })
    }
}

pub(crate) fn replace_episode_blocking(
    connection: &mut RepositoryConnection,
    namespace: &str,
    episode: &CompletedEpisode,
    videos: &[VideoAsset],
) -> Result<(), String> {
    ensure_dataset_namespace(connection, namespace)?;
    // Replacement is idempotent. Clear any earlier complete row and partial
    // children before writing, so a retry after a process or network failure
    // can finish without duplicating already-landed rows.
    clear_episode_rows(connection, namespace, &episode.start.id)?;
    let datasets = sql::relation(namespace, Table::Datasets);
    if !table_has_dataset(connection, &datasets, &episode.start.dataset_id)? {
        ingest_batch(
            connection,
            &datasets,
            dataset_batch(&episode.start).map_err(|error| error.to_string())?,
        )?;
    }
    ingest_batch(
        connection,
        &sql::relation(namespace, Table::EpisodeStreams),
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
            &sql::relation(namespace, Table::EpisodeStreams),
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
                &sql::relation(namespace, Table::JointSamples),
                batch,
            )?;
            rows.clear();
        }
    }
    if !rows.is_empty() {
        let batch = joint_samples_batch(&rows).map_err(|error| error.to_string())?;
        ingest_batch(
            connection,
            &sql::relation(namespace, Table::JointSamples),
            batch,
        )?;
    }
    if !videos.is_empty() {
        let media_relation = sql::video_dataset_output(namespace, &episode.start.id);
        let batch = video_assets_batch(&episode.start.id, videos, &media_relation)?;
        ingest_batch(
            connection,
            &sql::relation(namespace, Table::VideoAssets),
            batch,
        )?;
        // The view turns durable binary MP4 payloads into Spiral Blob values;
        // the managed dataset then persists the MP4/video index used by
        // `spql.video.*` frame selection and decode.
        let sources = sql::video_sources_relation(namespace, &episode.start.id, true);
        let assets = sql::relation(namespace, Table::VideoAssets);
        let dataset = sql::video_dataset_relation(namespace, &episode.start.id);
        execute_update_sql(
            connection,
            &sql::create_video_sources(&sources, &assets, Some(episode.start.id.as_str())),
        )
        .map_err(|error| format!("creating Spiral video source view: {error}"))?;
        // Completed episodes are immutable, so each has one immutable managed
        // MP4 Dataset. This avoids flattening the native VideoIndex extension
        // through ADBC metadata or relying on output replacement.
        let created =
            execute_update_sql(connection, &sql::create_video_dataset(&dataset, &sources));
        let needs_refresh = match created {
            Ok(()) => true,
            Err(error) if error.contains("already exists") => {
                let output = sql::video_dataset_output(namespace, &episode.start.id);
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
    // Inserted last: readers never see an episode whose streams or media only
    // partially arrived.
    ingest_batch(
        connection,
        &sql::relation(namespace, Table::Episodes),
        episode_completion_batch(&episode.start, &episode.completion, encoder.frames())
            .map_err(|error| error.to_string())?,
    )?;
    Ok(())
}

/// Spiral's managed catalog is a resource tree: `spiral_robotics.episodes` can only
/// be created after a live `spiral_robotics` parent exists. Until the catalog exposes
/// a standalone namespace resource, a tiny durable root table is the parent
/// container for every Spiral Robotics table, view, and managed Dataset.
pub(crate) fn ensure_dataset_namespace(
    connection: &mut RepositoryConnection,
    dataset: &str,
) -> Result<(), String> {
    match connection.table_schema(dataset) {
        Ok(Some(schema))
            if schema
                .field_with_name(spiral::DATASET_NAMESPACE_MARKER)
                .is_ok() =>
        {
            return Ok(());
        }
        Ok(Some(_)) => {
            return Err(format!(
                "SpiralDB namespace {dataset} exists but is not managed by Spiral Robotics; choose a new namespace or remove that resource"
            ));
        }
        Ok(None) => {}
        Err(error) => {
            return Err(format!("checking SpiralDB namespace {dataset}: {error}"));
        }
    }
    execute_update_sql(connection, &sql::create_dataset_namespace(dataset))
        .map_err(|error| format!("creating SpiralDB namespace {dataset}: {error}"))
}

pub(crate) fn clear_episode_rows(
    connection: &mut RepositoryConnection,
    namespace: &str,
    episode_id: &str,
) -> Result<(), String> {
    for table in Table::EPISODE_RELATIONS {
        let target = sql::relation(namespace, table);
        if !table_exists(connection, &target)? {
            continue;
        }
        execute_delete_sql(connection, &sql::delete_episode(&target, episode_id))
            .map_err(|error| format!("clearing prior Spiral rows in {target}: {error}"))?;
    }
    Ok(())
}

fn table_has_dataset(
    connection: &mut RepositoryConnection,
    target: &str,
    dataset_id: &str,
) -> Result<bool, String> {
    if !table_exists(connection, target)? {
        return Ok(false);
    }
    let result = run_query(connection, &sql::dataset_exists(target, dataset_id), 1)?;
    Ok(!result.rows.is_empty())
}

pub(crate) fn ingest_batch(
    connection: &mut RepositoryConnection,
    target: &str,
    batch: RecordBatch,
) -> Result<(), String> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening ADBC ingest for {target}: {error}"))?;
    statement
        .configure_ingest(target)
        .map_err(|error| format!("configuring ADBC ingest for {target}: {error}"))?;
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

fn training_checkpoint_batch(checkpoint: CheckpointWrite) -> Result<RecordBatch, String> {
    validate_checkpoint_artifact_size(checkpoint.config_json.len(), checkpoint.payload.len())?;
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

pub(crate) fn table_exists(
    connection: &RepositoryConnection,
    target: &str,
) -> Result<bool, String> {
    connection
        .table_schema(target)
        .map(|schema| schema.is_some())
        .map_err(|error| format!("checking Spiral table {target}: {error}"))
}

pub(crate) fn execute_update_sql(
    connection: &mut RepositoryConnection,
    sql: &str,
) -> Result<(), String> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening Spiral SQL statement: {error}"))?;
    statement
        .set_sql_query(sql)
        .and_then(|()| statement.execute_update())
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Spiral reports a no-op DELETE as an execution error. Replacement and
/// cleanup callers intentionally treat that driver-specific result as success.
pub(crate) fn execute_delete_sql(
    connection: &mut RepositoryConnection,
    sql: &str,
) -> Result<(), String> {
    match execute_update_sql(connection, sql) {
        Err(error) if error.contains("without any affected rows") => Ok(()),
        result => result,
    }
}

fn video_assets_batch(
    episode_id: &str,
    videos: &[VideoAsset],
    media_relation: &str,
) -> Result<RecordBatch, String> {
    let payloads = videos
        .iter()
        .map(|video| video.payload.as_slice())
        .collect::<Vec<_>>();
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
        Arc::new(BinaryArray::from_iter_values(payloads.iter().copied())),
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

pub(crate) fn run_query(
    connection: &mut RepositoryConnection,
    sql: &str,
    max_rows: usize,
) -> Result<SpiralQueryResponse, String> {
    run_query_with_byte_limit(connection, sql, max_rows, MAX_INTERNAL_QUERY_RESPONSE_BYTES)
}

fn run_query_with_byte_limit(
    connection: &mut RepositoryConnection,
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

fn run_binary_query(connection: &mut RepositoryConnection, sql: &str) -> Result<Vec<u8>, String> {
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
    connection: &mut RepositoryConnection,
    sql: &str,
) -> Result<CheckpointArtifact, String> {
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
                let value = values.value(row);
                validate_checkpoint_artifact_size(0, value.len())?;
                value.to_vec()
            } else if let Some(values) = checkpoint.as_any().downcast_ref::<LargeBinaryArray>() {
                let value = values.value(row);
                validate_checkpoint_artifact_size(0, value.len())?;
                value.to_vec()
            } else if let Some(values) = checkpoint.as_any().downcast_ref::<BinaryViewArray>() {
                let value = values.value(row);
                validate_checkpoint_artifact_size(0, value.len())?;
                value.to_vec()
            } else {
                return Err(format!(
                    "Spiral checkpoint payload has unexpected type {}",
                    checkpoint.data_type()
                ));
            };
            let config_json = if let Some(values) = config.as_any().downcast_ref::<StringArray>() {
                let value = values.value(row).as_bytes();
                validate_checkpoint_artifact_size(value.len(), payload.len())?;
                value.to_vec()
            } else if let Some(values) = config.as_any().downcast_ref::<LargeStringArray>() {
                let value = values.value(row).as_bytes();
                validate_checkpoint_artifact_size(value.len(), payload.len())?;
                value.to_vec()
            } else if let Some(values) = config.as_any().downcast_ref::<StringViewArray>() {
                let value = values.value(row).as_bytes();
                validate_checkpoint_artifact_size(value.len(), payload.len())?;
                value.to_vec()
            } else {
                return Err(format!(
                    "Spiral checkpoint config has unexpected type {}",
                    config.data_type()
                ));
            };
            artifact = Some(CheckpointArtifact {
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

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_owned_arrow_schemas_match_the_shared_contract() {
        assert_eq!(
            training_checkpoint_schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            Table::TrainingCheckpoints.columns(),
        );
        assert_eq!(
            video_assets_schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            Table::VideoAssets.columns(),
        );
    }

    #[test]
    fn query_preview_rejects_mutation_and_multiple_statements() {
        assert!(
            sql::ensure_read_only("SELECT * FROM robotics.episodes", ReadOnlyMode::Preview).is_ok()
        );
        assert!(
            sql::ensure_read_only("DELETE FROM robotics.episodes", ReadOnlyMode::Preview).is_err()
        );
        assert!(sql::ensure_read_only("SELECT 1; SELECT 2", ReadOnlyMode::Preview).is_err());
    }

    #[test]
    fn remote_trajectory_requires_a_consistent_clock_and_pose() {
        let rows = vec![JsonMap::from_iter([
            (db::SAMPLE_HZ.to_owned(), JsonValue::from(60.0)),
            (
                db::JOINT_NAMES.to_owned(),
                serde_json::json!(["shoulder", "elbow"]),
            ),
            (
                db::REPLAY_POSITION.to_owned(),
                serde_json::json!([0.25, -0.5]),
            ),
        ])];
        let (sample_hz, trajectory) = decode_trajectory(rows).unwrap();
        assert_eq!(sample_hz, 60.0);
        assert_eq!(trajectory[0]["shoulder"], 0.25);
        assert_eq!(trajectory[0]["elbow"], -0.5);
    }

    #[test]
    fn malformed_remote_trajectory_is_rejected() {
        let rows = vec![JsonMap::from_iter([
            (db::SAMPLE_HZ.to_owned(), JsonValue::from(0.0)),
            (db::JOINT_NAMES.to_owned(), serde_json::json!(["shoulder"])),
            (db::REPLAY_POSITION.to_owned(), serde_json::json!([0.0])),
        ])];
        assert!(decode_trajectory(rows).is_err());
    }
}
