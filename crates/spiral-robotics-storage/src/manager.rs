//! SpiralDB persistence, query preview, and remote replay for Studio data.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::schema::{self as spiral, Table, column as db};
use arrow_array::{
    Array as _, ArrayRef, BinaryArray, BinaryViewArray, FixedSizeListArray, Int32Array, Int64Array,
    LargeBinaryArray, LargeListArray, LargeStringArray, ListArray, RecordBatch, StringArray,
    StringViewArray, StructArray, UInt8Array, UInt32Array, UInt64Array,
};
use arrow_json::writer::{JsonArray, WriterBuilder};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Fields, Schema};
use base64::Engine as _;
use futures::StreamExt as _;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map as JsonMap, Value as JsonValue};
use sha2::{Digest as _, Sha256};
use spiral_robotics_api::recording::DatasetSummary;
use spiral_robotics_api::relations::{CatalogRelation, RelationCatalog};
use spiral_robotics_api::relations::{
    ProjectDataRelation, ProjectDataRelationsResponse, QueryColumn, SpiralQueryResponse,
};
use spiral_robotics_api::settings::{ConnectionTestResponse, DataSettings, SpiralSettings};
use spiral_robotics_api::sql::QualifiedRelation;

use crate::bundle;
use crate::checkpoints::{CheckpointArtifact, CheckpointFuture, CheckpointKey, CheckpointStore};
use crate::database::{DatabaseCommand, DatabaseSession, RelationKind, RelationName};
use crate::episode_schema::{
    JointSampleRows, SpiralEpisodeEncoder, VideoStreamProperties, VideoStreamRow, VideoStreamRows,
};
use crate::episodes::FullEpisode;
use crate::project_store::{ProjectConnection, ProjectStore};
use crate::settings::{SpiralConnectionSettings, StoredStudioPreferences};
use crate::sql::{ReadOnlyQueryMode, SpiralSql};
use crate::video::{VideoAsset, VideoAssetKey, VideoByteRange};

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

#[derive(Clone, Copy)]
struct QueryLimits {
    rows: usize,
    bytes: usize,
}

impl QueryLimits {
    const fn new(rows: usize, bytes: usize) -> Self {
        Self { rows, bytes }
    }
}

struct QueryExecution<'a> {
    sql: &'a str,
    limits: QueryLimits,
}

impl<'a> QueryExecution<'a> {
    const fn new(sql: &'a str, limits: QueryLimits) -> Self {
        Self { sql, limits }
    }
}

struct QueryRows {
    values: Vec<JsonMap<String, JsonValue>>,
    encoded_bytes: usize,
    max_bytes: usize,
}

#[derive(Clone, Copy)]
struct ImageDimensions {
    width: u32,
    height: u32,
}

impl ImageDimensions {
    const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

struct QueryImage<'a> {
    pixels: &'a [u8],
    dimensions: ImageDimensions,
    pixel_format: u8,
}

impl<'a> QueryImage<'a> {
    const fn new(pixels: &'a [u8], dimensions: ImageDimensions, pixel_format: u8) -> Self {
        Self {
            pixels,
            dimensions,
            pixel_format,
        }
    }
}

impl QueryRows {
    fn with_capacity(row_capacity: usize, max_bytes: usize) -> Self {
        Self {
            values: Vec::with_capacity(row_capacity),
            encoded_bytes: 2,
            max_bytes,
        }
    }

    fn push(&mut self, row: JsonMap<String, JsonValue>) -> Result<bool, String> {
        let row_bytes = serde_json::to_vec(&row)
            .map_err(|error| format!("measuring Spiral row: {error}"))?
            .len()
            .saturating_add(usize::from(!self.values.is_empty()));
        if self.encoded_bytes.saturating_add(row_bytes) > self.max_bytes {
            return Ok(false);
        }
        self.encoded_bytes = self.encoded_bytes.saturating_add(row_bytes);
        self.values.push(row);
        Ok(true)
    }

    fn into_values(self) -> Vec<JsonMap<String, JsonValue>> {
        self.values
    }
}

#[derive(Clone, Copy)]
struct JsonDocumentKey<'a> {
    table: Table,
    key: &'a str,
}

impl<'a> JsonDocumentKey<'a> {
    const fn new(table: Table, key: &'a str) -> Self {
        Self { table, key }
    }
}

struct JsonDocumentTarget {
    table: Table,
    relation: String,
    key: String,
}

impl JsonDocumentTarget {
    fn new(table: Table, relation: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            table,
            relation: relation.into(),
            key: key.into(),
        }
    }
}

struct JsonDocumentWrite {
    target: JsonDocumentTarget,
    json: String,
}

impl JsonDocumentWrite {
    fn new(target: JsonDocumentTarget, json: String) -> Self {
        Self { target, json }
    }

    async fn save(self, connection: &mut ProjectConnection) -> Result<(), String> {
        let Self { target, json } = self;
        if connection.table_exists(&target.relation).await? {
            connection
                .execute_delete_sql(&format!(
                    "DELETE FROM {} WHERE {} = {}",
                    target.relation,
                    db::RUN_ID,
                    SpiralSql::string_literal(&target.key)
                ))
                .await?;
        }
        let schema = Arc::new(Schema::new(vec![
            Field::new(db::RUN_ID, DataType::Utf8, false),
            Field::new(db::SNAPSHOT_JSON, DataType::Utf8, false),
            Field::new(db::UPDATED_MS, DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![target.key])) as ArrayRef,
                Arc::new(StringArray::from(vec![json])) as ArrayRef,
                Arc::new(Int64Array::from(vec![StorageClock::unix_millis()])) as ArrayRef,
            ],
        )
        .map_err(|error| format!("building {} snapshot batch: {error}", target.table))?;
        connection.ingest_batch(&target.relation, batch).await
    }
}

pub struct StorageManager {
    project_store: Arc<ProjectStore>,
}

impl StorageManager {
    pub fn new(project_store: Arc<ProjectStore>) -> Self {
        Self { project_store }
    }

    pub(crate) fn project_store(&self) -> &ProjectStore {
        &self.project_store
    }

    pub fn data_settings(&self, preferences: &StoredStudioPreferences) -> DataSettings {
        let credentials = self.project_store.credentials();
        let spiral = SpiralSettings::new(
            self.project_store.endpoint(),
            self.project_store.spfs_endpoint(),
            self.project_store.project_id(),
        )
        .with_client(
            credentials
                .as_ref()
                .map(|credentials| credentials.client_id().to_owned())
                .unwrap_or_default(),
            credentials.is_some(),
        )
        .with_namespace(self.project_store.namespace());
        DataSettings::new(
            spiral,
            preferences.training().clone(),
            preferences.video().clone(),
        )
    }

    pub fn project_id(&self) -> String {
        self.project_store.project_id()
    }

    pub fn connection_settings(&self) -> SpiralConnectionSettings {
        self.project_store.connection_settings()
    }

    pub fn workflow_model_store(&self) -> Arc<ProjectStore> {
        self.project_store.clone()
    }

    pub fn spiral_endpoint(&self) -> String {
        self.project_store.endpoint()
    }

    pub fn spiral_spfs_endpoint(&self) -> String {
        self.project_store.spfs_endpoint()
    }

    pub fn spiral_credentials(&self) -> Option<crate::settings::SpiralCredentials> {
        self.project_store.credentials()
    }

    pub fn is_configured(&self) -> bool {
        self.project_store.is_configured()
    }

    pub async fn prewarm(&self) -> Result<(), String> {
        self.project_store.prewarm().await?;
        self.ensure_async_video_assets_blob_relation().await
    }

