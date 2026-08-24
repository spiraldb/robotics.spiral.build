//! SpiralDB persistence, query preview, and remote replay for Studio data.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arrow_array::{
    Array as _, ArrayRef, BinaryArray, BinaryViewArray, FixedSizeListArray, Int32Array, Int64Array,
    LargeBinaryArray, LargeListArray, LargeStringArray, ListArray, RecordBatch,
    RecordBatchIterator, StringArray, StringViewArray, StructArray, UInt8Array, UInt32Array,
    UInt64Array,
};
use arrow_json::writer::{JsonArray, WriterBuilder};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{ArrowError, DataType, Field, Fields, Schema};
use base64::Engine as _;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map as JsonMap, Value as JsonValue};
use sha2::{Digest as _, Sha256};
use spiral_robotics_contracts::api::{
    ConnectionTestResponse, DataSettings, DatasetSummary, ProjectDataRelation,
    ProjectDataRelationsResponse, QueryColumn, SpiralQueryResponse, SpiralSettings,
};
use spiral_robotics_contracts::spiral::{self, Table, column as db};
use spiral_robotics_contracts::workflow::{
    WorkflowDocument, WorkflowRelation, WorkflowRelationsResponse,
};
use spiral_robotics_core::workflow::validate_qualified_relation;

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
use spiral_robotics_studio_application::checkpoints::{
    CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointRepository,
    validate_checkpoint_artifact_size,
};

const MAX_QUERY_ROWS: usize = 1_000;
const MAX_QUERY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_INTERNAL_QUERY_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
const QUERY_JSON_CHUNK_ROWS: usize = 64;
const QUERY_IMAGE_PREVIEW_MAX_EDGE: u32 = 640;
const QUERY_IMAGE_JPEG_QUALITY: u8 = 82;
const MAX_REPLAY_ROWS: usize = 250_000;
const JOINT_BATCH_ROWS: usize = 512;
const BLOB_EXTENSION_NAME: &str = "spql.io.Blob";
const IMAGE_EXTENSION_NAME: &str = "spql.image.Image";
const VARIABLE_TENSOR_EXTENSION_NAME: &str = "arrow.variable_shape_tensor";

pub(crate) struct StorageManager {
    pub(super) repository: Arc<SpiralRepository>,
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