    async fn ensure_async_video_assets_blob_relation(&self) -> Result<(), String> {
        let assets = Table::VideoAssets.qualified_name(self.project_store.namespace());
        let target = RelationName::parse(&assets).map_err(|error| error.to_string())?;
        let mut connection = self.project_store.connect().await?;
        let Some(schema) = connection
            .table_schema(target)
            .await
            .map_err(|error| error.to_string())?
        else {
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
        connection
            .execute(DatabaseCommand::statement(
                SpiralSql::migrate_video_assets_to_blob(&assets),
            ))
            .await
            .and_then(crate::DatabaseOutput::into_summary)
            .map(|_| ())
            .map_err(|error| format!("migrating video assets from Binary to Blob: {error}"))
    }

    pub async fn configure_connection(
        &self,
        settings: SpiralConnectionSettings,
    ) -> Result<(), String> {
        self.project_store.configure_connection(settings).await
    }

    pub fn namespace(&self) -> &str {
        self.project_store.namespace()
    }

    /// Commit a complete episode transaction directly to SpiralDB. This is
    /// also used once more when finalized camera segments arrive after the
    /// joint writer has inserted the episode's initial catalog row.
    pub async fn commit_episode(
        &self,
        episode: &FullEpisode,
        videos: &[VideoAsset],
    ) -> Result<(), String> {
        let namespace = self.project_store.namespace().to_owned();
        let mut connection = self.project_store.connect().await?;
        EpisodeReplacement::new(&namespace, episode, videos)
            .save(&mut connection)
            .await
    }

    pub async fn load_training_snapshot<T>(&self) -> Result<T, String>
    where
        T: DeserializeOwned + Default + Serialize,
    {
        if !self.is_configured() {
            return Ok(T::default());
        }
        self.load_json_document(Table::TrainingRuns, "training_history")
            .await
    }

    pub async fn save_training_snapshot<T: Serialize>(&self, value: &T) -> Result<(), String> {
        self.save_json_document(
            JsonDocumentKey::new(Table::TrainingRuns, "training_history"),
            value,
        )
        .await
    }

    pub async fn load_rollout_snapshot<T>(&self) -> Result<T, String>
    where
        T: DeserializeOwned + Default,
    {
        if !self.is_configured() {
            return Ok(T::default());
        }
        self.load_json_document(Table::RolloutRuns, "rollout_history")
            .await
    }

    pub async fn save_rollout_snapshot<T: Serialize>(&self, value: &T) -> Result<(), String> {
        self.save_json_document(
            JsonDocumentKey::new(Table::RolloutRuns, "rollout_history"),
            value,
        )
        .await
    }

    pub async fn load_workflow_document<T>(&self) -> Result<T, String>
    where
        T: DeserializeOwned + Default + Serialize,
    {
        if !self.is_configured() {
            return Ok(T::default());
        }
        self.load_json_document_resetting_incompatible(Table::WorkflowRuns, "default")
            .await
    }

    pub async fn save_workflow_document<T: Serialize>(&self, value: &T) -> Result<(), String> {
        self.save_json_document(JsonDocumentKey::new(Table::WorkflowRuns, "default"), value)
            .await
    }

    pub async fn load_workflow_agent_snapshot_json(&self) -> Result<Option<String>, String> {
        if !self.is_configured() {
            return Ok(None);
        }
        self.load_json_document_raw_optional(Table::WorkflowRuns, "agent:default")
            .await
    }

    pub async fn save_workflow_agent_document<T: Serialize>(
        &self,
        value: &T,
    ) -> Result<(), String> {
        let namespace = self.project_store.namespace().to_owned();
        let relation = Table::WorkflowRuns.qualified_name(&namespace);
        let json = serde_json::to_string(value)
            .map_err(|error| format!("encoding Workflow runs snapshot: {error}"))?;
        let mut connection = self.project_store.connect_exclusive().await?;
        connection.ensure_dataset_namespace(&namespace).await?;
        connection.set_autocommit(false).await?;
        let mutation = match JsonDocumentWrite::new(
            JsonDocumentTarget::new(Table::WorkflowRuns, &relation, "agent:default"),
            json,
        )
        .save(&mut connection)
        .await
        {
            Ok(()) => connection.commit().await,
            Err(error) => Err(error),
        };
        match mutation {
            Ok(()) => match connection.set_autocommit(true).await {
                Ok(()) => Ok(()),
                Err(_) => {
                    connection.discard();
                    Ok(())
                }
            },
            Err(error) => {
                let rollback = connection.rollback().await;
                let restore = connection.set_autocommit(true).await;
                let mut detail = error;
                if let Err(rollback) = rollback {
                    detail.push_str(&format!(
                        "; rolling back Assistant history failed: {rollback}"
                    ));
                }
                if let Err(restore) = restore {
                    detail.push_str(&format!(
                        "; restoring database autocommit failed: {restore}"
                    ));
                }
                Err(detail)
            }
        }
    }

    /// Replace an incompatible prototype Assistant document only when the
    /// durable raw snapshot is still exactly the one Studio inspected. The
    /// compare and replacement share one catalog transaction, so a newer
    /// document from another Studio cannot be overwritten.
    pub async fn reset_workflow_agent_document<T: Serialize>(
        &self,
        expected_raw_snapshot: &str,
        value: &T,
    ) -> Result<(), String> {
        let namespace = self.project_store.namespace().to_owned();
        let relation = Table::WorkflowRuns.qualified_name(&namespace);
        let expected_digest = Sha256::digest(expected_raw_snapshot.as_bytes()).to_vec();
        let json = serde_json::to_string(value)
            .map_err(|error| format!("encoding reset Assistant history: {error}"))?;
        let mut connection = self.project_store.connect_exclusive().await?;
        connection.set_autocommit(false).await?;
        let mutation = match connection
            .load_json_document_raw_optional(&relation, "agent:default")
            .await?
        {
            Some(current)
                if Sha256::digest(current.as_bytes()).as_slice() == expected_digest.as_slice() =>
            {
                match JsonDocumentWrite::new(
                    JsonDocumentTarget::new(Table::WorkflowRuns, &relation, "agent:default"),
                    json,
                )
                .save(&mut connection)
                .await
                {
                    Ok(()) => connection.commit().await,
                    Err(error) => Err(error),
                }
            }
            _ => Err(
                "Assistant history changed before reset. Reload the current project.".to_owned(),
            ),
        };
        let rollback = if mutation.is_err() {
            Some(connection.rollback().await)
        } else {
            None
        };
        if rollback.as_ref().is_some_and(Result::is_err) {
            connection.discard();
        }
        let restore = connection.set_autocommit(true).await;
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
    }

    pub async fn load_studio_preferences(&self) -> Result<StoredStudioPreferences, String> {
        if !self.is_configured() {
            return Ok(StoredStudioPreferences::default());
        }
        let relation = Table::StudioSettings.qualified_name(self.project_store.namespace());
        let mut connection = self.project_store.connect().await?;
        if !connection.table_exists(&relation).await? {
            return Ok(StoredStudioPreferences::default());
        }
        let result = connection
            .run_query(
                &format!(
                    "SELECT {} FROM {relation} WHERE {} = 'preferences' ORDER BY {} DESC LIMIT 1",
                    db::VALUE_JSON,
                    db::KEY,
                    db::UPDATED_MS,
                ),
                1,
            )
            .await?;
        let Some(value) = result
            .rows()
            .first()
            .and_then(|row| row.get(db::VALUE_JSON))
            .and_then(JsonValue::as_str)
        else {
            return Ok(StoredStudioPreferences::default());
        };
        serde_json::from_str(value).map_err(|error| format!("decoding Studio preferences: {error}"))
    }

    pub async fn save_studio_preferences(
        &self,
        preferences: &StoredStudioPreferences,
    ) -> Result<(), String> {
        let namespace = self.project_store.namespace().to_owned();
        let relation = Table::StudioSettings.qualified_name(&namespace);
        let json = serde_json::to_string(preferences)
            .map_err(|error| format!("encoding Studio preferences: {error}"))?;
        let mut connection = self.project_store.connect().await?;
        connection.ensure_dataset_namespace(&namespace).await?;
        connection
            .ensure_video_assets_blob_relation(&namespace)
            .await?;
        if connection.table_exists(&relation).await? {
            connection
                .execute_delete_sql(&format!(
                    "DELETE FROM {relation} WHERE {} = 'preferences'",
                    db::KEY
                ))
                .await?;
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
                Arc::new(Int64Array::from(vec![StorageClock::unix_millis()])) as ArrayRef,
            ],
        )
        .map_err(|error| format!("building Studio preferences batch: {error}"))?;
        connection.ingest_batch(&relation, batch).await
    }

    pub async fn export_project_bundle(&self) -> Result<Vec<u8>, String> {
        let namespace = self.project_store.namespace().to_owned();
        let mut connection = self.project_store.connect_exclusive().await?;
        bundle::ProjectBundle
            .export(&mut connection, &namespace)
            .await
    }

    pub async fn import_project_bundle(
        &self,
        bytes: &[u8],
    ) -> Result<spiral_robotics_api::datasets::ProjectImportSummary, String> {
        let namespace = self.project_store.namespace().to_owned();
        let mut connection = self.project_store.connect_exclusive().await?;
        bundle::ProjectBundle
            .import(
                &mut connection,
                bundle::BundleSource::new(&namespace, bytes),
            )
            .await
    }

    pub async fn export_episode_bundle(&self, dataset_id: &str) -> Result<Vec<u8>, String> {
        let namespace = self.project_store.namespace().to_owned();
        let mut connection = self.project_store.connect_exclusive().await?;
        bundle::EpisodeBundle
            .export(
                &mut connection,
                bundle::EpisodeBundleSelection::new(&namespace, dataset_id),
            )
            .await
    }

    pub async fn import_episode_bundle(
        &self,
        dataset_id: &str,
        bytes: &[u8],
    ) -> Result<spiral_robotics_api::datasets::EpisodeImportSummary, String> {
        let namespace = self.project_store.namespace().to_owned();
        let mut connection = self.project_store.connect_exclusive().await?;
        bundle::EpisodeBundle
            .import(
                &mut connection,
                bundle::EpisodeBundleImport::new(
                    bundle::EpisodeBundleSelection::new(&namespace, dataset_id),
                    bytes,
                ),
            )
            .await
    }

    async fn load_json_document<T>(&self, table: Table, key: &str) -> Result<T, String>
    where
        T: DeserializeOwned + Default,
    {
        let relation = table.qualified_name(self.project_store.namespace());
        let key = SpiralSql::string_literal(key);
        let mut connection = self.project_store.connect().await?;
        if !connection.table_exists(&relation).await? {
            return Ok(T::default());
        }
        let result = connection
            .run_query(
                &format!(
                    "SELECT {} FROM {relation} WHERE {} = {key} ORDER BY {} DESC LIMIT 1",
                    db::SNAPSHOT_JSON,
                    db::RUN_ID,
                    db::UPDATED_MS,
                ),
                1,
            )
            .await?;
        let Some(value) = result
            .rows()
            .first()
            .and_then(|row| row.get(db::SNAPSHOT_JSON))
            .and_then(JsonValue::as_str)
        else {
            return Ok(T::default());
        };
        serde_json::from_str(value).map_err(|error| format!("decoding {table} snapshot: {error}"))
    }

    async fn load_json_document_raw_optional(
        &self,
        table: Table,
        key: &str,
    ) -> Result<Option<String>, String> {
        let relation = table.qualified_name(self.project_store.namespace());
        let key = key.to_owned();
        let mut connection = self.project_store.connect().await?;
        connection
            .load_json_document_raw_optional(&relation, &key)
            .await
    }