    pub(crate) fn workflow_model_repository(&self) -> Arc<SpiralRepository> {
        self.repository.clone()
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
        self.repository.prewarm().await?;
        let namespace = self.repository.namespace().to_owned();
        self.repository
            .run(move |connection| ensure_video_assets_blob_relation(connection, &namespace))
            .await
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

    pub(crate) fn load_workflow_document(&self) -> Result<WorkflowDocument, String> {
        if !self.is_configured() {
            return Ok(WorkflowDocument::default());
        }
        self.load_json_document_resetting_incompatible(Table::WorkflowRuns, "default")
    }

    pub(crate) fn save_workflow_document(&self, value: &WorkflowDocument) -> Result<(), String> {
        self.save_json_document(Table::WorkflowRuns, "default", value)
    }

    pub(crate) fn load_workflow_agent_snapshot_json(&self) -> Result<Option<String>, String> {
        if !self.is_configured() {
            return Ok(None);
        }
        self.load_json_document_raw_optional(Table::WorkflowRuns, "agent:default")
    }

    pub(crate) fn save_workflow_agent_document<T: Serialize>(
        &self,
        value: &T,
    ) -> Result<(), String> {
        let namespace = self.repository.namespace().to_owned();
        let relation = sql::relation(&namespace, Table::WorkflowRuns);
        let json = serde_json::to_string(value)
            .map_err(|error| format!("encoding Workflow runs snapshot: {error}"))?;
        self.repository.run_exclusive_blocking(move |connection| {
            ensure_dataset_namespace(connection, &namespace)?;
            connection.set_autocommit(false)?;
            let mutation = save_json_document_on_connection(
                connection,
                Table::WorkflowRuns,
                &relation,
                "agent:default".to_owned(),
                json,
            )
            .and_then(|()| connection.commit());
            match mutation {
                Ok(()) => match connection.set_autocommit(true) {
                    Ok(()) => Ok(()),
                    Err(_) => {
                        // The document is committed. Discard the connection so
                        // manual-commit state can never leak into the pool, but
                        // do not tell the manager that durable state is old.
                        connection.discard();
                        Ok(())
                    }
                },
                Err(error) => {
                    let rollback = connection.rollback();
                    let restore = connection.set_autocommit(true);
                    let mut detail = error;
                    if let Err(rollback) = rollback {
                        detail.push_str(&format!(
                            "; rolling back Assistant history failed: {rollback}"
                        ));
                    }
                    if let Err(restore) = restore {
                        detail.push_str(&format!("; restoring ADBC autocommit failed: {restore}"));
                    }
                    // Returning an error prevents run_exclusive_blocking from
                    // checking this uncertain connection back into the pool.
                    Err(detail)
                }
            }
        })
    }

    /// Replace an incompatible prototype Assistant document only when the
    /// durable raw snapshot is still exactly the one Studio inspected. The
    /// compare and replacement share one catalog transaction, so a newer
    /// document from another Studio cannot be overwritten.
    pub(crate) fn reset_workflow_agent_document<T: Serialize>(
        &self,
        expected_raw_snapshot: &str,
        value: &T,
    ) -> Result<(), String> {
        let namespace = self.repository.namespace().to_owned();
        let relation = sql::relation(&namespace, Table::WorkflowRuns);
        let expected_digest = Sha256::digest(expected_raw_snapshot.as_bytes()).to_vec();
        let json = serde_json::to_string(value)
            .map_err(|error| format!("encoding reset Assistant history: {error}"))?;
        self.repository.run_exclusive_blocking(move |connection| {
            connection.set_autocommit(false)?;
            let mutation = (|| {
                let current = load_json_document_raw_optional_on_connection(
                    connection,
                    &relation,
                    "agent:default",
                )?
                .ok_or_else(|| {
                    "Assistant history changed before reset. Reload the current project."
                        .to_owned()
                })?;
                if Sha256::digest(current.as_bytes()).as_slice() != expected_digest.as_slice() {
                    return Err(
                        "Assistant history changed before reset. Reload the current project."
                            .to_owned(),
                    );
                }
                save_json_document_on_connection(
                    connection,
                    Table::WorkflowRuns,
                    &relation,
                    "agent:default".to_owned(),
                    json,
                )?;
                connection.commit()
            })();
            let rollback = mutation.is_err().then(|| connection.rollback());
            if rollback.as_ref().is_some_and(Result::is_err) {
                connection.discard();
            }
            let restore = connection.set_autocommit(true);
            if restore.is_err() {
                connection.discard();
            }
            match (mutation, rollback, restore) {
                (Ok(()), None, Ok(())) | (Ok(()), None, Err(_)) => Ok(()),
                (Err(error), Some(Ok(())), Ok(())) => Err(error),
                (Err(error), Some(rollback), restore) => Err(format!(
                    "{error}; Assistant reset cleanup was uncertain (rollback: {rollback:?}, autocommit: {restore:?})"
                )),
                _ => unreachable!("recovery rollback follows the mutation outcome"),
            }
        })
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
            ensure_video_assets_blob_relation(connection, &namespace)?;
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

    fn load_json_document_raw_optional(
        &self,
        table: Table,
        key: &str,
    ) -> Result<Option<String>, String> {
        let relation = sql::relation(self.repository.namespace(), table);
        let key = key.to_owned();
        self.repository.run_blocking(move |connection| {
            load_json_document_raw_optional_on_connection(connection, &relation, &key)
        })
    }

    /// Workflow wire shapes are prototype-owned rather than a compatibility
    /// surface. Replace an obsolete snapshot with the current empty document
    /// in one transaction so every later hydration sees the same truth.
    fn load_json_document_resetting_incompatible<T>(
        &self,
        table: Table,
        key: &str,
    ) -> Result<T, String>
    where
        T: DeserializeOwned + Default + Serialize,
    {
        let relation = sql::relation(self.repository.namespace(), table);
        let key = key.to_owned();
        self.repository.run_exclusive_blocking(move |connection| {
            if !table_exists(connection, &relation)? {
                return Ok(T::default());
            }
            let result = run_query(
                connection,
                &format!(
                    "SELECT {} FROM {relation} WHERE {} = {} ORDER BY {} DESC LIMIT 1",
                    db::SNAPSHOT_JSON,
                    db::RUN_ID,
                    sql::string_literal(&key),
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
            let (document, incompatible) = decode_json_document_or_default(value);
            if !incompatible {
                return Ok(document);
            }

            let json = serde_json::to_string(&document)
                .map_err(|error| format!("encoding reset {table} snapshot: {error}"))?;
            connection.set_autocommit(false)?;
            let mutation = (|| {
                save_json_document_on_connection(connection, table, &relation, key, json)?;
                connection.commit()
            })();
            if mutation.is_err() {
                let _ = connection.rollback();
            }
            let restore = connection.set_autocommit(true);
            match (mutation, restore) {
                (Ok(()), Ok(())) => Ok(document),
                (Err(error), Ok(())) => Err(error),
                (Ok(()), Err(_)) => {
                    connection.discard();
                    Ok(document)
                }
                (Err(error), Err(restore)) => Err(format!(
                    "{error}; restoring ADBC autocommit after resetting obsolete Workflow history also failed: {restore}"
                )),
            }
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
        let json = serde_json::to_string(value)
            .map_err(|error| format!("encoding {table} snapshot: {error}"))?;
        self.repository.run_blocking(move |connection| {
            ensure_dataset_namespace(connection, &namespace)?;
            save_json_document_on_connection(connection, table, &relation, key, json)
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

    pub(crate) async fn datasets(&self) -> Result<Vec<DatasetSummary>, String> {
        let relation = sql::relation(self.repository.namespace(), Table::Datasets);
        self.repository
            .run(move |connection| {
                if !table_exists(connection, &relation)? {
                    return Ok(Vec::new());
                }
                let result = run_query(
                    connection,
                    &format!(
                        "SELECT {id}, {name}, {created}, {description} FROM {relation} ORDER BY {created}, {id}",
                        id = db::DATASET_ID,
                        name = db::NAME,
                        created = db::CREATED_MS,
                        description = db::DESCRIPTION,
                    ),
                    100_000,
                )?;
                if result.truncated {
                    return Err("Spiral dataset catalog exceeds 100,000 rows".to_owned());
                }
                result
                    .rows
                    .into_iter()
                    .enumerate()
                    .map(|(index, row)| {
                        let text = |column| {
                            row.get(column).and_then(JsonValue::as_str).map(str::to_owned)
                        };
                        Ok(DatasetSummary {
                            id: text(db::DATASET_ID).ok_or_else(|| {
                                format!("dataset catalog row {index} has no dataset ID")
                            })?,
                            name: text(db::NAME).ok_or_else(|| {
                                format!("dataset catalog row {index} has no name")
                            })?,
                            created_ms: row
                                .get(db::CREATED_MS)
                                .and_then(JsonValue::as_i64)
                                .ok_or_else(|| {
                                    format!("dataset catalog row {index} has no creation time")
                                })?,
                            description: text(db::DESCRIPTION),
                        })
                    })
                    .collect()
            })
            .await
    }

    pub(crate) async fn workflow_relations(&self) -> Result<WorkflowRelationsResponse, String> {
        let project = self.repository.project_id();
        if project.is_empty() {
            return Ok(WorkflowRelationsResponse {
                scope_id: String::new(),
                relations: Vec::new(),
                complete: false,
                limitation: Some(
                    "Choose a Spiral project in Settings to discover relations.".to_owned(),
                ),
            });
        }
        self.repository
            .run(move |connection| {
                let catalog = connection.list_catalog_relations(&project)?;
                let mut relations = Vec::with_capacity(catalog.len());
                let mut skipped = 0_usize;
                for entry in catalog {
                    if validate_qualified_relation(&entry.name).is_err() {
                        skipped += 1;
                        continue;
                    }
                    let schema = match connection.table_schema(&entry.name) {
                        Ok(Some(schema)) => schema,
                        Ok(None) => {
                            skipped += 1;
                            continue;
                        }
                        Err(error)
                            if error.contains(
                                "extension type cannot be converted to Arrow without a plugin",
                            ) =>
                        {
                            // A project may contain an extension type this
                            // Studio build cannot decode. Discovery remains an
                            // honest usable subset instead of making every
                            // relation unavailable because of one entry.
                            skipped += 1;
                            continue;
                        }
                        Err(error) => {
                            return Err(format!(
                                "describing Spiral relation {:?}: {error}",
                                entry.name
                            ));
                        }
                    };
                    relations.push(WorkflowRelation {
                        name: entry.name,
                        relation_type: entry.relation_type,
                        columns: schema
                            .fields()
                            .iter()
                            .map(|field| QueryColumn {
                                name: field.name().clone(),
                                data_type: field.data_type().to_string(),
                                nullable: field.is_nullable(),
                                semantic_type: field
                                    .metadata()
                                    .get(EXTENSION_TYPE_NAME_KEY)
                                    .cloned(),
                            })
                            .collect(),
                        row_count: None,
                    });
                }
                let limitation = if skipped == 0 {
                    Some(
                        "Row counts are omitted because the Spiral ADBC catalog does not expose bounded relation statistics."
                            .to_owned(),
                    )
                } else {
                    Some(format!(
                        "Row counts are omitted; {skipped} catalog entries without a qualified name or Arrow schema were not returned."
                    ))
                };
                Ok(WorkflowRelationsResponse {
                    scope_id: String::new(),
                    relations,
                    complete: skipped == 0,
                    limitation,
                })
            })
            .await
    }

    /// Describe the active project's user-facing data and add exact row
    /// counts through ordinary read-only SQL. Counts live outside the Workflow
    /// catalog so opening Workflow remains a schema-only operation.
    pub(crate) async fn project_data_relations(
        &self,
    ) -> Result<ProjectDataRelationsResponse, String> {
        let project = self.repository.project_id();
        let namespace = self.repository.namespace().to_owned();
        if project.is_empty() {
            return Ok(ProjectDataRelationsResponse {
                relations: Vec::new(),
                complete: false,
                limitation: Some(
                    "Choose a Spiral project in Settings to discover project data.".to_owned(),
                ),
            });
        }
        self.repository
            .run(move |connection| {
                let catalog = connection.list_catalog_relations(&project)?;
                let mut relations = Vec::with_capacity(catalog.len());
                let mut skipped = 0_usize;
                let mut uncounted = 0_usize;
                for entry in catalog {
                    if !is_user_project_relation(&entry.name, &namespace) {
                        continue;
                    }
                    if validate_qualified_relation(&entry.name).is_err() {
                        skipped += 1;
                        continue;
                    }
                    let schema = match connection.table_schema(&entry.name) {
                        Ok(Some(schema)) => schema,
                        Ok(None) => {
                            skipped += 1;
                            continue;
                        }
                        Err(error)
                            if error.contains(
                                "extension type cannot be converted to Arrow without a plugin",
                            ) =>
                        {
                            skipped += 1;
                            continue;
                        }
                        Err(error) => {
                            return Err(format!(
                                "describing Spiral relation {:?}: {error}",
                                entry.name
                            ));
                        }
                    };
                    let row_count = relation_row_count(connection, &entry.name).map_or_else(
                        |_| {
                            uncounted += 1;
                            None
                        },
                        Some,
                    );
                    relations.push(ProjectDataRelation {
                        name: entry.name,
                        relation_type: entry.relation_type,
                        columns: schema
                            .fields()
                            .iter()
                            .map(|field| QueryColumn {
                                name: field.name().clone(),
                                data_type: field.data_type().to_string(),
                                nullable: field.is_nullable(),
                                semantic_type: field
                                    .metadata()
                                    .get(EXTENSION_TYPE_NAME_KEY)
                                    .cloned(),
                            })
                            .collect(),
                        row_count,
                    });
                }
                let limitation = match (skipped, uncounted) {
                    (0, 0) => None,
                    (skipped, 0) => Some(format!(
                        "{skipped} project relations without a qualified name or readable Arrow schema were omitted."
                    )),
                    (0, uncounted) => Some(format!(
                        "Exact row counts were unavailable for {uncounted} project relations."
                    )),
                    (skipped, uncounted) => Some(format!(
                        "{skipped} project relations were omitted and exact row counts were unavailable for {uncounted} others."
                    )),
                };
                Ok(ProjectDataRelationsResponse {
                    relations,
                    complete: skipped == 0 && uncounted == 0,
                    limitation,
                })
            })
            .await
    }

    /// Resolve the query and its Arrow contract before starting the trainer.
    /// Unknown relations and renamed fields therefore fail the start request,
    /// rather than becoming an asynchronous training-task failure.
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

    /// Read the authoritative MP4 size without materializing its bytes.
    pub(crate) async fn video_asset_size(
        &self,
        episode: &str,
        stream: &str,
        segment: Option<u32>,
    ) -> Result<u64, String> {
        let sql = sql::video_asset_size(self.repository.namespace(), episode, stream, segment);
        self.repository
            .run(move |connection| run_u64_query(connection, &sql, db::BYTE_SIZE))
            .await
    }

    /// Fetch exactly one browser-requested range from Spiral's managed Blob.
    ///
    /// The managed MP4 output retains the source Blob, including its provider
    /// URI and object offset. `spql.io.read_bytes` can therefore push the range
    /// into object storage instead of materializing the complete MP4 in Studio.
    pub(crate) async fn video_asset_range(
        &self,
        episode: &str,
        stream: &str,
        segment: Option<u32>,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>, String> {
        let sql = sql::video_asset_range(
            self.repository.namespace(),
            episode,
            stream,
            segment,
            offset,
            size,
        );
        self.repository
            .run(move |connection| run_binary_query(connection, &sql))
            .await
    }

    pub(crate) async fn video_asset(
        &self,
        episode: &str,
        stream: &str,
        segment: Option<u32>,
    ) -> Result<Vec<u8>, String> {
        let size = self.video_asset_size(episode, stream, segment).await?;
        if size == 0 {
            return Ok(Vec::new());
        }
        self.video_asset_range(episode, stream, segment, 0, size)
            .await
    }
}

fn save_json_document_on_connection(
    connection: &mut RepositoryConnection,
    table: Table,
    relation: &str,
    key: String,
    json: String,
) -> Result<(), String> {
    if table_exists(connection, relation)? {
        execute_delete_sql(
            connection,
            &format!(
                "DELETE FROM {relation} WHERE {} = {}",
                db::RUN_ID,
                sql::string_literal(&key)
            ),
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
    ingest_batch(connection, relation, batch)
}

fn load_json_document_raw_optional_on_connection(
    connection: &mut RepositoryConnection,
    relation: &str,
    key: &str,
) -> Result<Option<String>, String> {
    if !table_exists(connection, relation)? {
        return Ok(None);
    }
    let result = run_query(
        connection,
        &format!(
            "SELECT {} FROM {relation} WHERE {} = {} ORDER BY {} DESC LIMIT 1",
            db::SNAPSHOT_JSON,
            db::RUN_ID,
            sql::string_literal(key),
            db::UPDATED_MS,
        ),
        1,
    )?;
    Ok(result
        .rows
        .first()
        .and_then(|row| row.get(db::SNAPSHOT_JSON))
        .and_then(JsonValue::as_str)
        .map(str::to_owned))
}

fn decode_json_document_or_default<T>(value: &str) -> (T, bool)
where
    T: DeserializeOwned + Default,
{
    match serde_json::from_str(value) {
        Ok(document) => (document, false),
        Err(_) => (T::default(), true),
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
    if !videos.is_empty() {
        ensure_video_assets_blob_relation(connection, namespace)?;
    }
    let had_video = episode_has_video_assets(connection, namespace, &episode.start.id)?;
    let video_rows_changed = had_video || !videos.is_empty();
    let dataset_exists = if video_rows_changed {
        video_dataset_exists(connection, namespace)?
    } else {
        false
    };
    let previous_video_episodes = if video_rows_changed {
        stored_video_episode_ids(connection, namespace)?
    } else {
        BTreeSet::new()
    };
    let has_video_rows = !videos.is_empty()
        || previous_video_episodes
            .iter()
            .any(|episode_id| episode_id != &episode.start.id);
    if !dataset_exists && !previous_video_episodes.is_empty() && has_video_rows {
        retarget_stored_video_media_relations(connection, namespace)?;
    }
    // Replacement is idempotent. Clear any earlier complete row and partial
    // children before writing, so a retry after a process or network failure
    // can finish without duplicating already-landed rows.
    clear_episode_rows_only(connection, namespace, &episode.start.id)?;
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
        let media_relation = sql::video_dataset_output(namespace);
        let batch = video_assets_batch(&episode.start.id, videos, &media_relation)?;
        ingest_batch(
            connection,
            &sql::relation(namespace, Table::VideoAssets),
            batch,
        )?;
    }
    if video_rows_changed {
        if has_video_rows {
            rebuild_video_dataset(connection, namespace, dataset_exists)?;
        } else {
            drop_video_dataset(connection, namespace)?;
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

/// Rebuild the namespace's one managed MP4 index from all durable `video_assets` rows.
///
/// The output is deliberately refreshed even when the Dataset already exists.
/// Project import replaces those source rows in place, so merely observing an
/// existing output does not prove that its video index matches the imported
/// payloads.
pub(crate) fn rebuild_video_dataset(
    connection: &mut RepositoryConnection,
    namespace: &str,
    dataset_exists: bool,
) -> Result<(), String> {
    // One unfiltered view and Dataset cover the whole Studio namespace. The
    // output preserves episode and stream IDs, so consumers select individual
    // media without multiplying catalog resources by episode count.
    let sources = sql::video_sources_relation(namespace);
    let assets = sql::relation(namespace, Table::VideoAssets);
    let dataset = sql::video_dataset_relation(namespace);
    execute_update_sql(connection, &sql::create_video_sources(&sources, &assets))
        .map_err(|error| format!("creating Spiral video source view: {error}"))?;
    if !dataset_exists {
        execute_update_sql(connection, &sql::create_video_dataset(&dataset, &sources))
            .map_err(|error| format!("creating Spiral MP4 dataset: {error}"))?;
    }
    execute_update_sql(connection, &sql::refresh_dataset(&dataset))
        .map_err(|error| format!("refreshing Spiral MP4 dataset: {error}"))
}

pub(crate) fn video_dataset_exists(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<bool, String> {
    let dataset = sql::video_dataset_relation(namespace);
    match run_query(connection, &format!("DESCRIBE {dataset}"), 1) {
        Ok(_) => Ok(true),
        Err(error)
            if error.contains("unknown resource")
                || error.contains("does not exist")
                || error.contains("unknown relation") =>
        {
            Ok(false)
        }
        Err(error) => Err(format!("checking Spiral MP4 dataset: {error}")),
    }
}

pub(crate) fn drop_video_dataset(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<(), String> {
    let dataset = sql::video_dataset_relation(namespace);
    validate_qualified_relation(&dataset)
        .map_err(|error| format!("refusing to drop an invalid Spiral MP4 Dataset: {error}"))?;
    execute_update_sql(connection, &sql::drop_video_dataset(&dataset))
        .map_err(|error| format!("dropping Spiral MP4 dataset with CASCADE: {error}"))?;
    execute_update_sql(
        connection,
        &sql::drop_video_sources(&sql::video_sources_relation(namespace)),
    )
    .map_err(|error| format!("dropping Spiral video source view: {error}"))
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
    let previous_video_episodes = stored_video_episode_ids(connection, namespace)?;
    let had_video = previous_video_episodes.contains(episode_id);
    let dataset_exists = if had_video {
        video_dataset_exists(connection, namespace)?
    } else {
        false
    };
    if had_video && !dataset_exists && previous_video_episodes.len() > 1 {
        retarget_stored_video_media_relations(connection, namespace)?;
    }
    clear_episode_rows_only(connection, namespace, episode_id)?;
    if !had_video {
        return Ok(());
    }
    if previous_video_episodes.len() == 1 {
        drop_video_dataset(connection, namespace)?;
    } else {
        rebuild_video_dataset(connection, namespace, dataset_exists)?;
    }
    Ok(())
}

fn clear_episode_rows_only(
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

pub(crate) fn stored_video_episode_ids(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<BTreeSet<String>, String> {
    const MAX_VIDEO_EPISODES: usize = 100_000;

    let assets = sql::relation(namespace, Table::VideoAssets);
    if !table_exists(connection, &assets)? {
        return Ok(BTreeSet::new());
    }
    let result = run_query(
        connection,
        &format!(
            "SELECT DISTINCT {episode_id} FROM {assets}",
            episode_id = db::EPISODE_ID
        ),
        MAX_VIDEO_EPISODES,
    )?;
    if result.truncated {
        return Err(format!(
            "project has more than {MAX_VIDEO_EPISODES} video episodes; refusing incomplete video-resource reconciliation"
        ));
    }
    result
        .rows
        .into_iter()
        .map(|row| {
            row.get(db::EPISODE_ID)
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "stored video asset has no valid episode ID".to_owned())
        })
        .collect()
}

fn episode_has_video_assets(
    connection: &mut RepositoryConnection,
    namespace: &str,
    episode_id: &str,
) -> Result<bool, String> {
    let assets = sql::relation(namespace, Table::VideoAssets);
    if !table_exists(connection, &assets)? {
        return Ok(false);
    }
    let result = run_query(
        connection,
        &format!(
            "SELECT {episode_column} FROM {assets} WHERE {episode_column} = {episode_id} LIMIT 1",
            episode_column = db::EPISODE_ID,
            episode_id = sql::string_literal(episode_id),
        ),
        1,
    )?;
    Ok(!result.rows.is_empty())
}

pub(crate) fn retarget_stored_video_media_relations(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<(), String> {
    let assets = sql::relation(namespace, Table::VideoAssets);
    let output = sql::video_dataset_output(namespace);
    execute_update_sql(
        connection,
        &format!(
            "UPDATE {assets} SET {media_relation} = {output}",
            media_relation = db::MEDIA_RELATION,
            output = sql::string_literal(&output),
        ),
    )
    .map_err(|error| format!("retargeting stored video media relations: {error}"))
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
    ingest_batches(connection, target, vec![batch])
}

pub(crate) fn ingest_batches(
    connection: &mut RepositoryConnection,
    target: &str,
    batches: Vec<RecordBatch>,
) -> Result<(), String> {
    write_batches(connection, target, batches, false)
}

/// Atomically replace one complete Arbor relation from a single Arrow stream.
///
/// Whole-project restore owns the complete incoming relation and therefore uses
/// hosted Arbor's replacement path. Incremental recording and episode import
/// deliberately continue through [`ingest_batches`].
pub(crate) fn replace_batches(
    connection: &mut RepositoryConnection,
    target: &str,
    batches: Vec<RecordBatch>,
) -> Result<(), String> {
    write_batches(connection, target, batches, true)
}

fn write_batches(
    connection: &mut RepositoryConnection,
    target: &str,
    batches: Vec<RecordBatch>,
    replace: bool,
) -> Result<(), String> {
    let Some(schema) = batches.first().map(RecordBatch::schema) else {
        return Ok(());
    };
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening ADBC ingest for {target}: {error}"))?;
    if replace {
        statement
            .configure_replace(target)
            .map_err(|error| format!("configuring ADBC replacement for {target}: {error}"))?;
    } else {
        statement
            .configure_ingest(target)
            .map_err(|error| format!("configuring ADBC ingest for {target}: {error}"))?;
    }
    statement
        .bind_stream(Box::new(RecordBatchIterator::new(
            batches.into_iter().map(Ok::<_, ArrowError>),
            schema,
        )))
        .map_err(|error| format!("binding Spiral table {target}: {error}"))?;
    statement.execute_update().map_err(|error| {
        if replace {
            format!("replacing Spiral table {target}: {error}")
        } else {
            format!("ingesting Spiral table {target}: {error}")
        }
    })?;
    Ok(())
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

#[cfg(test)]
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
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Finish a Spiral transaction, rebasing its optimistic project mutation when
/// a concurrently published managed artifact advances the project head.
pub(crate) fn commit_catalog_transaction(
    connection: &mut RepositoryConnection,
) -> Result<(), String> {
    const MAX_COMMIT_ATTEMPTS: usize = 16;

    for attempt in 1..=MAX_COMMIT_ATTEMPTS {
        match connection.commit() {
            Ok(()) => return Ok(()),
            Err(error)
                if error.contains("project namespace compare-and-swap conflict")
                    && attempt < MAX_COMMIT_ATTEMPTS =>
            {
                std::thread::yield_now();
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the bounded commit loop returns on its final attempt")
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

pub(crate) fn video_assets_batch(
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
        inline_video_blob_array(&payloads)?,
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
        video_blob_field(db::PAYLOAD),
        Field::new(db::BYTE_SIZE, DataType::UInt64, false),
        Field::new(db::WIDTH, DataType::UInt32, false),
        Field::new(db::HEIGHT, DataType::UInt32, false),
        Field::new(db::FPS, DataType::UInt32, false),
        Field::new(db::FRAMES, DataType::UInt64, false),
        Field::new(db::MEDIA_RELATION, DataType::Utf8, false),
    ]))
}

fn video_blob_storage_fields() -> Fields {
    vec![
        Field::new("bytes", DataType::BinaryView, true),
        Field::new("uri", DataType::Utf8View, true),
        Field::new("offset", DataType::UInt64, true),
        Field::new("size", DataType::UInt64, true),
        Field::new("mime_type", DataType::Utf8View, true),
        Field::new("etag", DataType::Utf8View, true),
        Field::new("version_id", DataType::Utf8View, true),
        Field::new("generation", DataType::Utf8View, true),
        Field::new("md5", DataType::Utf8View, true),
        Field::new("sha1", DataType::Utf8View, true),
        Field::new("sha256", DataType::Utf8View, true),
        Field::new("manifest_digest", DataType::Utf8View, true),
    ]
    .into()
}

fn video_blob_field(name: &str) -> Field {
    Field::new(name, DataType::Struct(video_blob_storage_fields()), true).with_metadata(
        [(
            EXTENSION_TYPE_NAME_KEY.to_owned(),
            BLOB_EXTENSION_NAME.to_owned(),
        )]
        .into(),
    )
}

fn inline_video_blob_array(payloads: &[&[u8]]) -> Result<ArrayRef, String> {
    let len = payloads.len();
    let fields = video_blob_storage_fields();
    let null_text = || Arc::new(StringViewArray::from(vec![None::<&str>; len])) as ArrayRef;
    let null_u64 = || Arc::new(UInt64Array::from(vec![None::<u64>; len])) as ArrayRef;
    let sha256 = Arc::new(StringViewArray::from_iter_values(payloads.iter().map(
        |payload| {
            let digest = Sha256::digest(payload);
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        },
    ))) as ArrayRef;
    let values = vec![
        Arc::new(BinaryViewArray::from_iter_values(payloads.iter().copied())) as ArrayRef,
        null_text(),
        null_u64(),
        Arc::new(UInt64Array::from_iter_values(
            payloads.iter().map(|payload| payload.len() as u64),
        )) as ArrayRef,
        Arc::new(StringViewArray::from(vec![Some("video/mp4"); len])) as ArrayRef,
        null_text(),
        null_text(),
        null_text(),
        null_text(),
        null_text(),
        sha256,
        null_text(),
    ];
    StructArray::try_new(fields, values, None)
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|error| format!("building Spiral video Blob array: {error}"))
}

pub(crate) fn normalize_video_asset_batch(batch: RecordBatch) -> Result<RecordBatch, String> {
    let payload_index = batch
        .schema()
        .index_of(db::PAYLOAD)
        .map_err(|error| format!("video asset batch is missing {}: {error}", db::PAYLOAD))?;
    let batch_schema = batch.schema();
    let payload_field = batch_schema.field(payload_index);
    if payload_field
        .metadata()
        .get(EXTENSION_TYPE_NAME_KEY)
        .is_some_and(|name| name == BLOB_EXTENSION_NAME)
    {
        return Ok(batch);
    }
    let payload = batch.column(payload_index);
    let values = if let Some(payload) = payload.as_any().downcast_ref::<BinaryArray>() {
        payload.iter().collect::<Vec<_>>()
    } else if let Some(payload) = payload.as_any().downcast_ref::<LargeBinaryArray>() {
        payload.iter().collect::<Vec<_>>()
    } else if let Some(payload) = payload.as_any().downcast_ref::<BinaryViewArray>() {
        payload.iter().collect::<Vec<_>>()
    } else {
        return Err(format!(
            "video asset {} must be a Spiral Blob or Arrow binary, got {}",
            db::PAYLOAD,
            payload.data_type(),
        ));
    };
    if values.iter().any(Option::is_none) {
        return Err("video asset payload cannot be null".to_owned());
    }
    let payloads = values.into_iter().flatten().collect::<Vec<_>>();
    let mut fields = batch
        .schema()
        .fields()
        .iter()
        .map(|field| field.as_ref().clone())
        .collect::<Vec<_>>();
    fields[payload_index] = video_blob_field(db::PAYLOAD);
    let schema = Arc::new(Schema::new_with_metadata(
        fields,
        batch.schema().metadata().clone(),
    ));
    let mut columns = batch.columns().to_vec();
    columns[payload_index] = inline_video_blob_array(&payloads)?;
    RecordBatch::try_new(schema, columns)
        .map_err(|error| format!("normalizing video asset Blob batch: {error}"))
}

pub(crate) fn ensure_video_assets_blob_relation(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<(), String> {
    let assets = sql::relation(namespace, Table::VideoAssets);
    let Some(schema) = connection.table_schema(&assets)? else {
        return Ok(());
    };
    let payload = schema
        .field_with_name(db::PAYLOAD)
        .map_err(|error| format!("stored video assets are missing {}: {error}", db::PAYLOAD))?;
    if payload
        .metadata()
        .get(EXTENSION_TYPE_NAME_KEY)
        .is_some_and(|name| name == BLOB_EXTENSION_NAME)
    {
        return Ok(());
    }
    if !matches!(
        payload.data_type(),
        DataType::Binary | DataType::LargeBinary | DataType::BinaryView
    ) {
        return Err(format!(
            "stored video asset {} must be a Spiral Blob or Arrow binary, got {}",
            db::PAYLOAD,
            payload.data_type(),
        ));
    }
    execute_update_sql(connection, &sql::migrate_video_assets_to_blob(&assets))
        .map_err(|error| format!("migrating video assets from Binary to Blob: {error}"))
}

pub(crate) fn run_query(
    connection: &mut RepositoryConnection,
    sql: &str,
    max_rows: usize,
) -> Result<SpiralQueryResponse, String> {
    run_query_with_byte_limit(connection, sql, max_rows, MAX_INTERNAL_QUERY_RESPONSE_BYTES)
}

fn relation_row_count(
    connection: &mut RepositoryConnection,
    relation: &str,
) -> Result<u64, String> {
    let result = run_query(connection, &sql::count_rows(relation), 1)?;
    let value = result
        .rows
        .first()
        .and_then(|row| row.get("rows"))
        .ok_or_else(|| format!("Spiral returned no row count for {relation}"))?;
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|count| count.try_into().ok()))
        .ok_or_else(|| format!("Spiral returned a non-integer row count for {relation}"))
}

fn is_user_project_relation(relation: &str, namespace: &str) -> bool {
    if relation == namespace
        || relation == sql::video_sources_relation(namespace)
        || relation == sql::video_dataset_relation(namespace)
        || relation == sql::video_dataset_output(namespace)
        || relation.starts_with("workflow.")
    {
        return false;
    }
    ![
        Table::Robots,
        Table::StudioSettings,
        Table::TrainingCheckpoints,
        Table::TrainingRuns,
        Table::RolloutRuns,
        Table::WorkflowRuns,
    ]
    .into_iter()
    .any(|table| relation == spiral::relation(namespace, table))
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
    let schema = reader.schema();
    let columns = schema
        .fields()
        .iter()
        .map(|field| QueryColumn {
            name: field.name().clone(),
            data_type: field.data_type().to_string(),
            nullable: field.is_nullable(),
            semantic_type: field.metadata().get(EXTENSION_TYPE_NAME_KEY).cloned(),
        })
        .collect();
    let chunk_rows = if schema.fields().iter().any(|field| {
        matches!(
            field
                .metadata()
                .get(EXTENSION_TYPE_NAME_KEY)
                .map(String::as_str),
            Some(IMAGE_EXTENSION_NAME | VARIABLE_TENSOR_EXTENSION_NAME)
        )
    }) {
        1
    } else {
        QUERY_JSON_CHUNK_ROWS
    };
    let fetch = max_rows.saturating_add(1);
    let mut rows = Vec::with_capacity(max_rows.min(QUERY_JSON_CHUNK_ROWS));
    let mut encoded_bytes = 2_usize;
    let mut inspected = 0_usize;
    let mut truncated = false;
    'result: for batch in &mut reader {
        let batch = batch.map_err(|error| format!("reading Spiral result: {error}"))?;
        let mut offset = 0_usize;
        while offset < batch.num_rows() && inspected < fetch {
            let count = chunk_rows
                .min(batch.num_rows() - offset)
                .min(fetch - inspected);
            let chunk = batch.slice(offset, count);
            let preview = query_preview_batch(&chunk)?;
            for row in batch_to_json(&preview)? {
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

/// Replace displayable image extension columns with bounded browser-ready
/// data URLs before Arrow's JSON writer expands every pixel into a number.
/// The response schema is captured from the original batch, so callers still
/// receive the real Arrow physical and semantic types.
fn query_preview_batch(batch: &RecordBatch) -> Result<RecordBatch, String> {
    let mut fields = Vec::with_capacity(batch.num_columns());
    let mut columns = Vec::with_capacity(batch.num_columns());
    let mut changed = false;
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        let semantic_type = field
            .metadata()
            .get(EXTENSION_TYPE_NAME_KEY)
            .map(String::as_str);
        let preview = match semantic_type {
            Some(IMAGE_EXTENSION_NAME) => Some(image_preview_array(column.as_ref())?),
            Some(VARIABLE_TENSOR_EXTENSION_NAME) => tensor_preview_array(column.as_ref())?,
            _ => None,
        };
        if let Some(preview) = preview {
            fields.push(Field::new(
                field.name(),
                DataType::Utf8,
                field.is_nullable(),
            ));
            columns.push(preview);
            changed = true;
        } else {
            fields.push(field.as_ref().clone());
            columns.push(column.clone());
        }
    }
    if !changed {
        return Ok(batch.clone());
    }
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        )),
        columns,
    )
    .map_err(|error| format!("building Spiral image previews: {error}"))
}

fn image_preview_array(array: &dyn arrow_array::Array) -> Result<ArrayRef, String> {
    let values = array
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| "Spiral Image column did not use struct storage".to_owned())?;
    let data = values
        .column_by_name("data")
        .ok_or_else(|| "Spiral Image column has no data field".to_owned())?;
    let widths = values
        .column_by_name("width")
        .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
        .ok_or_else(|| "Spiral Image column has no UInt32 width field".to_owned())?;
    let heights = values
        .column_by_name("height")
        .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
        .ok_or_else(|| "Spiral Image column has no UInt32 height field".to_owned())?;
    let formats = values
        .column_by_name("pixel_format")
        .and_then(|column| column.as_any().downcast_ref::<UInt8Array>())
        .ok_or_else(|| "Spiral Image column has no UInt8 pixel_format field".to_owned())?;
    let mut previews = Vec::with_capacity(values.len());
    for row in 0..values.len() {
        if values.is_null(row) {
            previews.push(None);
            continue;
        }
        if widths.is_null(row) || heights.is_null(row) || formats.is_null(row) {
            return Err("Spiral Image dimensions and pixel format cannot be null".to_owned());
        }
        let pixels = list_u8_value(data.as_ref(), row)
            .ok_or_else(|| "Spiral Image data must be a List<UInt8>".to_owned())?;
        previews.push(Some(encode_query_image(
            &pixels,
            widths.value(row),
            heights.value(row),
            formats.value(row),
        )?));
    }
    Ok(Arc::new(StringArray::from(previews)))
}

/// Render only the canonical HWC UInt8 tensor shape used by
/// `spql.image.to_tensor(image, 'hwc', 'none')`. Other tensors retain their
/// ordinary JSON representation instead of being mislabeled as imagery.
fn tensor_preview_array(array: &dyn arrow_array::Array) -> Result<Option<ArrayRef>, String> {
    let Some(values) = array.as_any().downcast_ref::<StructArray>() else {
        return Ok(None);
    };
    let Some(data) = values.column_by_name("data") else {
        return Ok(None);
    };
    let Some(shapes) = values
        .column_by_name("shape")
        .and_then(|column| column.as_any().downcast_ref::<FixedSizeListArray>())
    else {
        return Ok(None);
    };
    if shapes.value_length() != 3 {
        return Ok(None);
    }
    let mut previews = Vec::with_capacity(values.len());
    for row in 0..values.len() {
        if values.is_null(row) {
            previews.push(None);
            continue;
        }
        let shape_value = shapes.value(row);
        let Some(shape) = shape_value.as_any().downcast_ref::<Int32Array>() else {
            return Ok(None);
        };
        let [height, width, channels] = shape.values().as_ref() else {
            return Ok(None);
        };
        if *height <= 0 || *width <= 0 || !matches!(*channels, 3 | 4) {
            return Ok(None);
        }
        let Some(pixels) = list_u8_value(data.as_ref(), row) else {
            return Ok(None);
        };
        previews.push(Some(encode_query_image(
            &pixels,
            u32::try_from(*width).map_err(|_| "Spiral image tensor width overflowed u32")?,
            u32::try_from(*height).map_err(|_| "Spiral image tensor height overflowed u32")?,
            if *channels == 3 { 1 } else { 2 },
        )?));
    }
    Ok(Some(Arc::new(StringArray::from(previews))))
}

fn list_u8_value(array: &dyn arrow_array::Array, row: usize) -> Option<Vec<u8>> {
    if array.is_null(row) {
        return None;
    }
    let value = if let Some(values) = array.as_any().downcast_ref::<ListArray>() {
        values.value(row)
    } else {
        let values = array.as_any().downcast_ref::<LargeListArray>()?;
        values.value(row)
    };
    let values = value.as_any().downcast_ref::<UInt8Array>()?;
    (values.null_count() == 0).then(|| values.values().to_vec())
}

fn encode_query_image(
    pixels: &[u8],
    width: u32,
    height: u32,
    pixel_format: u8,
) -> Result<String, String> {
    if width == 0 || height == 0 {
        return Err("Spiral Image dimensions must be positive".to_owned());
    }
    let channels = match pixel_format {
        1 => 3_usize,
        2 => 4_usize,
        value => {
            return Err(format!(
                "Spiral Image uses unsupported pixel format {value}"
            ));
        }
    };
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "Spiral Image dimensions overflowed the host".to_owned())?;
    if pixels.len() != expected {
        return Err(format!(
            "Spiral Image declares {width}×{height}×{channels} but contains {} values",
            pixels.len()
        ));
    }
    let image = match pixel_format {
        1 => image::RgbImage::from_raw(width, height, pixels.to_vec()).map(DynamicImage::ImageRgb8),
        2 => {
            image::RgbaImage::from_raw(width, height, pixels.to_vec()).map(DynamicImage::ImageRgba8)
        }
        _ => unreachable!(),
    }
    .ok_or_else(|| "Spiral Image pixels did not match their dimensions".to_owned())?;
    let preview = if width > QUERY_IMAGE_PREVIEW_MAX_EDGE || height > QUERY_IMAGE_PREVIEW_MAX_EDGE {
        image.thumbnail(QUERY_IMAGE_PREVIEW_MAX_EDGE, QUERY_IMAGE_PREVIEW_MAX_EDGE)
    } else {
        image
    };
    let (mime_type, encoded) = if pixel_format == 1 {
        let rgb = preview.to_rgb8();
        let mut encoded = Vec::new();
        JpegEncoder::new_with_quality(&mut encoded, QUERY_IMAGE_JPEG_QUALITY)
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|error| format!("encoding Spiral Image preview as JPEG: {error}"))?;
        ("image/jpeg", encoded)
    } else {
        let mut encoded = Cursor::new(Vec::new());
        preview
            .write_to(&mut encoded, ImageFormat::Png)
            .map_err(|error| format!("encoding Spiral Image preview as PNG: {error}"))?;
        ("image/png", encoded.into_inner())
    };
    let payload = base64::engine::general_purpose::STANDARD.encode(encoded);
    Ok(format!("data:{mime_type};base64,{payload}"))
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

pub(crate) fn run_binary_query(
    connection: &mut RepositoryConnection,
    sql: &str,
) -> Result<Vec<u8>, String> {
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

fn run_u64_query(
    connection: &mut RepositoryConnection,
    sql: &str,
    column: &str,
) -> Result<u64, String> {
    let mut statement = connection
        .new_statement()
        .map_err(|error| format!("opening Spiral scalar query: {error}"))?;
    statement
        .set_sql_query(sql)
        .map_err(|error| format!("binding Spiral scalar query: {error}"))?;
    let reader = statement
        .execute()
        .map_err(|error| format!("executing Spiral scalar query: {error}"))?;
    for batch in reader {
        let batch = batch.map_err(|error| format!("reading Spiral scalar result: {error}"))?;
        if batch.num_rows() == 0 {
            continue;
        }
        let values = batch
            .column_by_name(column)
            .and_then(|array| array.as_any().downcast_ref::<UInt64Array>())
            .ok_or_else(|| format!("Spiral scalar query returned no UInt64 {column} column"))?;
        if values.is_null(0) {
            return Err(format!("Spiral scalar query returned null {column}"));
        }
        return Ok(values.value(0));
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
    use std::collections::HashMap;

    use arrow_array::builder::{FixedSizeListBuilder, Int32Builder, ListBuilder, UInt8Builder};

    #[derive(Debug, Default, PartialEq, serde::Deserialize)]
    struct CurrentPrototypeDocument {
        features: Vec<String>,
    }

    #[test]
    fn project_data_catalog_hides_studio_plumbing_but_keeps_external_relations() {
        let namespace = "robotics";
        for table in [
            Table::Robots,
            Table::StudioSettings,
            Table::TrainingCheckpoints,
            Table::TrainingRuns,
            Table::RolloutRuns,
            Table::WorkflowRuns,
        ] {
            assert!(!is_user_project_relation(
                &spiral::relation(namespace, table),
                namespace
            ));
        }
        assert!(!is_user_project_relation(
            "workflow.feature_registry",
            namespace
        ));
        assert!(!is_user_project_relation(
            "robotics.video.assets",
            namespace
        ));
        assert!(is_user_project_relation("robotics.episodes", namespace));
        assert!(is_user_project_relation("customer.annotations", namespace));
    }

    #[test]
    fn obsolete_workflow_json_resets_once_instead_of_poisoning_bootstrap() {
        let (document, incompatible) =
            decode_json_document_or_default::<CurrentPrototypeDocument>(r#"{"concepts":["cube"]}"#);
        assert!(incompatible);
        assert_eq!(document, CurrentPrototypeDocument::default());

        let (document, incompatible) = decode_json_document_or_default::<CurrentPrototypeDocument>(
            r#"{"features":["is_cube_feature"]}"#,
        );
        assert!(!incompatible);
        assert_eq!(document.features, ["is_cube_feature"]);
    }

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
    fn inline_video_blobs_store_content_sha256_in_the_named_field() {
        let payload = b"spiral";
        let array = inline_video_blob_array(&[payload.as_slice()]).unwrap();
        let blob = array.as_any().downcast_ref::<StructArray>().unwrap();
        let sha256 = blob
            .column_by_name("sha256")
            .unwrap()
            .as_any()
            .downcast_ref::<StringViewArray>()
            .unwrap();
        let expected = Sha256::digest(payload)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(sha256.value(0), expected);

        let generation = blob
            .column_by_name("generation")
            .unwrap()
            .as_any()
            .downcast_ref::<StringViewArray>()
            .unwrap();
        assert!(generation.is_null(0));
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
    fn decoded_spiral_images_become_bounded_browser_previews() {
        let mut data = ListBuilder::new(UInt8Builder::new());
        for value in [255, 0, 0, 0, 255, 0] {
            data.values().append_value(value);
        }
        data.append(true);
        let data = Arc::new(data.finish()) as ArrayRef;
        let widths = Arc::new(UInt32Array::from(vec![2])) as ArrayRef;
        let heights = Arc::new(UInt32Array::from(vec![1])) as ArrayRef;
        let formats = Arc::new(UInt8Array::from(vec![1])) as ArrayRef;
        let storage_fields = Fields::from(vec![
            Arc::new(Field::new("data", data.data_type().clone(), false)),
            Arc::new(Field::new("width", DataType::UInt32, false)),
            Arc::new(Field::new("height", DataType::UInt32, false)),
            Arc::new(Field::new("pixel_format", DataType::UInt8, false)),
        ]);
        let images = Arc::new(StructArray::new(
            storage_fields,
            vec![data, widths, heights, formats],
            None,
        )) as ArrayRef;
        let image_field =
            Field::new("image", images.data_type().clone(), false).with_metadata(HashMap::from([
                (
                    EXTENSION_TYPE_NAME_KEY.to_owned(),
                    IMAGE_EXTENSION_NAME.to_owned(),
                ),
            ]));
        let batch =
            RecordBatch::try_new(Arc::new(Schema::new(vec![image_field])), vec![images]).unwrap();

        let preview = query_preview_batch(&batch).unwrap();
        let rows = batch_to_json(&preview).unwrap();
        let value = rows[0].get("image").and_then(JsonValue::as_str).unwrap();
        assert!(value.starts_with("data:image/jpeg;base64,"));
        let encoded = value.split_once(',').unwrap().1;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 2);
        assert!(value.len() < 2_048, "tiny preview unexpectedly expanded");
    }

    #[test]
    fn hwc_u8_arrow_tensors_use_the_same_image_preview_path() {
        let mut data = ListBuilder::new(UInt8Builder::new());
        for value in [255, 0, 0, 0, 255, 0] {
            data.values().append_value(value);
        }
        data.append(true);
        let data = Arc::new(data.finish()) as ArrayRef;
        let mut shapes = FixedSizeListBuilder::new(Int32Builder::new(), 3);
        for value in [1, 2, 3] {
            shapes.values().append_value(value);
        }
        shapes.append(true);
        let shapes = Arc::new(shapes.finish()) as ArrayRef;
        let storage_fields = Fields::from(vec![
            Arc::new(Field::new("data", data.data_type().clone(), false)),
            Arc::new(Field::new("shape", shapes.data_type().clone(), false)),
        ]);
        let tensors =
            Arc::new(StructArray::new(storage_fields, vec![data, shapes], None)) as ArrayRef;
        let tensor_field =
            Field::new("image", tensors.data_type().clone(), false).with_metadata(HashMap::from([
                (
                    EXTENSION_TYPE_NAME_KEY.to_owned(),
                    VARIABLE_TENSOR_EXTENSION_NAME.to_owned(),
                ),
            ]));
        let batch =
            RecordBatch::try_new(Arc::new(Schema::new(vec![tensor_field])), vec![tensors]).unwrap();

        let rows = batch_to_json(&query_preview_batch(&batch).unwrap()).unwrap();
        assert!(
            rows[0]
                .get("image")
                .and_then(JsonValue::as_str)
                .is_some_and(|value| value.starts_with("data:image/jpeg;base64,"))
        );
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