    /// Workflow wire shapes are prototype-owned rather than a compatibility
    /// surface. Replace an obsolete snapshot with the current empty document
    /// in one transaction so every later hydration sees the same truth.
    async fn load_json_document_resetting_incompatible<T>(
        &self,
        table: Table,
        key: &str,
    ) -> Result<T, String>
    where
        T: DeserializeOwned + Default + Serialize,
    {
        let relation = table.qualified_name(self.project_store.namespace());
        let key = key.to_owned();
        let mut connection = self.project_store.connect_exclusive().await?;
        if !connection.table_exists(&relation).await? {
            return Ok(T::default());
        }
        let result = connection
            .run_query(
                &format!(
                    "SELECT {} FROM {relation} WHERE {} = {} ORDER BY {} DESC LIMIT 1",
                    db::SNAPSHOT_JSON,
                    db::RUN_ID,
                    SpiralSql::string_literal(&key),
                    db::UPDATED_MS,
                ),
                1,
            )
            .await?;
        let Some(value) = result
            .rows()
            .first()
            .and_then(|row| row.get(db::SNAPSHOT_JSON))
            .and_then(JsonValue::as_str)
        else {
            return Ok(T::default());
        };
        let (document, incompatible) = value.decode_json_or_default();
        if !incompatible {
            return Ok(document);
        }

        let json = serde_json::to_string(&document)
            .map_err(|error| format!("encoding reset {table} snapshot: {error}"))?;
        connection.set_autocommit(false).await?;
        let mutation =
            match JsonDocumentWrite::new(JsonDocumentTarget::new(table, &relation, key), json)
                .save(&mut connection)
                .await
            {
                Ok(()) => connection.commit().await,
                Err(error) => Err(error),
            };
        if mutation.is_err() {
            let _ = connection.rollback().await;
        }
        let restore = connection.set_autocommit(true).await;
        match (mutation, restore) {
            (Ok(()), Ok(())) => Ok(document),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(_)) => {
                connection.discard();
                Ok(document)
            }
            (Err(error), Err(restore)) => Err(format!(
                "{error}; restoring database autocommit after resetting obsolete Workflow history also failed: {restore}"
            )),
        }
    }

    async fn save_json_document<T: Serialize>(
        &self,
        document: JsonDocumentKey<'_>,
        value: &T,
    ) -> Result<(), String> {
        let namespace = self.project_store.namespace().to_owned();
        let target = JsonDocumentTarget::new(
            document.table,
            document.table.qualified_name(&namespace),
            document.key,
        );
        let json = serde_json::to_string(value)
            .map_err(|error| format!("encoding {} snapshot: {error}", document.table))?;
        let mut connection = self.project_store.connect().await?;
        connection.ensure_dataset_namespace(&namespace).await?;
        JsonDocumentWrite::new(target, json)
            .save(&mut connection)
            .await
    }

    pub async fn test_spiral(&self) -> ConnectionTestResponse {
        let endpoint = self.project_store.endpoint().to_owned();
        let project = self.project_store.project_id();
        if project.is_empty() {
            return ConnectionTestResponse::new(
                false,
                "Choose a Spiral project in Settings before testing the connection.",
            );
        }
        let catalog_project = project.clone();
        let table_filter = Table::Episodes.qualified_name(self.project_store.namespace());
        let catalog_check = async {
            let mut connection = self.project_store.connect().await?;
            let relation = RelationName::parse(table_filter).map_err(|error| error.to_string())?;
            let _ = connection
                .table_schema(relation)
                .await
                .map_err(|error| error.to_string())?;
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = catalog_check {
            return ConnectionTestResponse::new(false, error);
        }

        ConnectionTestResponse::new(
            true,
            format!("Connected to {endpoint} · project {catalog_project}"),
        )
    }

    pub async fn query(&self, sql: &str, max_rows: usize) -> Result<SpiralQueryResponse, String> {
        SpiralSql::ensure_read_only(sql, ReadOnlyQueryMode::Preview)?;
        let mut connection = self.project_store.connect().await?;
        QueryExecution::new(
            sql,
            QueryLimits::new(max_rows.clamp(1, MAX_QUERY_ROWS), MAX_QUERY_RESPONSE_BYTES),
        )
        .run_async(&mut connection)
        .await
    }

    pub async fn datasets(&self) -> Result<Vec<DatasetSummary>, String> {
        let relation = Table::Datasets.qualified_name(self.project_store.namespace());
        let mut connection = self.project_store.connect().await?;
        let target = RelationName::parse(&relation).map_err(|error| error.to_string())?;
        if connection
            .table_schema(target)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT {id}, {name}, {created}, {description} FROM {relation} ORDER BY {created}, {id}",
            id = db::DATASET_ID,
            name = db::NAME,
            created = db::CREATED_MS,
            description = db::DESCRIPTION,
        );
        let result = QueryExecution::new(
            &sql,
            QueryLimits::new(100_000, MAX_INTERNAL_QUERY_RESPONSE_BYTES),
        )
        .run_async(&mut connection)
        .await?;
        if result.truncated() {
            return Err("Spiral dataset catalog exceeds 100,000 rows".to_owned());
        }
        result
            .into_rows()
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                let text = |column| {
                    row.get(column)
                        .and_then(JsonValue::as_str)
                        .map(str::to_owned)
                };
                Ok(DatasetSummary::new(
                    text(db::DATASET_ID)
                        .ok_or_else(|| format!("dataset catalog row {index} has no dataset ID"))?,
                    text(db::NAME)
                        .ok_or_else(|| format!("dataset catalog row {index} has no name"))?,
                    row.get(db::CREATED_MS)
                        .and_then(JsonValue::as_i64)
                        .ok_or_else(|| {
                            format!("dataset catalog row {index} has no creation time")
                        })?,
                )
                .with_description(text(db::DESCRIPTION)))
            })
            .collect()
    }

    pub async fn workflow_relations(&self) -> Result<RelationCatalog, String> {
        let project = self.project_store.project_id();
        if project.is_empty() {
            return Ok(
                RelationCatalog::new(Vec::new(), false).with_limitation(Some(
                    "Choose a Spiral project in Settings to discover relations.".to_owned(),
                )),
            );
        }
        let mut connection = self.project_store.connect().await?;
        let catalog = connection
            .relations()
            .await
            .map_err(|error| error.to_string())?;
        let mut relations = Vec::with_capacity(catalog.len());
        let mut skipped = 0_usize;
        for entry in catalog {
            let (relation_name, kind) = entry.into_parts();
            if QualifiedRelation::try_from(relation_name.as_str()).is_err() {
                skipped += 1;
                continue;
            }
            let schema = match connection.table_schema(relation_name.clone()).await {
                Ok(Some(schema)) => schema,
                Ok(None) => {
                    skipped += 1;
                    continue;
                }
                Err(error)
                    if error.message().contains(
                        "extension type cannot be converted to Arrow without a plugin",
                    ) =>
                {
                    skipped += 1;
                    continue;
                }
                Err(error) => {
                    return Err(format!(
                        "describing Spiral relation {:?}: {error}",
                        relation_name.as_str()
                    ));
                }
            };
            let relation_type = match kind {
                RelationKind::Table => "TABLE",
                RelationKind::View => "VIEW",
            };
            relations.push(CatalogRelation::new(
                relation_name.into_string(),
                relation_type.to_owned(),
                schema
                    .fields()
                    .iter()
                    .map(|field| {
                        QueryColumn::new(
                            field.name().clone(),
                            field.data_type().to_string(),
                            field.is_nullable(),
                        )
                        .with_semantic_type(field.metadata().get(EXTENSION_TYPE_NAME_KEY).cloned())
                    })
                    .collect(),
            ));
        }
        let limitation = if skipped == 0 {
            Some("Row counts are omitted from Workflow relation discovery.".to_owned())
        } else {
            Some(format!(
                "Row counts are omitted; {skipped} catalog entries without a qualified name or Arrow schema were not returned."
            ))
        };
        Ok(RelationCatalog::new(relations, skipped == 0).with_limitation(limitation))
    }

    /// Describe the active project's user-facing data and add exact row
    /// counts through ordinary read-only SQL. Counts live outside the Workflow
    /// catalog so opening Workflow remains a schema-only operation.
    pub async fn project_data_relations(&self) -> Result<ProjectDataRelationsResponse, String> {
        let project = self.project_store.project_id();
        let namespace = self.project_store.namespace().to_owned();
        if project.is_empty() {
            return Ok(
                ProjectDataRelationsResponse::new(Vec::new(), false).with_limitation(Some(
                    "Choose a Spiral project in Settings to discover project data.".to_owned(),
                )),
            );
        }
        let mut connection = self.project_store.connect().await?;
        let catalog = connection
            .relations()
            .await
            .map_err(|error| error.to_string())?;
        let mut relations = Vec::with_capacity(catalog.len());
        let mut skipped = 0_usize;
        let mut uncounted = 0_usize;
        for entry in catalog {
            let (relation_name, kind) = entry.into_parts();
            if !Self::is_user_project_relation(relation_name.as_str(), &namespace) {
                continue;
            }
            if QualifiedRelation::try_from(relation_name.as_str()).is_err() {
                skipped += 1;
                continue;
            }
            let schema = match connection.table_schema(relation_name.clone()).await {
                Ok(Some(schema)) => schema,
                Ok(None) => {
                    skipped += 1;
                    continue;
                }
                Err(error)
                    if error.message().contains(
                        "extension type cannot be converted to Arrow without a plugin",
                    ) =>
                {
                    skipped += 1;
                    continue;
                }
                Err(error) => {
                    return Err(format!(
                        "describing Spiral relation {:?}: {error}",
                        relation_name.as_str()
                    ));
                }
            };
            let relation_type = match kind {
                RelationKind::Table => "TABLE",
                RelationKind::View => "VIEW",
            };
            let relation_name = relation_name.into_string();
            let row_count = match self
                .async_relation_row_count(&mut connection, &relation_name)
                .await
            {
                Ok(row_count) => Some(row_count),
                Err(_) => {
                    uncounted += 1;
                    None
                }
            };
            relations.push(
                ProjectDataRelation::new(
                    relation_name,
                    relation_type.to_owned(),
                    schema
                        .fields()
                        .iter()
                        .map(|field| {
                            QueryColumn::new(
                                field.name().clone(),
                                field.data_type().to_string(),
                                field.is_nullable(),
                            )
                            .with_semantic_type(
                                field.metadata().get(EXTENSION_TYPE_NAME_KEY).cloned(),
                            )
                        })
                        .collect(),
                )
                .with_row_count(row_count),
            );
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
        Ok(
            ProjectDataRelationsResponse::new(relations, skipped == 0 && uncounted == 0)
                .with_limitation(limitation),
        )
    }

    /// Resolve the query and its Arrow contract before starting the trainer.
    /// Unknown relations and renamed fields therefore fail the start request,
    /// rather than becoming an asynchronous training-task failure.
    async fn load_checkpoint_artifact(
        &self,
        key: CheckpointKey,
    ) -> Result<CheckpointArtifact, String> {
        let query = SpiralSql::training_checkpoint(
            self.project_store.namespace(),
            key.run_id(),
            key.step(),
        );
        self.checkpoint_query(query).await
    }

    async fn delete_checkpoint_artifacts(&self, run_id: String) -> Result<(), String> {
        let target = Table::TrainingCheckpoints.qualified_name(self.project_store.namespace());
        let mut connection = self.project_store.connect().await?;
        let relation = RelationName::parse(&target).map_err(|error| error.to_string())?;
        if connection
            .table_schema(relation)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(());
        }
        connection
            .execute(DatabaseCommand::statement(SpiralSql::delete_training_run(
                &target, &run_id,
            )))
            .await
            .and_then(crate::DatabaseOutput::into_summary)
            .map(|_| ())
            .map_err(|error| {
                format!("deleting training run {run_id:?} checkpoints from {target}: {error}")
            })
    }

    pub async fn trajectory(
        &self,
        episode: &str,
        stream: &str,
    ) -> Result<(f64, Vec<BTreeMap<String, f64>>), String> {
        let sql = SpiralSql::trajectory(self.project_store.namespace(), episode, stream);
        let mut connection = self.project_store.connect().await?;
        let result = QueryExecution::new(
            &sql,
            QueryLimits::new(MAX_REPLAY_ROWS, MAX_INTERNAL_QUERY_RESPONSE_BYTES),
        )
        .run_async(&mut connection)
        .await?;
        if result.truncated() {
            return Err(format!(
                "Spiral trajectory exceeds the replay safety limit of {MAX_REPLAY_ROWS} rows"
            ));
        }
        DecodedTrajectory::decode(result.into_rows()).map(DecodedTrajectory::into_parts)
    }

    /// Read the authoritative MP4 size without materializing its bytes.
    pub async fn video_asset_size(&self, asset: &VideoAssetKey) -> Result<u64, String> {
        let sql = SpiralSql::video_asset_size(self.project_store.namespace(), asset);
        self.u64_query(sql, db::BYTE_SIZE).await
    }

    /// Fetch exactly one browser-requested range from Spiral's managed Blob.
    ///
    /// The managed MP4 output retains the source Blob, including its provider
    /// URI and object offset. `spql.io.read_bytes` can therefore push the range
    /// into object storage instead of materializing the complete MP4 in Studio.
    pub async fn video_asset_range(
        &self,
        asset: &VideoAssetKey,
        range: VideoByteRange,
    ) -> Result<Vec<u8>, String> {
        let sql = SpiralSql::video_asset_range(self.project_store.namespace(), asset, range);
        self.binary_query(sql).await
    }

    pub async fn video_asset(&self, asset: VideoAssetKey) -> Result<Vec<u8>, String> {
        let size = self.video_asset_size(&asset).await?;
        if size == 0 {
            return Ok(Vec::new());
        }
        self.video_asset_range(&asset, VideoByteRange::new(0, size))
            .await
    }

    async fn record_batches(&self, sql: String) -> Result<crate::RecordBatchStream, String> {
        let mut connection = self.project_store.connect().await?;
        connection
            .execute(DatabaseCommand::statement(sql))
            .await
            .map_err(|error| error.to_string())?
            .into_records()
            .map_err(|error| error.to_string())
    }

    async fn binary_query(&self, sql: String) -> Result<Vec<u8>, String> {
        let mut records = self.record_batches(sql).await?;
        while let Some(batch) = records.next().await {
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

    async fn u64_query(&self, sql: String, column: &str) -> Result<u64, String> {
        let mut records = self.record_batches(sql).await?;
        while let Some(batch) = records.next().await {
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
        Err("Spiral scalar query returned no rows".to_owned())
    }

    async fn checkpoint_query(&self, sql: String) -> Result<CheckpointArtifact, String> {
        let mut records = self.record_batches(sql).await?;
        let mut artifact = None;
        while let Some(batch) = records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral checkpoint: {error}"))?;
            for row in 0..batch.num_rows() {
                if artifact.is_some() {
                    return Err("Spiral returned duplicate rows for that checkpoint".to_owned());
                }
                artifact = Some(CheckpointArtifact::from_batch_row(&batch, row)?);
            }
        }
        artifact.ok_or_else(|| "No checkpoint matched that training run and step".to_owned())
    }

    async fn async_relation_row_count(
        &self,
        connection: &mut dyn DatabaseSession,
        relation: &str,
    ) -> Result<u64, String> {
        let sql = SpiralSql::count_rows(relation);
        let result =
            QueryExecution::new(&sql, QueryLimits::new(1, MAX_INTERNAL_QUERY_RESPONSE_BYTES))
                .run_async(connection)
                .await?;
        let value = result
            .rows()
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
            || relation == SpiralSql::video_sources_relation(namespace)
            || relation == SpiralSql::video_dataset_relation(namespace)
            || relation == SpiralSql::video_dataset_output(namespace)
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
        .any(|table| relation == table.qualified_name(namespace))
    }
}

impl ProjectConnection {
    async fn load_json_document_raw_optional(
        &mut self,
        relation: &str,
        key: &str,
    ) -> Result<Option<String>, String> {
        if !self.table_exists(relation).await? {
            return Ok(None);
        }
        let result = self
            .run_query(
                &format!(
                    "SELECT {} FROM {relation} WHERE {} = {} ORDER BY {} DESC LIMIT 1",
                    db::SNAPSHOT_JSON,
                    db::RUN_ID,
                    SpiralSql::string_literal(key),
                    db::UPDATED_MS,
                ),
                1,
            )
            .await?;
        Ok(result
            .rows()
            .first()
            .and_then(|row| row.get(db::SNAPSHOT_JSON))
            .and_then(JsonValue::as_str)
            .map(str::to_owned))
    }
}

trait JsonDocumentDecode {
    fn decode_json_or_default<T>(&self) -> (T, bool)
    where
        T: DeserializeOwned + Default;
}

impl JsonDocumentDecode for str {
    fn decode_json_or_default<T>(&self) -> (T, bool)
    where
        T: DeserializeOwned + Default,
    {
        match serde_json::from_str(self) {
            Ok(document) => (document, false),
            Err(_) => (T::default(), true),
        }
    }
}

/// Decode remote replay data without dropping malformed joints or inventing a
/// clock. Physical replay is all-or-nothing: every row must declare the same
/// finite sample rate and one exact, unique, finite pose.
struct DecodedTrajectory {
    sample_hz: f64,
    frames: Vec<BTreeMap<String, f64>>,
}

impl DecodedTrajectory {
    fn decode(rows: Vec<JsonMap<String, JsonValue>>) -> Result<Self, String> {
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
        Ok(Self {
            sample_hz,
            frames: trajectory,
        })
    }

    fn into_parts(self) -> (f64, Vec<BTreeMap<String, f64>>) {
        (self.sample_hz, self.frames)
    }
}

impl CheckpointStore for StorageManager {
    fn load_checkpoint(&self, key: CheckpointKey) -> CheckpointFuture<'_, CheckpointArtifact> {
        Box::pin(async move { self.load_checkpoint_artifact(key).await })
    }

    fn delete_run_checkpoints(&self, run_id: String) -> CheckpointFuture<'_, ()> {
        Box::pin(async move { self.delete_checkpoint_artifacts(run_id).await })
    }
}

pub(crate) struct EpisodeReplacement<'a> {
    namespace: &'a str,
    episode: &'a FullEpisode,
    videos: &'a [VideoAsset],
}

impl<'a> EpisodeReplacement<'a> {
    pub(crate) const fn new(
        namespace: &'a str,
        episode: &'a FullEpisode,
        videos: &'a [VideoAsset],
    ) -> Self {
        Self {
            namespace,
            episode,
            videos,
        }
    }

    pub(crate) async fn save(self, connection: &mut ProjectConnection) -> Result<(), String> {
        let Self {
            namespace,
            episode,
            videos,
        } = self;
        connection.ensure_dataset_namespace(namespace).await?;
        if !videos.is_empty() {
            connection
                .ensure_video_assets_blob_relation(namespace)
                .await?;
        }
        let had_video = connection
            .episode_has_video_assets(namespace, episode.start().id())
            .await?;
        let video_rows_changed = had_video || !videos.is_empty();
        let dataset_exists = if video_rows_changed {
            connection.video_dataset_exists(namespace).await?
        } else {
            false
        };
        let previous_video_episodes = if video_rows_changed {
            connection.stored_video_episode_ids(namespace).await?
        } else {
            BTreeSet::new()
        };
        let has_video_rows = !videos.is_empty()
            || previous_video_episodes
                .iter()
                .any(|episode_id| episode_id != episode.start().id());
        if !dataset_exists && !previous_video_episodes.is_empty() && has_video_rows {
            connection
                .retarget_stored_video_media_relations(namespace)
                .await?;
        }
        // Replacement is idempotent. Clear any earlier complete row and partial
        // children before writing, so a retry after a process or network failure
        // can finish without duplicating already-landed rows.
        connection
            .clear_episode_rows_only(namespace, episode.start().id())
            .await?;
        let datasets = Table::Datasets.qualified_name(namespace);
        if !connection
            .table_has_dataset(&datasets, episode.start().dataset_id())
            .await?
        {
            connection
                .ingest_batch(
                    &datasets,
                    episode
                        .start()
                        .dataset_batch()
                        .map_err(|error| error.to_string())?,
                )
                .await?;
        }
        connection
            .ingest_batch(
                &Table::EpisodeStreams.qualified_name(namespace),
                episode
                    .start()
                    .streams_batch()
                    .map_err(|error| error.to_string())?,
            )
            .await?;
        if !videos.is_empty() {
            let streams = videos
                .iter()
                .map(|video| {
                    (
                        video.stream_id().to_owned(),
                        VideoStreamRow::new(
                            episode.start().id().to_owned(),
                            video.stream_id().to_owned(),
                            VideoStreamProperties::new(
                                video.camera_id().to_owned(),
                                video.size(),
                                video.timing().fps(),
                            ),
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .collect::<Vec<_>>();
            connection
                .ingest_batch(
                    &Table::EpisodeStreams.qualified_name(namespace),
                    streams.batch().map_err(|error| error.to_string())?,
                )
                .await?;
        }
        let mut encoder = SpiralEpisodeEncoder::new(episode.start().clone())
            .map_err(|error| error.to_string())?;
        let mut rows = Vec::new();
        for sample in episode.samples() {
            rows.extend(encoder.encode(sample).map_err(|error| error.to_string())?);
            if rows.len() >= JOINT_BATCH_ROWS {
                let batch = rows.batch().map_err(|error| error.to_string())?;
                connection
                    .ingest_batch(&Table::JointSamples.qualified_name(namespace), batch)
                    .await?;
                rows.clear();
            }
        }
        if !rows.is_empty() {
            let batch = rows.batch().map_err(|error| error.to_string())?;
            connection
                .ingest_batch(&Table::JointSamples.qualified_name(namespace), batch)
                .await?;
        }
        if !videos.is_empty() {
            let media_relation = SpiralSql::video_dataset_output(namespace);
            let batch =
                VideoAsset::publication_batch(episode.start().id(), videos, &media_relation)?;
            connection
                .ingest_batch(&Table::VideoAssets.qualified_name(namespace), batch)
                .await?;
        }
        if video_rows_changed {
            if has_video_rows {
                connection
                    .rebuild_video_dataset(namespace, dataset_exists)
                    .await?;
            } else {
                connection.drop_video_dataset(namespace).await?;
            }
        }
        // Inserted last: readers never see an episode whose streams or media only
        // partially arrived.
        connection
            .ingest_batch(
                &Table::Episodes.qualified_name(namespace),
                episode
                    .completion()
                    .episode_batch(episode.start(), encoder.frames())
                    .map_err(|error| error.to_string())?,
            )
            .await?;
        Ok(())
    }
}

/// Rebuild the namespace's one managed MP4 index from all durable `video_assets` rows.
///
/// The output is deliberately refreshed even when the Dataset already exists.
/// Project import replaces those source rows in place, so merely observing an
/// existing output does not prove that its video index matches the imported
/// payloads.
impl ProjectConnection {
    pub async fn rebuild_video_dataset(
        &mut self,
        namespace: &str,
        dataset_exists: bool,
    ) -> Result<(), String> {
        let connection = self;
        // One unfiltered view and Dataset cover the whole Studio namespace. The
        // output preserves episode and stream IDs, so consumers select individual
        // media without multiplying catalog resources by episode count.
        let sources = SpiralSql::video_sources_relation(namespace);
        let assets = Table::VideoAssets.qualified_name(namespace);
        let dataset = SpiralSql::video_dataset_relation(namespace);
        connection
            .execute_update_sql(&SpiralSql::create_video_sources(&sources, &assets))
            .await
            .map_err(|error| format!("creating Spiral video source view: {error}"))?;
        if !dataset_exists {
            connection
                .execute_update_sql(&SpiralSql::create_video_dataset(&dataset, &sources))
                .await
                .map_err(|error| format!("creating Spiral MP4 dataset: {error}"))?;
        }
        connection
            .execute_update_sql(&SpiralSql::refresh_dataset(&dataset))
            .await
            .map_err(|error| format!("refreshing Spiral MP4 dataset: {error}"))
    }

    pub(crate) async fn video_dataset_exists(&mut self, namespace: &str) -> Result<bool, String> {
        let connection = self;
        let dataset = SpiralSql::video_dataset_relation(namespace);
        match connection
            .run_query(&format!("DESCRIBE {dataset}"), 1)
            .await
        {
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

    pub(crate) async fn drop_video_dataset(&mut self, namespace: &str) -> Result<(), String> {
        let connection = self;
        let dataset = SpiralSql::video_dataset_relation(namespace);
        QualifiedRelation::try_from(dataset.as_str())
            .map_err(|error| format!("refusing to drop an invalid Spiral MP4 Dataset: {error}"))?;
        connection
            .execute_update_sql(&SpiralSql::drop_video_dataset(&dataset))
            .await
            .map_err(|error| format!("dropping Spiral MP4 dataset with CASCADE: {error}"))?;
        connection
            .execute_update_sql(&SpiralSql::drop_video_sources(
                &SpiralSql::video_sources_relation(namespace),
            ))
            .await
            .map_err(|error| format!("dropping Spiral video source view: {error}"))
    }

    /// Spiral's managed catalog is a resource tree: `spiral_robotics.episodes` can only
    /// be created after a live `spiral_robotics` parent exists. Until the catalog exposes
    /// a standalone namespace resource, a tiny durable root table is the parent
    /// container for every Spiral Robotics table, view, and managed Dataset.
    pub async fn ensure_dataset_namespace(&mut self, dataset: &str) -> Result<(), String> {
        let connection = self;
        match connection.relation_schema(dataset).await {
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
        connection
            .execute_update_sql(&SpiralSql::create_dataset_namespace(dataset))
            .await
            .map_err(|error| format!("creating SpiralDB namespace {dataset}: {error}"))
    }

    pub(crate) async fn clear_episode_rows(
        &mut self,
        namespace: &str,
        episode_id: &str,
    ) -> Result<(), String> {
        let connection = self;
        let previous_video_episodes = connection.stored_video_episode_ids(namespace).await?;
        let had_video = previous_video_episodes.contains(episode_id);
        let dataset_exists = if had_video {
            connection.video_dataset_exists(namespace).await?
        } else {
            false
        };
        if had_video && !dataset_exists && previous_video_episodes.len() > 1 {
            connection
                .retarget_stored_video_media_relations(namespace)
                .await?;
        }
        connection
            .clear_episode_rows_only(namespace, episode_id)
            .await?;
        if !had_video {
            return Ok(());
        }
        if previous_video_episodes.len() == 1 {
            connection.drop_video_dataset(namespace).await?;
        } else {
            connection
                .rebuild_video_dataset(namespace, dataset_exists)
                .await?;
        }
        Ok(())
    }

    async fn clear_episode_rows_only(
        &mut self,
        namespace: &str,
        episode_id: &str,
    ) -> Result<(), String> {
        let connection = self;
        for table in Table::EPISODE_RELATIONS {
            let target = table.qualified_name(namespace);
            if !connection.table_exists(&target).await? {
                continue;
            }
            connection
                .execute_delete_sql(&SpiralSql::delete_episode(&target, episode_id))
                .await
                .map_err(|error| format!("clearing prior Spiral rows in {target}: {error}"))?;
        }
        Ok(())
    }

    pub(crate) async fn stored_video_episode_ids(
        &mut self,
        namespace: &str,
    ) -> Result<BTreeSet<String>, String> {
        let connection = self;
        const MAX_VIDEO_EPISODES: usize = 100_000;

        let assets = Table::VideoAssets.qualified_name(namespace);
        if !connection.table_exists(&assets).await? {
            return Ok(BTreeSet::new());
        }
        let result = connection
            .run_query(
                &format!(
                    "SELECT DISTINCT {episode_id} FROM {assets}",
                    episode_id = db::EPISODE_ID
                ),
                MAX_VIDEO_EPISODES,
            )
            .await?;
        if result.truncated() {
            return Err(format!(
                "project has more than {MAX_VIDEO_EPISODES} video episodes; refusing incomplete video-resource reconciliation"
            ));
        }
        result
            .into_rows()
            .into_iter()
            .map(|row| {
                row.get(db::EPISODE_ID)
                    .and_then(JsonValue::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| "stored video asset has no valid episode ID".to_owned())
            })
            .collect()
    }

    async fn episode_has_video_assets(
        &mut self,
        namespace: &str,
        episode_id: &str,
    ) -> Result<bool, String> {
        let connection = self;
        let assets = Table::VideoAssets.qualified_name(namespace);
        if !connection.table_exists(&assets).await? {
            return Ok(false);
        }
        let result = connection.run_query(
        &format!(
            "SELECT {episode_column} FROM {assets} WHERE {episode_column} = {episode_id} LIMIT 1",
            episode_column = db::EPISODE_ID,
            episode_id = SpiralSql::string_literal(episode_id),
        ),
        1,
    ).await?;
        Ok(!result.rows().is_empty())
    }

    pub(crate) async fn retarget_stored_video_media_relations(
        &mut self,
        namespace: &str,
    ) -> Result<(), String> {
        let connection = self;
        let assets = Table::VideoAssets.qualified_name(namespace);
        let output = SpiralSql::video_dataset_output(namespace);
        connection
            .execute_update_sql(&format!(
                "UPDATE {assets} SET {media_relation} = {output}",
                media_relation = db::MEDIA_RELATION,
                output = SpiralSql::string_literal(&output),
            ))
            .await
            .map_err(|error| format!("retargeting stored video media relations: {error}"))
    }

    async fn table_has_dataset(&mut self, target: &str, dataset_id: &str) -> Result<bool, String> {
        let connection = self;
        if !connection.table_exists(target).await? {
            return Ok(false);
        }
        let result = connection
            .run_query(&SpiralSql::dataset_exists(target, dataset_id), 1)
            .await?;
        Ok(!result.rows().is_empty())
    }
}

impl ProjectConnection {
    pub async fn ingest_batch(&mut self, target: &str, batch: RecordBatch) -> Result<(), String> {
        self.ingest_batches(target, vec![batch]).await
    }

    pub(crate) async fn ingest_batches(
        &mut self,
        target: &str,
        batches: Vec<RecordBatch>,
    ) -> Result<(), String> {
        BatchWrite::new(target, batches, BatchWriteMode::Append)
            .save(self)
            .await
    }

    /// Atomically replace one complete Arbor relation from a single Arrow stream.
    pub async fn replace_batches(
        &mut self,
        target: &str,
        batches: Vec<RecordBatch>,
    ) -> Result<(), String> {
        BatchWrite::new(target, batches, BatchWriteMode::Replace)
            .save(self)
            .await
    }
}

#[derive(Clone, Copy)]
enum BatchWriteMode {
    Append,
    Replace,
}

struct BatchWrite<'a> {
    target: &'a str,
    batches: Vec<RecordBatch>,
    mode: BatchWriteMode,
}

impl<'a> BatchWrite<'a> {
    fn new(target: &'a str, batches: Vec<RecordBatch>, mode: BatchWriteMode) -> Self {
        Self {
            target,
            batches,
            mode,
        }
    }

    async fn save(self, connection: &mut ProjectConnection) -> Result<(), String> {
        let Some(schema) = self.batches.first().map(RecordBatch::schema) else {
            return Ok(());
        };
        let relation = RelationName::parse(self.target)
            .map_err(|error| format!("configuring native ingest for {}: {error}", self.target))?;
        let mode = if matches!(self.mode, BatchWriteMode::Replace) {
            crate::IngestMode::Replace
        } else {
            crate::IngestMode::CreateOrAppend
        };
        let batches = futures::stream::iter(self.batches.into_iter().map(Ok)).boxed();
        let command = DatabaseCommand::ingest(crate::BatchIngestion::new(
            relation,
            mode,
            crate::RecordBatchStream::new(schema, batches),
        ));
        connection
            .execute(command)
            .await
            .and_then(crate::DatabaseOutput::into_summary)
            .map_err(|error| {
                if matches!(self.mode, BatchWriteMode::Replace) {
                    format!("replacing Spiral table {}: {error}", self.target)
                } else {
                    format!("ingesting Spiral table {}: {error}", self.target)
                }
            })?;
        Ok(())
    }
}

impl ProjectConnection {
    pub async fn table_exists(&mut self, target: &str) -> Result<bool, String> {
        self.relation_schema(target)
            .await
            .map(|schema| schema.is_some())
            .map_err(|error| format!("checking Spiral table {target}: {error}"))
    }
}

#[cfg(test)]
struct TrainingCheckpointStorage;

#[cfg(test)]
impl TrainingCheckpointStorage {
    fn schema() -> Arc<Schema> {
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
}

impl ProjectConnection {
    pub async fn execute_update_sql(&mut self, sql: &str) -> Result<(), String> {
        self.execute(DatabaseCommand::statement(sql))
            .await
            .and_then(crate::DatabaseOutput::into_summary)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Finish a Spiral transaction, rebasing its optimistic project mutation when
    /// a concurrently published managed artifact advances the project head.
    pub(crate) async fn commit_catalog_transaction(&mut self) -> Result<(), String> {
        const MAX_COMMIT_ATTEMPTS: usize = 16;

        for attempt in 1..=MAX_COMMIT_ATTEMPTS {
            match self.commit().await {
                Ok(()) => return Ok(()),
                Err(error)
                    if error.contains("project namespace compare-and-swap conflict")
                        && attempt < MAX_COMMIT_ATTEMPTS =>
                {
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the bounded commit loop returns on its final attempt")
    }

    /// Spiral reports a no-op DELETE as an execution error. Replacement and
    /// cleanup callers intentionally treat that driver-specific result as success.
    pub async fn execute_delete_sql(&mut self, sql: &str) -> Result<(), String> {
        match self.execute_update_sql(sql).await {
            Err(error) if error.contains("without any affected rows") => Ok(()),
            result => result,
        }
    }
}

impl VideoAsset {
    pub(crate) fn storage_publication_batch(
        episode_id: &str,
        assets: &[Self],
        media_relation: &str,
    ) -> Result<RecordBatch, String> {
        let videos = assets;
        let payloads = videos.iter().map(VideoAsset::payload).collect::<Vec<_>>();
        let timeline_offsets = videos
            .iter()
            .map(|video| {
                video
                    .segment()
                    .timeline_offset_seconds()
                    .map(|seconds| {
                        if !seconds.is_finite() || seconds < 0.0 {
                            return Err(format!(
                                "video timeline offset for {} must be finite and non-negative",
                                video.stream_id()
                            ));
                        }
                        let nanos = seconds * 1_000_000_000.0;
                        if nanos > u64::MAX as f64 {
                            return Err(format!(
                                "video timeline offset for {} is too large",
                                video.stream_id()
                            ));
                        }
                        Ok(nanos.round() as u64)
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let schema = VideoAssetStorage::schema();
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                videos.iter().map(|_| episode_id),
            )),
            Arc::new(StringArray::from_iter_values(
                videos.iter().map(VideoAsset::stream_id),
            )),
            Arc::new(UInt32Array::from_iter_values(
                videos.iter().map(|video| video.segment().index()),
            )),
            Arc::new(UInt64Array::from(timeline_offsets)),
            Arc::new(StringArray::from_iter_values(
                videos.iter().map(VideoAsset::camera_id),
            )),
            VideoAssetStorage::inline_blob_array(&payloads)?,
            Arc::new(UInt64Array::from_iter_values(
                payloads.iter().map(|payload| payload.len() as u64),
            )),
            Arc::new(UInt32Array::from_iter_values(
                videos.iter().map(|video| video.size().width()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                videos.iter().map(|video| video.size().height()),
            )),
            Arc::new(UInt32Array::from_iter_values(
                videos.iter().map(|video| video.timing().fps()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                videos.iter().map(|video| video.timing().frames()),
            )),
            Arc::new(StringArray::from_iter_values(
                videos.iter().map(|_| media_relation),
            )),
        ];
        RecordBatch::try_new(schema, columns)
            .map_err(|error| format!("building Spiral video batch: {error}"))
    }
}

struct VideoAssetStorage;

impl VideoAssetStorage {
    fn schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new(db::EPISODE_ID, DataType::Utf8, false),
            Field::new(db::STREAM_ID, DataType::Utf8, false),
            Field::new(db::SEGMENT_INDEX, DataType::UInt32, false),
            Field::new(db::TIMELINE_OFFSET_NS, DataType::UInt64, true),
            Field::new(db::CAMERA_ID, DataType::Utf8, false),
            Self::blob_field(db::PAYLOAD),
            Field::new(db::BYTE_SIZE, DataType::UInt64, false),
            Field::new(db::WIDTH, DataType::UInt32, false),
            Field::new(db::HEIGHT, DataType::UInt32, false),
            Field::new(db::FPS, DataType::UInt32, false),
            Field::new(db::FRAMES, DataType::UInt64, false),
            Field::new(db::MEDIA_RELATION, DataType::Utf8, false),
        ]))
    }

    fn blob_storage_fields() -> Fields {
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

    fn blob_field(name: &str) -> Field {
        Field::new(name, DataType::Struct(Self::blob_storage_fields()), true).with_metadata(
            [(
                EXTENSION_TYPE_NAME_KEY.to_owned(),
                BLOB_EXTENSION_NAME.to_owned(),
            )]
            .into(),
        )
    }

    fn inline_blob_array(payloads: &[&[u8]]) -> Result<ArrayRef, String> {
        let len = payloads.len();
        let fields = Self::blob_storage_fields();
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
}

pub(crate) trait VideoAssetRecordBatch {
    fn normalize_video_asset(self) -> Result<RecordBatch, String>;
}

impl VideoAssetRecordBatch for RecordBatch {
    fn normalize_video_asset(self) -> Result<RecordBatch, String> {
        let batch = self;
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
        fields[payload_index] = VideoAssetStorage::blob_field(db::PAYLOAD);
        let schema = Arc::new(Schema::new_with_metadata(
            fields,
            batch.schema().metadata().clone(),
        ));
        let mut columns = batch.columns().to_vec();
        columns[payload_index] = VideoAssetStorage::inline_blob_array(&payloads)?;
        RecordBatch::try_new(schema, columns)
            .map_err(|error| format!("normalizing video asset Blob batch: {error}"))
    }
}

impl ProjectConnection {
    pub(crate) async fn ensure_video_assets_blob_relation(
        &mut self,
        namespace: &str,
    ) -> Result<(), String> {
        let connection = self;
        let assets = Table::VideoAssets.qualified_name(namespace);
        let Some(schema) = connection.relation_schema(&assets).await? else {
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
        connection
            .execute_update_sql(&SpiralSql::migrate_video_assets_to_blob(&assets))
            .await
            .map_err(|error| format!("migrating video assets from Binary to Blob: {error}"))
    }
}

impl ProjectConnection {
    pub async fn run_query(
        &mut self,
        sql: &str,
        max_rows: usize,
    ) -> Result<SpiralQueryResponse, String> {
        QueryExecution::new(
            sql,
            QueryLimits::new(max_rows, MAX_INTERNAL_QUERY_RESPONSE_BYTES),
        )
        .run_async(self)
        .await
    }
}

impl QueryExecution<'_> {
    async fn run_async(
        self,
        connection: &mut dyn DatabaseSession,
    ) -> Result<SpiralQueryResponse, String> {
        let sql = self.sql.trim();
        if sql.is_empty() {
            return Err("Spiral query cannot be empty".to_owned());
        }
        if sql.len() > 64 * 1024 {
            return Err("Spiral query is too large".to_owned());
        }
        let started = Instant::now();
        let output = connection
            .execute(DatabaseCommand::statement(sql))
            .await
            .map_err(|error| format!("executing Spiral query: {error}"))?;
        let mut records = output
            .into_records()
            .map_err(|error| format!("executing Spiral query: {error}"))?;
        let schema = Arc::clone(records.schema());
        let columns = schema
            .fields()
            .iter()
            .map(|field| {
                QueryColumn::new(
                    field.name().clone(),
                    field.data_type().to_string(),
                    field.is_nullable(),
                )
                .with_semantic_type(field.metadata().get(EXTENSION_TYPE_NAME_KEY).cloned())
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
        let fetch = self.limits.rows.saturating_add(1);
        let mut rows = QueryRows::with_capacity(
            self.limits.rows.min(QUERY_JSON_CHUNK_ROWS),
            self.limits.bytes,
        );
        let mut inspected = 0_usize;
        let mut truncated = false;
        'result: while let Some(batch) = records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral result: {error}"))?;
            let mut offset = 0_usize;
            while offset < batch.num_rows() && inspected < fetch {
                let count = chunk_rows
                    .min(batch.num_rows() - offset)
                    .min(fetch - inspected);
                let chunk = batch.slice(offset, count);
                let preview = chunk.query_preview()?;
                for row in preview.to_json_rows()? {
                    inspected += 1;
                    if rows.values.len() >= self.limits.rows {
                        truncated = true;
                        break 'result;
                    }
                    if !rows.push(row)? {
                        truncated = true;
                        break 'result;
                    }
                }
                offset += count;
            }
        }
        Ok(
            SpiralQueryResponse::new(columns, rows.into_values(), truncated)
                .with_elapsed_ms(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
        )
    }
}

/// Replace displayable image extension columns with bounded browser-ready
/// data URLs before Arrow's JSON writer expands every pixel into a number.
/// The response schema is captured from the original batch, so callers still
/// receive the real Arrow physical and semantic types.
trait RecordBatchPresentation {
    fn query_preview(&self) -> Result<RecordBatch, String>;
    fn to_json_rows(&self) -> Result<Vec<JsonMap<String, JsonValue>>, String>;
}

impl RecordBatchPresentation for RecordBatch {
    fn query_preview(&self) -> Result<RecordBatch, String> {
        let batch = self;
        let mut fields = Vec::with_capacity(batch.num_columns());
        let mut columns = Vec::with_capacity(batch.num_columns());
        let mut changed = false;
        for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
            let semantic_type = field
                .metadata()
                .get(EXTENSION_TYPE_NAME_KEY)
                .map(String::as_str);
            let preview = match semantic_type {
                Some(IMAGE_EXTENSION_NAME) => Some(column.as_ref().image_preview()?),
                Some(VARIABLE_TENSOR_EXTENSION_NAME) => column.as_ref().tensor_preview()?,
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

    fn to_json_rows(&self) -> Result<Vec<JsonMap<String, JsonValue>>, String> {
        let batch = self;
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
        let values: Vec<JsonValue> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("decoding Spiral rows: {error}"))?;
        values
            .into_iter()
            .map(|value| match value {
                JsonValue::Object(row) => Ok(row),
                _ => Err("Spiral returned a non-object row".to_owned()),
            })
            .collect()
    }
}

trait ArrowArrayPreview {
    fn image_preview(&self) -> Result<ArrayRef, String>;
    fn tensor_preview(&self) -> Result<Option<ArrayRef>, String>;
    fn list_u8(&self, row: usize) -> Option<Vec<u8>>;
}

impl ArrowArrayPreview for dyn arrow_array::Array {
    fn image_preview(&self) -> Result<ArrayRef, String> {
        let array = self;
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
            let pixels = data
                .as_ref()
                .list_u8(row)
                .ok_or_else(|| "Spiral Image data must be a List<UInt8>".to_owned())?;
            previews.push(Some(
                QueryImage::new(
                    &pixels,
                    ImageDimensions::new(widths.value(row), heights.value(row)),
                    formats.value(row),
                )
                .encode()?,
            ));
        }
        Ok(Arc::new(StringArray::from(previews)))
    }

    /// Render only the canonical HWC UInt8 tensor shape used by
    /// `spql.image.to_tensor(image, 'hwc', 'none')`. Other tensors retain their
    /// ordinary JSON representation instead of being mislabeled as imagery.
    fn tensor_preview(&self) -> Result<Option<ArrayRef>, String> {
        let array = self;
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
            let Some(pixels) = data.as_ref().list_u8(row) else {
                return Ok(None);
            };
            previews.push(Some(
                QueryImage::new(
                    &pixels,
                    ImageDimensions::new(
                        u32::try_from(*width)
                            .map_err(|_| "Spiral image tensor width overflowed u32")?,
                        u32::try_from(*height)
                            .map_err(|_| "Spiral image tensor height overflowed u32")?,
                    ),
                    if *channels == 3 { 1 } else { 2 },
                )
                .encode()?,
            ));
        }
        Ok(Some(Arc::new(StringArray::from(previews))))
    }

    fn list_u8(&self, row: usize) -> Option<Vec<u8>> {
        let array = self;
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
}

impl QueryImage<'_> {
    fn encode(self) -> Result<String, String> {
        let ImageDimensions { width, height } = self.dimensions;
        let pixels = self.pixels;
        let pixel_format = self.pixel_format;
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
            1 => image::RgbImage::from_raw(width, height, pixels.to_vec())
                .map(DynamicImage::ImageRgb8),
            2 => image::RgbaImage::from_raw(width, height, pixels.to_vec())
                .map(DynamicImage::ImageRgba8),
            _ => unreachable!(),
        }
        .ok_or_else(|| "Spiral Image pixels did not match their dimensions".to_owned())?;
        let preview =
            if width > QUERY_IMAGE_PREVIEW_MAX_EDGE || height > QUERY_IMAGE_PREVIEW_MAX_EDGE {
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
}

impl ProjectConnection {
    #[cfg(test)]
    pub(crate) async fn run_binary_query(&mut self, sql: &str) -> Result<Vec<u8>, String> {
        let mut records = self.execute_query(sql).await?;
        while let Some(batch) = records.next().await {
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
}

impl CheckpointArtifact {
    fn from_batch_row(batch: &RecordBatch, row: usize) -> Result<Self, String> {
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
        Self::validate_size(0, payload.len())?;
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
        Self::validate_size(config_json.len(), payload.len())?;
        Self::new(config_json, payload)
    }
}

struct StorageClock;

impl StorageClock {
    fn unix_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(i64::MAX)
    }
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
            assert!(!StorageManager::is_user_project_relation(
                &table.qualified_name(namespace),
                namespace
            ));
        }
        assert!(!StorageManager::is_user_project_relation(
            "workflow.feature_registry",
            namespace
        ));
        assert!(!StorageManager::is_user_project_relation(
            "robotics.video.assets",
            namespace
        ));
        assert!(StorageManager::is_user_project_relation(
            "robotics.episodes",
            namespace
        ));
        assert!(StorageManager::is_user_project_relation(
            "customer.annotations",
            namespace
        ));
    }

    #[test]
    fn obsolete_workflow_json_resets_once_instead_of_poisoning_bootstrap() {
        let (document, incompatible) =
            r#"{"concepts":["cube"]}"#.decode_json_or_default::<CurrentPrototypeDocument>();
        assert!(incompatible);
        assert_eq!(document, CurrentPrototypeDocument::default());

        let (document, incompatible) = r#"{"features":["is_cube_feature"]}"#
            .decode_json_or_default::<CurrentPrototypeDocument>();
        assert!(!incompatible);
        assert_eq!(document.features, ["is_cube_feature"]);
    }

    #[test]
    fn backend_owned_arrow_schemas_match_the_shared_contract() {
        assert_eq!(
            TrainingCheckpointStorage::schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            Table::TrainingCheckpoints.columns(),
        );
        assert_eq!(
            VideoAssetStorage::schema()
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
        let array = VideoAssetStorage::inline_blob_array(&[payload.as_slice()]).unwrap();
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
            SpiralSql::ensure_read_only(
                "SELECT * FROM robotics.episodes",
                ReadOnlyQueryMode::Preview
            )
            .is_ok()
        );
        assert!(
            SpiralSql::ensure_read_only(
                "DELETE FROM robotics.episodes",
                ReadOnlyQueryMode::Preview
            )
            .is_err()
        );
        assert!(
            SpiralSql::ensure_read_only("SELECT 1; SELECT 2", ReadOnlyQueryMode::Preview).is_err()
        );
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

        let preview = batch.query_preview().unwrap();
        let rows = preview.to_json_rows().unwrap();
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

        let rows = batch.query_preview().unwrap().to_json_rows().unwrap();
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
        let (sample_hz, trajectory) = DecodedTrajectory::decode(rows).unwrap().into_parts();
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
        assert!(DecodedTrajectory::decode(rows).is_err());
    }
}
