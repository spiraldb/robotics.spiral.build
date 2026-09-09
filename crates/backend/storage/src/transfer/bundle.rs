//! Portable single-file snapshots of the Studio-owned Spiral tables.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Cursor, Read as _};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::{self as spiral, DatabaseBundleSection, ManagedRelationSchema, Table, column};
use arrow_array::{
    Array as _, ArrayRef, Float32Array, LargeStringArray, ListArray, RecordBatch, StringArray,
    StringViewArray, UInt32Array, UInt64Array,
};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::ProjectImportSummary;
use spiral_robotics_common::sql::{QualifiedRelation, UnquotedSqlIdentifier};
use spiral_robotics_common::workflow::{
    MAX_SIGLIP_CONCEPT_LENGTH, MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH,
    MAX_WORKFLOW_FEATURE_STREAMS, WorkflowFeatureApplicability, WorkflowFeatureDefinition,
};

use crate::db::ArrowColumns;
use crate::db::ProjectConnection;
use crate::db::SpiralSql;
use crate::episodes::EpisodeRelations;
use crate::rollout_runs::RolloutBundleSection;
use crate::settings::StudioPreferenceBundleSection;
use crate::training::TrainingBundleSection;
use crate::workflow_documents::WorkflowDocumentBundleSection;
use crate::workflow_features::{
    CACHE_KEY_FIELDS, FEATURE_VALUE_SEMANTICS, MAX_FEATURE_CACHE_COLUMNS, MAX_FEATURE_CACHE_ROWS,
    MAX_FEATURE_REGISTRY_ROWS, SIGLIP_BASE_MODEL_ID, SIGLIP_BASE_REPOSITORY, SIGLIP_BASE_REVISION,
    WorkflowFeatureBundleSection, WorkflowFeatureSchema,
};

const PROJECT_MAGIC: &[u8; 8] = b"SPRPROJ1";
const PROJECT_FORMAT: &str = "spiral-robotics-project";
const FORMAT_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ENTRY_NAME_BYTES: usize = 256;
const MAX_ARROW_MESSAGE_METADATA_BYTES: usize = MAX_MANIFEST_BYTES;
const MAX_ARROW_DECODED_BYTES: usize = ProjectBundle::MAX_BYTES;
const MAX_ARROW_RECORD_BATCH_ROWS: usize = MAX_FEATURE_CACHE_ROWS;
const MAX_ARROW_DECODED_ROWS: usize = MAX_FEATURE_CACHE_ROWS;
const MAX_ARROW_SCHEMA_FIELDS: usize = MAX_FEATURE_CACHE_COLUMNS + CACHE_KEY_FIELDS.len();
#[cfg(test)]
const WORKFLOW_AGENT_DOCUMENT_SCHEMA_VERSION: u64 = 2;
struct WorkflowArrow;

/// Browser transfers are memory-backed at the Axum/storage boundary. Keep the
/// cap low enough that one import and its decoded Arrow batch cannot exhaust a
/// typical Studio workstation.
pub struct ProjectBundle;

impl ProjectBundle {
    pub const MAX_BYTES: usize = 256 * 1024 * 1024;
}

pub(super) struct ProjectBundleImport<'a> {
    table_prefix: &'a str,
    bytes: &'a [u8],
}

impl<'a> ProjectBundleImport<'a> {
    pub(super) const fn new(table_prefix: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            table_prefix,
            bytes,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Manifest {
    format: String,
    format_version: u32,
    schema_version: u32,
    exported_ms: i64,
    relations: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProjectRelation {
    Managed(Table),
    WorkflowFeatureCache {
        bundle_relation: String,
        source_relation: String,
    },
}

impl ProjectRelation {
    #[cfg(test)]
    fn bundle_name(&self) -> &str {
        match self {
            Self::Managed(table) => table.name(),
            Self::WorkflowFeatureCache {
                bundle_relation, ..
            } => bundle_relation,
        }
    }

    fn target(&self, table_prefix: &str) -> String {
        match self {
            Self::Managed(table) => table.qualified_name(table_prefix),
            Self::WorkflowFeatureCache {
                source_relation, ..
            } => spiral::StorageSchema::workflow_feature_cache_relation(
                table_prefix,
                source_relation,
            ),
        }
    }
}

#[derive(Debug)]
struct DecodedProjectRelation {
    relation: ProjectRelation,
    batches: Vec<RecordBatch>,
}

#[derive(Debug, Eq, PartialEq)]
struct WorkflowRestorePlan {
    retire_caches: Vec<String>,
}

#[derive(Clone, Debug)]
struct WorkflowBundleRegistryEntry {
    definition: WorkflowFeatureDefinition,
    cache_relation: String,
}

#[derive(Clone, Debug, Default)]
struct WorkflowBundleRegistry {
    entries: BTreeMap<(String, String), WorkflowBundleRegistryEntry>,
}

impl WorkflowBundleRegistry {
    fn cache_relations(&self) -> BTreeSet<String> {
        self.entries
            .values()
            .map(|entry| entry.cache_relation.clone())
            .collect()
    }

    fn features_for_cache(
        &self,
        cache_relation: &str,
    ) -> BTreeMap<String, WorkflowFeatureDefinition> {
        self.entries
            .values()
            .filter(|entry| entry.cache_relation == cache_relation)
            .map(|entry| (entry.definition.name().to_owned(), entry.definition.clone()))
            .collect()
    }

    fn source_for_cache(&self, cache_relation: &str) -> Option<&str> {
        self.entries
            .values()
            .find(|entry| entry.cache_relation == cache_relation)
            .map(|entry| entry.definition.applicability().source_relation())
    }
}

impl WorkflowBundleRegistry {
    async fn load(
        connection: &mut ProjectConnection,
        table_prefix: &str,
    ) -> Result<Option<WorkflowBundleRegistry>, String> {
        let relation = Table::FeatureRegistry.qualified_name(table_prefix);
        if !connection.table_exists(&relation).await? {
            return Ok(None);
        }
        let mut records = connection
            .execute_query(format!("SELECT * FROM {relation}"))
            .await
            .map_err(|error| format!("reading Workflow feature registry: {error}"))?;
        let schema = records.schema().clone();
        let mut batches = Vec::new();
        while let Some(batch) = records.next().await {
            batches.push(
                batch.map_err(|error| format!("reading Workflow feature registry: {error}"))?,
            );
        }
        if batches.is_empty() {
            batches.push(RecordBatch::new_empty(schema.clone()));
        }
        Self::validate_for_namespace(&batches, table_prefix).map(Some)
    }
}

impl ProjectBundle {
    pub(super) async fn export(
        &self,
        connection: &mut ProjectConnection,
        table_prefix: &str,
    ) -> Result<Vec<u8>, String> {
        let mut queries = Vec::new();
        for table in ManagedRelationCatalog::tables() {
            let relation = table.qualified_name(table_prefix);
            let query = if table == Table::VideoAssets {
                ProjectBundle::portable_video_assets_query(&relation)
            } else {
                format!("SELECT * FROM {relation}")
            };
            queries.push((table.name().to_owned(), query));
        }
        if let Some(registry) = WorkflowBundleRegistry::load(connection, table_prefix).await? {
            for relation in registry.cache_relations() {
                if !connection.table_exists(&relation).await? {
                    return Err(format!(
                        "Workflow feature registry declares missing cache relation {relation:?}"
                    ));
                }
                queries.push((relation.clone(), format!("SELECT * FROM {relation}")));
            }
        }
        self.export_named_queries(connection, queries).await
    }
}

impl ProjectBundle {
    async fn export_named_queries(
        &self,
        connection: &mut ProjectConnection,
        queries: Vec<(String, String)>,
    ) -> Result<Vec<u8>, String> {
        let mut entries = Vec::<(String, Vec<u8>)>::new();
        for (relation, query) in queries {
            let mut records = connection
                .execute_query(query)
                .await
                .map_err(|error| format!("reading project relation {relation}: {error}"))?;
            // A project must only ever hold shapes its own importer accepts.
            if let Some(table) = ManagedRelationCatalog::table_named(&relation) {
                ManagedRelationCatalog::validate(table, records.schema().as_ref(), "project")
                    .map_err(|error| format!("{error}; relation {relation} is not exportable"))?;
            }
            let occupied = Self::encoded_entries_size(&entries)
                .saturating_add(PROJECT_MAGIC.len() + 4 + MAX_MANIFEST_BYTES + 2);
            let payload_limit = Self::MAX_BYTES
                .saturating_sub(occupied)
                .saturating_sub(2 + relation.len() + 8);
            let mut payload = LimitedPayload::new(payload_limit);
            {
                let mut writer = StreamWriter::try_new(&mut payload, records.schema().as_ref())
                    .map_err(|error| format!("opening Arrow stream for {relation}: {error}"))?;
                while let Some(batch) = records.next().await {
                    let batch = batch
                        .map_err(|error| format!("reading project relation {relation}: {error}"))?;
                    writer
                        .write(&batch)
                        .map_err(|error| format!("writing Arrow stream for {relation}: {error}"))?;
                }
                writer
                    .finish()
                    .map_err(|error| format!("finishing Arrow stream for {relation}: {error}"))?;
            }
            let payload = payload.into_inner();
            let projected_size = Self::encoded_entries_size(&entries)
                .saturating_add(2 + relation.len() + 8 + payload.len())
                .saturating_add(PROJECT_MAGIC.len() + 4 + MAX_MANIFEST_BYTES);
            if projected_size > Self::MAX_BYTES {
                return Err(format!(
                    "project export exceeds the {} MiB portable-file limit",
                    Self::MAX_BYTES / (1024 * 1024),
                ));
            }
            entries.push((relation, payload));
        }
        self.build(entries)
    }
}

impl ProjectBundle {
    fn portable_video_assets_query(relation: &str) -> String {
        format!(
            "SELECT {episode_id}, {stream_id}, {segment_index}, {timeline_offset}, {camera_id}, \
         spql.io.read_bytes({payload}) AS {payload}, {byte_size}, {width}, {height}, {fps}, \
         {frames}, {media_relation} FROM {relation}",
            episode_id = column::EPISODE_ID,
            stream_id = column::STREAM_ID,
            segment_index = column::SEGMENT_INDEX,
            timeline_offset = column::TIMELINE_OFFSET_NS,
            camera_id = column::CAMERA_ID,
            payload = column::PAYLOAD,
            byte_size = column::BYTE_SIZE,
            width = column::WIDTH,
            height = column::HEIGHT,
            fps = column::FPS,
            frames = column::FRAMES,
            media_relation = column::MEDIA_RELATION,
        )
    }

    fn encoded_entries_size(entries: &[(String, Vec<u8>)]) -> usize {
        entries
            .iter()
            .map(|(name, payload)| 2 + name.len() + 8 + payload.len())
            .sum()
    }
}

impl ProjectBundle {
    fn build(&self, entries: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
        if let Some((name, _)) = entries
            .iter()
            .find(|(name, _)| name.len() > MAX_ENTRY_NAME_BYTES)
        {
            return Err(format!("project relation name {name:?} is too long"));
        }
        let manifest = Manifest {
            format: PROJECT_FORMAT.to_owned(),
            format_version: FORMAT_VERSION,
            schema_version: spiral::SCHEMA_VERSION,
            exported_ms: BundleClock::unix_millis(),
            relations: entries.iter().map(|(name, _)| name.clone()).collect(),
        };
        let manifest = serde_json::to_vec(&manifest)
            .map_err(|error| format!("encoding project manifest: {error}"))?;
        let mut bundle = Vec::new();
        bundle.extend_from_slice(PROJECT_MAGIC);
        BundleCodec::write_u32(&mut bundle, manifest.len())?;
        bundle.extend_from_slice(&manifest);
        for (name, payload) in entries {
            BundleCodec::write_u16(&mut bundle, name.len())?;
            bundle.extend_from_slice(name.as_bytes());
            BundleCodec::write_u64(&mut bundle, payload.len())?;
            bundle.extend_from_slice(&payload);
        }
        bundle.extend_from_slice(&0_u16.to_le_bytes());
        if bundle.len() > Self::MAX_BYTES {
            return Err(format!(
                "project export exceeds the {} MiB portable-file limit",
                Self::MAX_BYTES / (1024 * 1024),
            ));
        }
        Ok(bundle)
    }
}

impl ProjectBundle {
    pub(super) async fn import(
        &self,
        connection: &mut ProjectConnection,
        source: ProjectBundleImport<'_>,
    ) -> Result<ProjectImportSummary, String> {
        let ProjectBundleImport {
            table_prefix,
            bytes,
        } = source;
        let decoded = ProjectBundle::decode_entries(self.parse_raw(bytes)?)?;
        let decoded = ProjectBundle::with_empty_managed_relations(decoded);
        let fixed_relations = decoded
            .iter()
            .filter_map(|entry| match &entry.relation {
                ProjectRelation::Managed(table) => Some((*table, entry.batches.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        let imported_video_episodes = ProjectBundle::video_episode_ids(&fixed_relations)?;
        let current_caches = WorkflowBundleRegistry::load(connection, table_prefix)
            .await?
            .map(|registry| registry.cache_relations())
            .unwrap_or_default();
        let incoming_caches = decoded
            .iter()
            .filter_map(|entry| match &entry.relation {
                ProjectRelation::WorkflowFeatureCache { .. } => {
                    Some(entry.relation.target(table_prefix))
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let workflow_plan = WorkflowRestorePlan::from_caches(&current_caches, &incoming_caches);

        connection
            .begin_bundle_transaction("project import")
            .await?;
        let outcome = async {
            for relation in &workflow_plan.retire_caches {
                if connection.table_exists(relation).await? {
                    connection
                        .execute_update_sql(&format!("DROP TABLE {relation}"))
                        .await
                        .map_err(|error| format!("clearing Workflow cache {relation}: {error}"))?;
                }
            }
            let mut summary = ProjectImportSummary::default();
            for entry in decoded {
                let relation = entry.relation.target(table_prefix);
                let batches = entry
                    .batches
                    .into_iter()
                    .map(|batch| {
                        if entry.relation == ProjectRelation::Managed(Table::VideoAssets) {
                            ProjectBundle::prepare_video_asset_import(batch, table_prefix)
                        } else if entry.relation == ProjectRelation::Managed(Table::FeatureRegistry)
                        {
                            WorkflowBundleRegistry::retarget_batch(batch, table_prefix)
                        } else {
                            Ok(batch)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                summary.record_relation(
                    batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64
                );
                if matches!(entry.relation, ProjectRelation::Managed(_)) {
                    connection.replace_batches(&relation, batches).await?;
                } else {
                    connection
                        .replace_or_create_batches(&relation, batches)
                        .await?;
                }
            }
            Ok(summary)
        }
        .await;
        let (mut summary, reconnect_warning) = connection
            .finish_bundle_transaction(outcome, "project import")
            .await?;
        if let Some(warning) = reconnect_warning {
            summary.push_warning(warning);
            summary.push_warning(
                "project rows committed, but derived video reconciliation was skipped because a post-commit connection could not be opened"
                    .to_owned(),
            );
            return Ok(summary);
        }
        // Dynamic Workflow relations may be created inside the import
        // transaction, and Spiral's SQL binder cannot resolve a new relation
        // until the commit is published. Keep the project_store's exclusive
        // checkout while reconciling derived MP4 resources immediately after
        // the durable rows commit.
        if let Err(error) = ProjectBundle::reconcile_video_dataset(
            connection,
            table_prefix,
            &imported_video_episodes,
        )
        .await
        {
            summary.push_warning(format!(
                "project rows committed, but the derived video dataset needs repair: {error}"
            ));
        }
        Ok(summary)
    }

    fn with_empty_managed_relations(
        mut decoded: Vec<DecodedProjectRelation>,
    ) -> Vec<DecodedProjectRelation> {
        let mut complete = Vec::new();
        for table in ManagedRelationCatalog::tables() {
            if let Some(index) = decoded
                .iter()
                .position(|entry| entry.relation == ProjectRelation::Managed(table))
            {
                complete.push(decoded.remove(index));
            } else {
                complete.push(DecodedProjectRelation {
                    relation: ProjectRelation::Managed(table),
                    batches: vec![RecordBatch::new_empty(Arc::new(
                        ManagedRelationCatalog::schema(table),
                    ))],
                });
            }
        }
        complete.append(&mut decoded);
        complete
    }
}

#[derive(Clone, Copy)]
struct ArrowDecodeLimits {
    decoded_bytes: usize,
    decoded_rows: usize,
    relation_rows: usize,
    record_batch_rows: usize,
    schema_fields: usize,
    message_metadata_bytes: usize,
}

const ARROW_DECODE_LIMITS: ArrowDecodeLimits = ArrowDecodeLimits {
    decoded_bytes: MAX_ARROW_DECODED_BYTES,
    decoded_rows: MAX_ARROW_DECODED_ROWS,
    relation_rows: MAX_ARROW_DECODED_ROWS,
    record_batch_rows: MAX_ARROW_RECORD_BATCH_ROWS,
    schema_fields: MAX_ARROW_SCHEMA_FIELDS,
    message_metadata_bytes: MAX_ARROW_MESSAGE_METADATA_BYTES,
};

struct ArrowPayload<'a> {
    relation: &'a str,
    bytes: Vec<u8>,
}

struct ArrowPreflight<'a> {
    relation: &'a str,
    body: &'a [u8],
    limits: ArrowDecodeLimits,
}

impl<'a> ArrowPreflight<'a> {
    const fn new(relation: &'a str, limits: ArrowDecodeLimits) -> Self {
        Self {
            relation,
            body: &[],
            limits,
        }
    }

    const fn with_body(mut self, body: &'a [u8]) -> Self {
        self.body = body;
        self
    }
}

#[derive(Default)]
struct ArrowPreflightCounters {
    claimed_decoded_bytes: usize,
    rows: usize,
}

impl<'a> ArrowPayload<'a> {
    fn new(relation: &'a str, bytes: Vec<u8>) -> Self {
        Self { relation, bytes }
    }
}

#[derive(Default)]
struct ArrowDecodeBudget {
    claimed_decoded_bytes: usize,
    rows: usize,
}

impl ArrowDecodeBudget {
    fn decode(&mut self, payload: ArrowPayload<'_>) -> Result<Vec<RecordBatch>, String> {
        self.decode_with_limits(payload, ARROW_DECODE_LIMITS)
    }

    fn decode_with_row_limit(
        &mut self,
        payload: ArrowPayload<'_>,
        relation_rows: usize,
    ) -> Result<Vec<RecordBatch>, String> {
        self.decode_with_limits(
            payload,
            ArrowDecodeLimits {
                relation_rows,
                ..ARROW_DECODE_LIMITS
            },
        )
    }

    fn decode_with_limits(
        &mut self,
        payload: ArrowPayload<'_>,
        limits: ArrowDecodeLimits,
    ) -> Result<Vec<RecordBatch>, String> {
        self.preflight(&payload, limits)?;
        let ArrowPayload { relation, bytes } = payload;
        // The preflight above bounds every IPC buffer by its declared decoded
        // length before Arrow can allocate or decompress it. Do not use
        // `RecordBatch::get_array_memory_size` for this bound: the stream reader
        // exposes column buffers as slices of one shared message-body allocation,
        // and that estimate counts the shared capacity once for every slice.
        let reader = StreamReader::try_new(Cursor::new(bytes), None)
            .map_err(|error| format!("opening Arrow stream for {relation}: {error}"))?;
        let schema = reader.schema();
        let mut batches = Vec::new();
        for batch in reader {
            let batch =
                batch.map_err(|error| format!("reading Arrow stream for {relation}: {error}"))?;
            batches.push(batch);
        }
        if batches.is_empty() {
            batches.push(RecordBatch::new_empty(schema));
        }
        if batches
            .iter()
            .any(|batch| batch.schema() != batches[0].schema())
        {
            return Err(format!(
                "project relation {relation} changes schema between Arrow batches"
            ));
        }
        Ok(batches)
    }

    fn preflight(
        &mut self,
        payload: &ArrowPayload<'_>,
        limits: ArrowDecodeLimits,
    ) -> Result<(), String> {
        let relation = payload.relation;
        let payload = payload.bytes.as_slice();
        let mut position = 0_usize;
        let mut counters = ArrowPreflightCounters::default();
        while position < payload.len() {
            let first = BundleCodec::read_arrow_u32(payload, &mut position, relation)?;
            let metadata_len = if first == u32::MAX {
                BundleCodec::read_arrow_u32(payload, &mut position, relation)?
            } else {
                first
            };
            if metadata_len == 0 {
                if position != payload.len() {
                    return Err(format!(
                        "project Arrow stream for {relation} has bytes after its end marker"
                    ));
                }
                break;
            }
            let metadata_len = usize::try_from(metadata_len)
                .expect("Arrow metadata length always fits usize on supported targets");
            if metadata_len > limits.message_metadata_bytes {
                return Err(format!(
                    "project Arrow metadata for {relation} exceeds the {} MiB message limit",
                    limits.message_metadata_bytes / (1024 * 1024),
                ));
            }
            counters.claimed_decoded_bytes = counters
            .claimed_decoded_bytes
            .checked_add(metadata_len)
            .filter(|bytes| *bytes <= limits.decoded_bytes)
            .ok_or_else(|| {
                format!(
                    "project Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                    limits.decoded_bytes / (1024 * 1024),
                )
            })?;
            let metadata_end = position
                .checked_add(metadata_len)
                .filter(|end| *end <= payload.len())
                .ok_or_else(|| format!("project Arrow metadata for {relation} is truncated"))?;
            let message = arrow_ipc::root_as_message(&payload[position..metadata_end])
                .map_err(|error| format!("opening Arrow metadata for {relation}: {error}"))?;
            position = metadata_end;

            let body_len = usize::try_from(message.bodyLength()).map_err(|_| {
                format!("project Arrow message for {relation} has an invalid body length")
            })?;
            if body_len > limits.decoded_bytes {
                return Err(format!(
                    "project Arrow message body for {relation} exceeds the {} MiB decoded-memory limit",
                    limits.decoded_bytes / (1024 * 1024),
                ));
            }
            let body_end = position
                .checked_add(body_len)
                .filter(|end| *end <= payload.len())
                .ok_or_else(|| format!("project Arrow message body for {relation} is truncated"))?;
            let body = &payload[position..body_end];

            match message.header_type() {
                arrow_ipc::MessageHeader::Schema => {
                    let schema = message.header_as_schema().ok_or_else(|| {
                        format!("project Arrow schema metadata for {relation} is invalid")
                    })?;
                    ArrowPreflight::new(relation, limits).validate_schema(schema)?;
                }
                arrow_ipc::MessageHeader::RecordBatch => {
                    let batch = message.header_as_record_batch().ok_or_else(|| {
                        format!("project Arrow record metadata for {relation} is invalid")
                    })?;
                    ArrowPreflight::new(relation, limits)
                        .with_body(body)
                        .validate_record_batch(batch, &mut counters)?;
                }
                arrow_ipc::MessageHeader::DictionaryBatch => {
                    let dictionary = message.header_as_dictionary_batch().ok_or_else(|| {
                        format!("project Arrow dictionary metadata for {relation} is invalid")
                    })?;
                    let batch = dictionary.data().ok_or_else(|| {
                        format!("project Arrow dictionary for {relation} has no record metadata")
                    })?;
                    ArrowPreflight::new(relation, limits)
                        .with_body(body)
                        .validate_record_batch(batch, &mut counters)?;
                }
                _ => {}
            }
            position = body_end;
        }

        if counters.rows > limits.relation_rows {
            return Err(format!(
                "project Arrow relation {relation} exceeds the {}-row relation limit",
                limits.relation_rows,
            ));
        }
        self.claimed_decoded_bytes = self
        .claimed_decoded_bytes
        .checked_add(counters.claimed_decoded_bytes)
        .filter(|bytes| *bytes <= limits.decoded_bytes)
        .ok_or_else(|| {
            format!(
                "project Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                limits.decoded_bytes / (1024 * 1024),
            )
        })?;
        self.rows = self
            .rows
            .checked_add(counters.rows)
            .filter(|rows| *rows <= limits.decoded_rows)
            .ok_or_else(|| {
                format!(
                    "project Arrow data exceeds the {}-row decoded limit at relation {relation}",
                    limits.decoded_rows,
                )
            })?;
        Ok(())
    }
}

struct BundleCodec;

impl BundleCodec {
    fn read_arrow_u32(payload: &[u8], position: &mut usize, relation: &str) -> Result<u32, String> {
        let end = position
            .checked_add(4)
            .filter(|end| *end <= payload.len())
            .ok_or_else(|| format!("Arrow stream for {relation} has a truncated message prefix"))?;
        let value = u32::from_le_bytes(
            payload[*position..end]
                .try_into()
                .expect("Arrow prefix has exactly four bytes"),
        );
        *position = end;
        Ok(value)
    }
}

impl ArrowPreflight<'_> {
    fn validate_schema(&self, schema: arrow_ipc::Schema<'_>) -> Result<(), String> {
        let relation = self.relation;
        let max_fields = self.limits.schema_fields;
        let Some(fields) = schema.fields() else {
            return Ok(());
        };
        if fields.len() > max_fields {
            return Err(format!(
                "project Arrow schema for {relation} exceeds the {max_fields}-column limit"
            ));
        }
        let mut pending = fields.iter().collect::<Vec<_>>();
        let mut total = 0_usize;
        while let Some(field) = pending.pop() {
            total = total
                .checked_add(1)
                .filter(|count| *count <= max_fields)
                .ok_or_else(|| {
                    format!(
                        "project Arrow schema for {relation} exceeds the {max_fields}-field limit"
                    )
                })?;
            if let Some(children) = field.children() {
                pending.extend(children.iter());
            }
        }
        Ok(())
    }

    fn validate_record_batch(
        &self,
        batch: arrow_ipc::RecordBatch<'_>,
        counters: &mut ArrowPreflightCounters,
    ) -> Result<(), String> {
        let relation = self.relation;
        let body = self.body;
        let limits = self.limits;
        let batch_rows = usize::try_from(batch.length()).map_err(|_| {
            format!("project Arrow record batch for {relation} has an invalid row count")
        })?;
        if batch_rows > limits.record_batch_rows {
            return Err(format!(
                "project Arrow record batch for {relation} exceeds the {}-row batch limit",
                limits.record_batch_rows,
            ));
        }
        counters.rows = counters.rows.checked_add(batch_rows).ok_or_else(|| {
            format!("project Arrow row count overflows while reading relation {relation}")
        })?;

        let compressed = batch.compression().is_some();
        for buffer in batch.buffers().into_iter().flatten() {
            let offset = usize::try_from(buffer.offset()).map_err(|_| {
                format!("project Arrow buffer for {relation} has an invalid offset")
            })?;
            let length = usize::try_from(buffer.length()).map_err(|_| {
                format!("project Arrow buffer for {relation} has an invalid length")
            })?;
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= body.len())
                .ok_or_else(|| {
                    format!("project Arrow buffer for {relation} exceeds its message body")
                })?;
            let decoded_length = if length == 0 || !compressed {
                length
            } else {
                if length < 8 {
                    return Err(format!(
                        "project compressed Arrow buffer for {relation} has no length prefix"
                    ));
                }
                let declared = i64::from_le_bytes(
                    body[offset..offset + 8]
                        .try_into()
                        .expect("compressed Arrow prefix has exactly eight bytes"),
                );
                match declared {
                -1 => length - 8,
                0.. => usize::try_from(declared).map_err(|_| {
                    format!(
                        "project compressed Arrow buffer for {relation} is too large for this machine"
                    )
                })?,
                _ => {
                    return Err(format!(
                        "project compressed Arrow buffer for {relation} has an invalid decoded length"
                    ));
                }
            }
            };
            counters.claimed_decoded_bytes = counters
            .claimed_decoded_bytes
            .checked_add(decoded_length)
            .filter(|bytes| *bytes <= limits.decoded_bytes)
                .ok_or_else(|| {
                    format!(
                        "project Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                    limits.decoded_bytes / (1024 * 1024),
                )
            })?;
            debug_assert!(end <= body.len());
        }
        Ok(())
    }
}

impl ProjectBundle {
    #[cfg(test)]
    fn decode_managed_entries(
        &self,
        entries: Vec<(Table, Vec<u8>)>,
    ) -> Result<Vec<(Table, Vec<RecordBatch>)>, String> {
        let mut budget = ArrowDecodeBudget::default();
        self.decode_managed_entries_with_budget(entries, &mut budget)
    }

    fn decode_managed_entries_with_budget(
        &self,
        entries: Vec<(Table, Vec<u8>)>,
        budget: &mut ArrowDecodeBudget,
    ) -> Result<Vec<(Table, Vec<RecordBatch>)>, String> {
        let mut decoded = Vec::new();
        for (table, payload) in entries {
            let batches = budget.decode(ArrowPayload::new(table.name(), payload))?;
            let schema = batches[0].schema();
            ManagedRelationCatalog::validate(table, schema.as_ref(), "project")?;
            decoded.push((table, batches));
        }
        Ok(decoded)
    }
}

/// Composes the schema sections owned by each feature.
///
/// The transfer mechanism iterates feature implementations and never declares
/// a feature relation's columns itself.
pub(crate) struct ManagedRelationCatalog;

impl ManagedRelationCatalog {
    pub(crate) fn tables() -> Vec<Table> {
        let database = DatabaseBundleSection;
        let episode_relations = EpisodeRelations;
        let settings = StudioPreferenceBundleSection;
        let training = TrainingBundleSection;
        let rollout = RolloutBundleSection;
        let workflow = WorkflowDocumentBundleSection;
        let workflow_features = WorkflowFeatureBundleSection;
        let sections: [&dyn ManagedRelationSchema; 7] = [
            &database,
            &episode_relations,
            &settings,
            &training,
            &rollout,
            &workflow,
            &workflow_features,
        ];
        let mut tables = sections
            .into_iter()
            .flat_map(ManagedRelationSchema::relations)
            .copied()
            .collect::<Vec<_>>();
        tables.sort_by_key(|table| {
            Table::ALL
                .iter()
                .position(|candidate| candidate == table)
                .unwrap_or(usize::MAX)
        });
        tables
    }

    pub(crate) fn schema(table: Table) -> Schema {
        let database = DatabaseBundleSection;
        let episode_relations = EpisodeRelations;
        let settings = StudioPreferenceBundleSection;
        let training = TrainingBundleSection;
        let rollout = RolloutBundleSection;
        let workflow = WorkflowDocumentBundleSection;
        let workflow_features = WorkflowFeatureBundleSection;
        let sections: [&dyn ManagedRelationSchema; 7] = [
            &database,
            &episode_relations,
            &settings,
            &training,
            &rollout,
            &workflow,
            &workflow_features,
        ];
        sections
            .into_iter()
            .find_map(|section| section.managed_relation_schema(table))
            .unwrap_or_else(|| panic!("managed relation {table} has no owning schema section"))
    }

    pub(crate) fn validate(table: Table, schema: &Schema, label: &str) -> Result<(), String> {
        let database = DatabaseBundleSection;
        let episode_relations = EpisodeRelations;
        let settings = StudioPreferenceBundleSection;
        let training = TrainingBundleSection;
        let rollout = RolloutBundleSection;
        let workflow = WorkflowDocumentBundleSection;
        let workflow_features = WorkflowFeatureBundleSection;
        let sections: [&dyn ManagedRelationSchema; 7] = [
            &database,
            &episode_relations,
            &settings,
            &training,
            &rollout,
            &workflow,
            &workflow_features,
        ];
        sections
            .into_iter()
            .find_map(|section| section.validate_managed_relation_schema(table, schema, label))
            .unwrap_or_else(|| {
                Err(format!(
                    "managed relation {table} has no owning schema section"
                ))
            })
    }

    fn table_named(name: &str) -> Option<Table> {
        Self::tables()
            .into_iter()
            .find(|table| table.name() == name)
    }

    #[cfg(test)]
    fn sort(decoded: &mut [(Table, Vec<RecordBatch>)]) {
        decoded.sort_by_key(|(table, _)| {
            Table::ALL
                .iter()
                .position(|candidate| candidate == table)
                .unwrap_or(usize::MAX)
        });
    }
}

impl ProjectBundle {
    fn decode_entries(
        entries: Vec<(String, Vec<u8>)>,
    ) -> Result<Vec<DecodedProjectRelation>, String> {
        let mut budget = ArrowDecodeBudget::default();
        let mut payloads = entries.into_iter().collect::<BTreeMap<_, _>>();
        let mut registry_batches = payloads
            .remove(Table::FeatureRegistry.name())
            .map(|payload| {
                let batches = budget.decode_with_row_limit(
                    ArrowPayload::new(Table::FeatureRegistry.name(), payload),
                    MAX_FEATURE_REGISTRY_ROWS,
                )?;
                ManagedRelationCatalog::validate(
                    Table::FeatureRegistry,
                    batches[0].schema().as_ref(),
                    "project",
                )?;
                Ok::<_, String>(batches)
            })
            .transpose()?;
        let registry = registry_batches
            .as_deref()
            .map(WorkflowBundleRegistry::validate)
            .transpose()?
            .unwrap_or_default();
        let declared_caches = registry.cache_relations();
        let dynamic_names = payloads
            .keys()
            .filter(|name| ManagedRelationCatalog::table_named(name).is_none())
            .cloned()
            .collect::<BTreeSet<_>>();
        if let Some(name) = dynamic_names.difference(&declared_caches).next() {
            return Err(format!(
                "project contains undeclared Workflow relation {name:?}"
            ));
        }
        if let Some(name) = declared_caches.difference(&dynamic_names).next() {
            return Err(format!(
                "project is missing declared Workflow cache relation {name:?}"
            ));
        }

        let mut decoded = Vec::new();
        for table in ManagedRelationCatalog::tables() {
            if table == Table::FeatureRegistry {
                if let Some(batches) = registry_batches.take() {
                    decoded.push(DecodedProjectRelation {
                        relation: ProjectRelation::Managed(table),
                        batches,
                    });
                }
                continue;
            }
            if let Some(payload) = payloads.remove(table.name()) {
                let mut relation = ProjectBundle
                    .decode_managed_entries_with_budget(vec![(table, payload)], &mut budget)?;
                if table == Table::WorkflowRuns {
                    AssistantBundleHistory::validate_batches(&relation[0].1)?;
                }
                decoded.push(DecodedProjectRelation {
                    relation: ProjectRelation::Managed(table),
                    batches: relation.pop().expect("one project relation was decoded").1,
                });
            }
        }
        for bundle_relation in declared_caches {
            let payload = payloads
                .remove(&bundle_relation)
                .expect("declared project cache presence was checked");
            let batches = budget.decode_with_row_limit(
                ArrowPayload::new(&bundle_relation, payload),
                MAX_FEATURE_CACHE_ROWS,
            )?;
            WorkflowCacheValidator::validate_bundle(&bundle_relation, &batches, &registry)?;
            let source_relation = registry
                .source_for_cache(&bundle_relation)
                .expect("a declared cache has a registered source")
                .to_owned();
            decoded.push(DecodedProjectRelation {
                relation: ProjectRelation::WorkflowFeatureCache {
                    bundle_relation,
                    source_relation,
                },
                batches,
            });
        }
        debug_assert!(payloads.is_empty());
        Ok(decoded)
    }
}

struct AssistantBundleHistory;

impl AssistantBundleHistory {
    fn validate_batches(batches: &[RecordBatch]) -> Result<(), String> {
        let mut keys = BTreeSet::new();
        for batch in batches {
            for row in 0..batch.num_rows() {
                let run_id = batch
                    .column_by_name("run_id")
                    .and_then(|column| WorkflowArrow::string_value(column.as_ref(), row))
                    .ok_or_else(|| "project Workflow history has an invalid run_id".to_owned())?;
                if !matches!(run_id, "default" | "agent:default") {
                    return Err(format!(
                        "project Workflow history contains unknown document key {run_id:?}"
                    ));
                }
                if !keys.insert(run_id.to_owned()) {
                    return Err(format!(
                        "project Workflow history contains duplicate document key {run_id:?}"
                    ));
                }
                if run_id != "agent:default" {
                    continue;
                }
                let snapshot = batch
                    .column_by_name("snapshot_json")
                    .and_then(|column| WorkflowArrow::string_value(column.as_ref(), row))
                    .ok_or_else(|| {
                        "project Assistant history has an invalid snapshot_json".to_owned()
                    })?;
                AssistantBundleHistory::validate_json(snapshot)?;
            }
        }
        Ok(())
    }

    fn validate_json(encoded: &str) -> Result<(), String> {
        crate::workflow_assistant::AssistantDocument::decode(encoded).map(|_| ())
    }
}

impl WorkflowBundleRegistry {
    fn validate(batches: &[RecordBatch]) -> Result<WorkflowBundleRegistry, String> {
        Self::validate_with_limits(
            batches,
            MAX_FEATURE_REGISTRY_ROWS,
            MAX_FEATURE_CACHE_COLUMNS,
        )
    }

    fn validate_for_namespace(
        batches: &[RecordBatch],
        table_prefix: &str,
    ) -> Result<WorkflowBundleRegistry, String> {
        let registry = Self::validate(batches)?;
        if let Some(entry) = registry.entries.values().find(|entry| {
            entry.cache_relation
                != spiral::StorageSchema::workflow_feature_cache_relation(
                    table_prefix,
                    entry.definition.applicability().source_relation(),
                )
        }) {
            return Err(format!(
                "Workflow feature registry cache relation {:?} is outside the configured table prefix {table_prefix:?}",
                entry.cache_relation
            ));
        }
        Ok(registry)
    }

    fn retarget_batch(batch: RecordBatch, table_prefix: &str) -> Result<RecordBatch, String> {
        let source_relations = batch
            .column_by_name("source_relation")
            .ok_or_else(|| "Workflow feature registry lost source_relation".to_owned())?;
        let cache_relations = (0..batch.num_rows())
            .map(|row| {
                WorkflowArrow::string_value(source_relations.as_ref(), row)
                    .map(|source| {
                        spiral::StorageSchema::workflow_feature_cache_relation(table_prefix, source)
                    })
                    .ok_or_else(|| {
                        "Workflow feature registry contains an invalid source_relation".to_owned()
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cache_index = batch
            .schema()
            .index_of("cache_relation")
            .map_err(|_| "Workflow feature registry lost cache_relation".to_owned())?;
        let mut fields = batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        fields[cache_index] = Field::new("cache_relation", DataType::Utf8, false);
        let mut columns = batch.columns().to_vec();
        columns[cache_index] = Arc::new(StringArray::from(cache_relations));
        RecordBatch::try_new(
            Arc::new(Schema::new_with_metadata(
                fields,
                batch.schema().metadata().clone(),
            )),
            columns,
        )
        .map_err(|error| format!("retargeting Workflow feature registry: {error}"))
    }

    fn validate_with_limits(
        batches: &[RecordBatch],
        max_rows: usize,
        max_features_per_cache: usize,
    ) -> Result<WorkflowBundleRegistry, String> {
        let mut registry = WorkflowBundleRegistry::default();
        let mut features_per_cache = BTreeMap::<String, usize>::new();
        let mut cache_sources = BTreeMap::<String, String>::new();
        for batch in batches {
            WorkflowFeatureSchema::validate_registry(batch.schema().as_ref())?;
            for row in 0..batch.num_rows() {
                if registry.entries.len() >= max_rows {
                    return Err(format!(
                        "Workflow feature registry exceeds the bounded limit of {max_rows} definitions"
                    ));
                }
                let source_relation =
                    WorkflowBundleRegistry::required_text(batch, "source_relation", row)?;
                let cache_relation =
                    WorkflowBundleRegistry::required_text(batch, "cache_relation", row)?;
                let feature_name =
                    WorkflowBundleRegistry::required_text(batch, "feature_name", row)?;
                let applicability = WorkflowFeatureApplicability::new(
                    source_relation.clone(),
                    WorkflowBundleRegistry::required_text(batch, "media_field", row)?,
                )
                .with_stream_ids(WorkflowBundleRegistry::stream_ids(batch, row)?);
                let definition = WorkflowFeatureDefinition::new(
                    feature_name.clone(),
                    WorkflowBundleRegistry::required_text(batch, "description", row)?,
                    WorkflowBundleRegistry::required_text(batch, "concept", row)?,
                )
                .with_applicability(applicability);
                QualifiedRelation::try_from(source_relation.as_str()).map_err(|error| {
                    format!("Workflow feature registry has unsafe source relation: {error}")
                })?;
                QualifiedRelation::try_from(cache_relation.as_str()).map_err(|error| {
                    format!("Workflow feature registry has unsafe cache relation: {error}")
                })?;
                if !spiral::StorageSchema::is_workflow_feature_cache_relation(
                    &cache_relation,
                    &source_relation,
                ) {
                    let expected = spiral_robotics_common::workflow::WorkflowFeatureApplicability::derived_cache_name(
                        &source_relation,
                    );
                    return Err(format!(
                        "Workflow feature registry cache relation {cache_relation:?} does not match server-derived name component {expected:?}"
                    ));
                }
                if let Some(existing) = cache_sources.get(&cache_relation) {
                    if existing != &source_relation {
                        return Err(
                            "Workflow feature registry maps one cache identity to multiple sources"
                                .to_owned(),
                        );
                    }
                } else {
                    cache_sources.insert(cache_relation.clone(), source_relation.clone());
                }
                WorkflowBundleRegistry::validate_definition(&definition)?;
                let producer_model =
                    WorkflowBundleRegistry::required_text(batch, "producer_model_id", row)?;
                let producer_repository =
                    WorkflowBundleRegistry::required_text(batch, "producer_repository", row)?;
                let artifact_revision =
                    WorkflowBundleRegistry::required_text(batch, "artifact_revision", row)?;
                let value_semantics =
                    WorkflowBundleRegistry::required_text(batch, "value_semantics", row)?;
                let definition_version =
                    WorkflowBundleRegistry::required_u32(batch, "definition_version", row)?;
                let created_at_ms =
                    WorkflowBundleRegistry::required_u64(batch, "created_at_ms", row)?;
                let updated_at_ms =
                    WorkflowBundleRegistry::required_u64(batch, "updated_at_ms", row)?;
                if producer_model != SIGLIP_BASE_MODEL_ID
                    || producer_repository != SIGLIP_BASE_REPOSITORY
                    || artifact_revision != SIGLIP_BASE_REVISION
                    || value_semantics != FEATURE_VALUE_SEMANTICS
                    || definition_version != 1
                    || updated_at_ms < created_at_ms
                {
                    return Err(
                        "Workflow feature registry contains incompatible producer provenance"
                            .to_owned(),
                    );
                }
                let key = (source_relation, feature_name);
                if registry
                    .entries
                    .insert(
                        key,
                        WorkflowBundleRegistryEntry {
                            definition,
                            cache_relation: cache_relation.clone(),
                        },
                    )
                    .is_some()
                {
                    return Err(
                        "Workflow feature registry contains a duplicate source/name definition"
                            .to_owned(),
                    );
                }
                let count = features_per_cache.entry(cache_relation).or_default();
                *count += 1;
                if *count > max_features_per_cache {
                    return Err(format!(
                        "Workflow feature cache exceeds {max_features_per_cache} registered feature columns"
                    ));
                }
            }
        }
        Ok(registry)
    }

    fn validate_definition(definition: &WorkflowFeatureDefinition) -> Result<(), String> {
        let applicability = definition.applicability();
        let streams = applicability.stream_ids().iter().collect::<BTreeSet<_>>();
        if !definition.has_valid_name()
            || definition.description().trim().is_empty()
            || definition.description().chars().count() > MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH
            || definition.concept().trim().is_empty()
            || definition.concept().chars().count() > MAX_SIGLIP_CONCEPT_LENGTH
            || UnquotedSqlIdentifier::try_from(applicability.media_field()).is_err()
            || applicability.stream_ids().len() > MAX_WORKFLOW_FEATURE_STREAMS
            || streams.len() != applicability.stream_ids().len()
            || streams
                .iter()
                .any(|stream| stream.trim().is_empty() || stream.len() > 128)
        {
            return Err(
                "Workflow feature registry contains an invalid feature definition".to_owned(),
            );
        }
        Ok(())
    }

    fn stream_ids(batch: &RecordBatch, row: usize) -> Result<Vec<String>, String> {
        let lists = batch
            .column_by_name("stream_ids")
            .and_then(|column| column.as_any().downcast_ref::<ListArray>())
            .ok_or_else(|| "Workflow feature registry stream_ids is not List<Utf8>".to_owned())?;
        if lists.is_null(row) {
            return Err("Workflow feature registry stream_ids is null".to_owned());
        }
        let values = lists.value(row);
        (0..values.len())
            .map(|index| {
                WorkflowArrow::string_value(values.as_ref(), index)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        "Workflow feature registry contains a null or invalid stream ID".to_owned()
                    })
            })
            .collect()
    }

    fn required_text(batch: &RecordBatch, name: &str, row: usize) -> Result<String, String> {
        batch
            .column_by_name(name)
            .and_then(|column| WorkflowArrow::string_value(column.as_ref(), row))
            .map(str::to_owned)
            .ok_or_else(|| format!("Workflow feature registry field {name:?} is null or invalid"))
    }

    fn required_u32(batch: &RecordBatch, name: &str, row: usize) -> Result<u32, String> {
        batch
            .column_by_name(name)
            .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
            .filter(|values| !values.is_null(row))
            .map(|values| values.value(row))
            .ok_or_else(|| format!("Workflow feature registry field {name:?} is null or invalid"))
    }

    fn required_u64(batch: &RecordBatch, name: &str, row: usize) -> Result<u64, String> {
        batch
            .column_by_name(name)
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
            .filter(|values| !values.is_null(row))
            .map(|values| values.value(row))
            .ok_or_else(|| format!("Workflow feature registry field {name:?} is null or invalid"))
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WorkflowCacheKey {
    episode_id: String,
    stream_id: String,
    segment_index: u32,
    frame_index: u64,
    episode_time_ns: u64,
}

#[derive(Clone, Copy)]
struct WorkflowCacheLimits {
    rows: usize,
    features: usize,
}

impl WorkflowCacheLimits {
    const fn new(rows: usize, features: usize) -> Self {
        Self { rows, features }
    }
}

struct WorkflowCacheValidator<'a> {
    relation: &'a str,
    registered: BTreeMap<String, WorkflowFeatureDefinition>,
    limits: WorkflowCacheLimits,
}

impl<'a> WorkflowCacheValidator<'a> {
    fn new(
        relation: &'a str,
        registry: &WorkflowBundleRegistry,
        limits: WorkflowCacheLimits,
    ) -> Self {
        Self {
            relation,
            registered: registry.features_for_cache(relation),
            limits,
        }
    }
}

impl WorkflowCacheValidator<'_> {
    fn validate_bundle(
        relation: &str,
        batches: &[RecordBatch],
        registry: &WorkflowBundleRegistry,
    ) -> Result<(), String> {
        WorkflowCacheValidator::new(
            relation,
            registry,
            WorkflowCacheLimits::new(MAX_FEATURE_CACHE_ROWS, MAX_FEATURE_CACHE_COLUMNS),
        )
        .validate(batches)
    }
}

impl WorkflowCacheValidator<'_> {
    fn validate(&self, batches: &[RecordBatch]) -> Result<(), String> {
        let relation = self.relation;
        let registered = &self.registered;
        let schema = batches
            .first()
            .map(RecordBatch::schema)
            .ok_or_else(|| format!("Workflow feature cache {relation:?} has no Arrow schema"))?;
        self.validate_schema(schema.as_ref())?;
        let mut keys = BTreeSet::new();
        for batch in batches {
            if batch.schema() != schema {
                return Err(format!(
                    "Workflow feature cache {relation:?} changes schema between batches"
                ));
            }
            for row in 0..batch.num_rows() {
                if keys.len() >= self.limits.rows {
                    return Err(format!(
                        "Workflow feature cache exceeds the bounded limit of {} rows",
                        self.limits.rows,
                    ));
                }
                let key = WorkflowCacheKey::from_batch(batch, row)?;
                if !keys.insert(key) {
                    return Err(
                        "Workflow feature cache contains duplicate stable frame identities"
                            .to_owned(),
                    );
                }
                for feature in registered.keys() {
                    let values = batch
                        .column_by_name(feature)
                        .and_then(|column| column.as_any().downcast_ref::<Float32Array>())
                        .ok_or_else(|| {
                            format!("Workflow feature cache field {feature:?} is not Float32")
                        })?;
                    if !values.is_null(row) {
                        let value = values.value(row);
                        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                            return Err(format!(
                                "Workflow feature cache field {feature:?} violates sigmoid probability semantics"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_schema(&self, schema: &Schema) -> Result<(), String> {
        let relation = self.relation;
        let registered = &self.registered;
        if schema.fields().len() < CACHE_KEY_FIELDS.len() {
            return Err(format!(
                "Workflow feature cache {relation:?} is missing stable identity fields"
            ));
        }
        for (index, (name, expected)) in CACHE_KEY_FIELDS.iter().enumerate() {
            let field = schema.field(index);
            if field.name() != name
                || (field.is_nullable() && !WorkflowFeatureSchema::cache_key_may_be_nullable(name))
                || !WorkflowFeatureSchema::storage_type_matches(field.data_type(), expected)
            {
                return Err(format!(
                    "Workflow feature cache key field {index} must use compatible {name} {expected:?} storage"
                ));
            }
        }
        let feature_fields = schema
            .fields()
            .iter()
            .skip(CACHE_KEY_FIELDS.len())
            .collect::<Vec<_>>();
        if feature_fields.len() > self.limits.features {
            return Err(format!(
                "Workflow feature cache exceeds {} feature columns",
                self.limits.features,
            ));
        }
        let actual = feature_fields
            .iter()
            .map(|field| field.name().clone())
            .collect::<BTreeSet<_>>();
        let expected = registered.keys().cloned().collect::<BTreeSet<_>>();
        if actual.len() != feature_fields.len() || actual != expected {
            return Err(format!(
                "Workflow feature cache {relation:?} feature columns do not exactly match its registry definitions"
            ));
        }
        for field in feature_fields {
            if field.data_type() != &DataType::Float32 || !field.is_nullable() {
                return Err(format!(
                    "Workflow feature cache field {:?} must be nullable Float32",
                    field.name()
                ));
            }
            // Spiral/Vortex intentionally drops arbitrary Arrow field metadata.
            // The registry and cache are restored atomically, so the validated
            // registry row is the durable producer/definition authority.
        }
        Ok(())
    }
}

impl WorkflowCacheKey {
    fn from_batch(batch: &RecordBatch, row: usize) -> Result<WorkflowCacheKey, String> {
        let columns = ArrowColumns::new(batch, "Workflow feature cache key");
        let u32_value = |name: &str| {
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
                .filter(|values| !values.is_null(row))
                .map(|values| values.value(row))
                .ok_or_else(|| format!("Workflow feature cache key {name:?} is null or invalid"))
        };
        let u64_value = |name: &str| {
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
                .filter(|values| !values.is_null(row))
                .map(|values| values.value(row))
                .ok_or_else(|| format!("Workflow feature cache key {name:?} is null or invalid"))
        };
        Ok(WorkflowCacheKey {
            episode_id: columns.text("episode_id", row)?.to_owned(),
            stream_id: columns.text("stream_id", row)?.to_owned(),
            segment_index: u32_value("segment_index")?,
            frame_index: u64_value("frame_index")?,
            episode_time_ns: u64_value("episode_time_ns")?,
        })
    }
}

impl WorkflowArrow {
    fn string_value(array: &dyn arrow_array::Array, row: usize) -> Option<&str> {
        if array.is_null(row) {
            return None;
        }
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            Some(values.value(row))
        } else if let Some(values) = array.as_any().downcast_ref::<LargeStringArray>() {
            Some(values.value(row))
        } else {
            array
                .as_any()
                .downcast_ref::<StringViewArray>()
                .map(|values| values.value(row))
        }
    }
}

impl WorkflowRestorePlan {
    fn from_caches(
        current_caches: &BTreeSet<String>,
        incoming_caches: &BTreeSet<String>,
    ) -> WorkflowRestorePlan {
        WorkflowRestorePlan {
            retire_caches: current_caches
                .difference(incoming_caches)
                .cloned()
                .collect(),
        }
    }
}

#[cfg(test)]
trait ProjectImportTransaction {
    fn begin_project_import(&mut self) -> Result<(), String>;
    fn commit_project_import(&mut self) -> Result<(), String>;
    fn rollback_project_import(&mut self) -> Result<(), String>;
    fn reconnect_project(&mut self) -> Result<(), String>;
    fn discard_project_connection(&mut self);
}

#[cfg(test)]
trait ProjectImportTransactionRunner: ProjectImportTransaction {
    fn run_project_import_transaction<T>(
        &mut self,
        operation: &str,
        mutation: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<(T, Option<String>), String> {
        let connection = self;
        if let Err(error) = connection.begin_project_import() {
            connection.discard_project_connection();
            return Err(format!(
                "starting {operation} transaction failed and its connection was discarded: {error}"
            ));
        }

        let value = match mutation(connection) {
            Ok(value) => value,
            Err(primary) => match connection.rollback_project_import() {
                Ok(()) => return Err(primary),
                Err(rollback) => {
                    connection.discard_project_connection();
                    return Err(format!(
                        "{primary}; rolling back {operation} also failed: {rollback}; connection discarded"
                    ));
                }
            },
        };

        if let Err(commit) = connection.commit_project_import() {
            connection.discard_project_connection();
            return Err(format!(
                "committing {operation} failed with an uncertain outcome; connection discarded: {commit}"
            ));
        }
        match connection.reconnect_project() {
            Ok(()) => Ok((value, None)),
            Err(error) => {
                connection.discard_project_connection();
                Ok((
                    value,
                    Some(format!(
                        "{operation} committed, but its post-commit Spiral connection could not be opened and was discarded: {error}"
                    )),
                ))
            }
        }
    }
}

#[cfg(test)]
impl<C: ProjectImportTransaction> ProjectImportTransactionRunner for C {}

impl ProjectConnection {
    async fn begin_bundle_transaction(&mut self, operation: &str) -> Result<(), String> {
        if let Err(error) = self.begin_transaction().await {
            self.discard();
            return Err(format!(
                "starting {operation} transaction failed and its connection was discarded: {error}"
            ));
        }
        Ok(())
    }

    async fn finish_bundle_transaction<T>(
        &mut self,
        outcome: Result<T, String>,
        operation: &str,
    ) -> Result<(T, Option<String>), String> {
        let value = match outcome {
            Ok(value) => value,
            Err(primary) => match self.rollback().await {
                Ok(()) => return Err(primary),
                Err(rollback) => {
                    self.discard();
                    return Err(format!(
                        "{primary}; rolling back {operation} also failed: {rollback}; connection discarded"
                    ));
                }
            },
        };

        if let Err(commit) = self.commit().await {
            self.discard();
            return Err(format!(
                "committing {operation} failed with an uncertain outcome; connection discarded: {commit}"
            ));
        }
        match self.reconnect().await {
            Ok(()) => Ok((value, None)),
            Err(error) => {
                self.discard();
                Ok((
                    value,
                    Some(format!(
                        "{operation} committed, but its post-commit Spiral connection could not be opened and was discarded: {error}"
                    )),
                ))
            }
        }
    }
}

impl ProjectBundle {
    fn video_episode_ids(
        decoded: &[(Table, Vec<RecordBatch>)],
    ) -> Result<BTreeSet<String>, String> {
        let mut episode_ids = BTreeSet::new();
        for (_, batches) in decoded
            .iter()
            .filter(|(table, _)| *table == Table::VideoAssets)
        {
            for batch in batches {
                for episode_id in ProjectBundle::utf8_values(batch, column::EPISODE_ID)? {
                    ProjectBundle::validate_episode_id(&episode_id)?;
                    episode_ids.insert(episode_id);
                }
            }
        }
        Ok(episode_ids)
    }

    async fn reconcile_video_dataset(
        connection: &mut ProjectConnection,
        table_prefix: &str,
        imported: &BTreeSet<String>,
    ) -> Result<(), String> {
        let dataset_exists = connection.video_dataset_exists(table_prefix).await?;
        if imported.is_empty() {
            connection.drop_video_dataset(table_prefix).await?;
        } else {
            connection
                .rebuild_video_dataset_internal(table_prefix, dataset_exists)
                .await?;
        }
        Ok(())
    }

    fn utf8_values(batch: &RecordBatch, name: &str) -> Result<Vec<String>, String> {
        let index = batch
            .schema()
            .index_of(name)
            .map_err(|error| format!("project is missing column {name}: {error}"))?;
        let values = batch.column(index);
        if let Some(values) = values.as_any().downcast_ref::<StringArray>() {
            return ProjectBundle::collect_utf8(values.iter(), name);
        }
        if let Some(values) = values.as_any().downcast_ref::<LargeStringArray>() {
            return ProjectBundle::collect_utf8(values.iter(), name);
        }
        if let Some(values) = values.as_any().downcast_ref::<StringViewArray>() {
            return ProjectBundle::collect_utf8(values.iter(), name);
        }
        Err(format!("project column {name} is not UTF-8 text"))
    }

    fn collect_utf8<'a>(
        values: impl Iterator<Item = Option<&'a str>>,
        name: &str,
    ) -> Result<Vec<String>, String> {
        values
            .map(|value| {
                value
                    .map(str::to_owned)
                    .ok_or_else(|| format!("project column {name} contains a null value"))
            })
            .collect()
    }

    fn retarget_video_media_relations(
        batch: RecordBatch,
        table_prefix: &str,
    ) -> Result<RecordBatch, String> {
        let media_relation = SpiralSql::video_dataset_output(table_prefix);
        let index = batch
            .schema()
            .index_of(column::MEDIA_RELATION)
            .map_err(|error| {
                format!(
                    "project is missing column {}: {error}",
                    column::MEDIA_RELATION
                )
            })?;
        let mut columns = batch.columns().to_vec();
        let source = &columns[index];
        columns[index] = if source.as_any().is::<StringArray>() {
            Arc::new(StringArray::from(vec![
                media_relation.as_str();
                batch.num_rows()
            ])) as ArrayRef
        } else if source.as_any().is::<LargeStringArray>() {
            Arc::new(LargeStringArray::from(vec![
                media_relation.as_str();
                batch.num_rows()
            ])) as ArrayRef
        } else if source.as_any().is::<StringViewArray>() {
            Arc::new(StringViewArray::from(vec![
                media_relation.as_str();
                batch.num_rows()
            ])) as ArrayRef
        } else {
            return Err(format!(
                "project column {} is not UTF-8 text",
                column::MEDIA_RELATION
            ));
        };
        RecordBatch::try_new(batch.schema(), columns)
            .map_err(|error| format!("retargeting project media relations: {error}"))
    }

    fn prepare_video_asset_import(
        batch: RecordBatch,
        table_prefix: &str,
    ) -> Result<RecordBatch, String> {
        ProjectBundle::retarget_video_media_relations(
            EpisodeRelations::normalize_video_asset(batch)?,
            table_prefix,
        )
    }

    fn validate_episode_id(episode_id: &str) -> Result<(), String> {
        if episode_id.is_empty()
            || episode_id.len() > 128
            || !episode_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(format!(
                "project contains invalid episode ID {episode_id:?}"
            ));
        }
        Ok(())
    }
}

impl ProjectBundle {
    #[cfg(test)]
    fn parse_managed_entries(&self, bundle: &[u8]) -> Result<Vec<(Table, Vec<u8>)>, String> {
        self.parse_raw(bundle)?
            .into_iter()
            .map(|(name, payload)| {
                ManagedRelationCatalog::table_named(&name)
                    .map(|table| (table, payload))
                    .ok_or_else(|| format!("project contains unknown relation {name:?}"))
            })
            .collect()
    }

    fn parse_raw(&self, bundle: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
        if bundle.len() > Self::MAX_BYTES {
            return Err(format!(
                "project file exceeds the {} MiB import limit",
                Self::MAX_BYTES / (1024 * 1024),
            ));
        }
        let mut source = Cursor::new(bundle);
        let mut magic = [0_u8; 8];
        source
            .read_exact(&mut magic)
            .map_err(|_| "project file is truncated".to_owned())?;
        if &magic != PROJECT_MAGIC {
            return Err("file is not a Spiral Robotics project".to_owned());
        }
        let manifest_len = BundleCodec::read_u32(&mut source)? as usize;
        if manifest_len == 0 || manifest_len > MAX_MANIFEST_BYTES {
            return Err("project manifest has an invalid size".to_owned());
        }
        let mut manifest = vec![0_u8; manifest_len];
        source
            .read_exact(&mut manifest)
            .map_err(|_| "project manifest is truncated".to_owned())?;
        let manifest: Manifest = serde_json::from_slice(&manifest)
            .map_err(|error| format!("project manifest is invalid: {error}"))?;
        if manifest.format != PROJECT_FORMAT || manifest.format_version != FORMAT_VERSION {
            return Err("project file uses an unsupported format version".to_owned());
        }
        if manifest.schema_version != spiral::SCHEMA_VERSION {
            return Err(format!(
                "project schema {} cannot be imported by schema {}",
                manifest.schema_version,
                spiral::SCHEMA_VERSION
            ));
        }

        let relation_count = manifest.relations.len();
        let mut expected = manifest.relations.into_iter().collect::<BTreeSet<_>>();
        if expected.len() != relation_count {
            return Err("project manifest contains duplicate relations".to_owned());
        }
        let mut seen = BTreeSet::new();
        let mut entries = BTreeMap::new();
        loop {
            let name_len = BundleCodec::read_u16(&mut source)? as usize;
            if name_len == 0 {
                break;
            }
            if name_len > MAX_ENTRY_NAME_BYTES {
                return Err("project relation name is too long".to_owned());
            }
            let mut name = vec![0_u8; name_len];
            source
                .read_exact(&mut name)
                .map_err(|_| "project relation name is truncated".to_owned())?;
            let name = String::from_utf8(name)
                .map_err(|_| "project relation name is not UTF-8".to_owned())?;
            if !expected.remove(&name) || !seen.insert(name.clone()) {
                return Err(format!(
                    "project contains unexpected or duplicate relation {name:?}"
                ));
            }
            let payload_len = BundleCodec::read_u64(&mut source)?;
            let payload_len = usize::try_from(payload_len)
                .map_err(|_| format!("project relation {name:?} is too large for this machine"))?;
            let remaining = bundle.len().saturating_sub(source.position() as usize);
            if payload_len > remaining {
                return Err(format!("project relation {name:?} is truncated"));
            }
            let mut payload = vec![0_u8; payload_len];
            source
                .read_exact(&mut payload)
                .map_err(|_| format!("project relation {name:?} is truncated"))?;
            entries.insert(name, payload);
        }
        if !expected.is_empty() {
            return Err(format!(
                "project is missing relations: {}",
                expected.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        if source.position() != bundle.len() as u64 {
            return Err("project file has trailing bytes".to_owned());
        }
        Ok(entries.into_iter().collect())
    }
}

struct LimitedPayload {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedPayload {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl io::Write for LimitedPayload {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(format!(
                "project export exceeds the {} MiB portable-file limit",
                self.limit / (1024 * 1024),
            )));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl BundleCodec {
    fn write_u16(target: &mut Vec<u8>, value: usize) -> Result<(), String> {
        let value =
            u16::try_from(value).map_err(|_| "project entry name is too long".to_owned())?;
        target.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    fn write_u32(target: &mut Vec<u8>, value: usize) -> Result<(), String> {
        let value = u32::try_from(value).map_err(|_| "project manifest is too large".to_owned())?;
        target.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    fn write_u64(target: &mut Vec<u8>, value: usize) -> Result<(), String> {
        target.extend_from_slice(&(value as u64).to_le_bytes());
        Ok(())
    }
    fn read_u16(source: &mut Cursor<&[u8]>) -> Result<u16, String> {
        let mut bytes = [0; 2];
        source
            .read_exact(&mut bytes)
            .map_err(|_| "project file is truncated".to_owned())?;
        Ok(u16::from_le_bytes(bytes))
    }
    fn read_u32(source: &mut Cursor<&[u8]>) -> Result<u32, String> {
        let mut bytes = [0; 4];
        source
            .read_exact(&mut bytes)
            .map_err(|_| "project file is truncated".to_owned())?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn read_u64(source: &mut Cursor<&[u8]>) -> Result<u64, String> {
        let mut bytes = [0; 8];
        source
            .read_exact(&mut bytes)
            .map_err(|_| "project file is truncated".to_owned())?;
        Ok(u64::from_le_bytes(bytes))
    }
}

struct BundleClock;

impl BundleClock {
    fn unix_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use crate::workflow_documents::MAX_WORKFLOW_AGENT_CONVERSATIONS;
    use std::fs;
    use std::io::Write as _;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::Instant;

    use arrow_array::builder::{ListBuilder, StringBuilder};
    use arrow_array::{
        ArrayRef, BinaryArray, Float32Array, Float64Array, Int64Array, LargeBinaryArray, NullArray,
        StringArray, UInt32Array, UInt64Array,
    };
    use arrow_ipc::writer::IpcWriteOptions;
    use arrow_schema::{DataType, Field, Schema};

    use super::*;
    use crate::db::ProjectStore;
    use crate::episodes::{EpisodeFixture, EpisodeStore};
    use crate::manager::StorageManager;
    use crate::settings::{BackendStateStore, ServiceEndpoint, SpiralConnectionSettings};

    const WORKFLOW_TEST_NAMESPACE: &str = "studio";

    fn envelope(manifest: Manifest, entries: &[(&str, &[u8])]) -> Vec<u8> {
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut bundle = Vec::new();
        bundle.extend_from_slice(PROJECT_MAGIC);
        BundleCodec::write_u32(&mut bundle, manifest.len()).unwrap();
        bundle.extend_from_slice(&manifest);
        for (name, payload) in entries {
            BundleCodec::write_u16(&mut bundle, name.len()).unwrap();
            bundle.extend_from_slice(name.as_bytes());
            BundleCodec::write_u64(&mut bundle, payload.len()).unwrap();
            bundle.extend_from_slice(payload);
        }
        bundle.extend_from_slice(&0_u16.to_le_bytes());
        bundle
    }

    fn manifest(relations: Vec<String>) -> Manifest {
        Manifest {
            format: PROJECT_FORMAT.to_owned(),
            format_version: FORMAT_VERSION,
            schema_version: spiral::SCHEMA_VERSION,
            exported_ms: 1,
            relations,
        }
    }

    fn text_batch(columns: &[(&str, &[&str])]) -> RecordBatch {
        let rows = columns.first().map_or(0, |(_, values)| values.len());
        assert!(columns.iter().all(|(_, values)| values.len() == rows));
        record_batch(
            columns
                .iter()
                .map(|(name, _)| Field::new(*name, DataType::Utf8, false))
                .collect(),
            columns
                .iter()
                .map(|(_, values)| Arc::new(StringArray::from(values.to_vec())) as ArrayRef)
                .collect(),
        )
    }

    fn arrow_payload(batch: &RecordBatch) -> Vec<u8> {
        arrow_payload_batches(std::slice::from_ref(batch))
    }

    /// The relation's production columns, all retyped as nulls, so a decode
    /// test exercises the validator on a correctly named but wrongly typed
    /// payload.
    fn null_typed_batch(table: Table, rows: usize) -> RecordBatch {
        let names = ManagedRelationCatalog::schema(table)
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect::<Vec<_>>();
        record_batch(
            names
                .iter()
                .map(|name| Field::new(name, DataType::Null, true))
                .collect(),
            names
                .iter()
                .map(|_| Arc::new(NullArray::new(rows)) as ArrayRef)
                .collect(),
        )
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
                ArrowPayload::new("video_assets", payload.clone()),
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
    fn managed_decode_rejects_repeated_null_batches_at_the_total_row_bound() {
        let batch = null_typed_batch(Table::Datasets, 250_001);
        let payload = arrow_payload_batches(&[batch.clone(), batch.clone(), batch.clone(), batch]);
        assert!(payload.len() < 16 * 1_024);

        let error = ProjectBundle
            .decode_managed_entries(vec![(Table::Datasets, payload)])
            .unwrap_err();

        assert!(
            error.contains("1000000-row") && error.contains("limit"),
            "{error}"
        );
    }

    #[test]
    fn managed_schema_accepts_required_binary_view_for_portable_video_payload() {
        let mut fields = ManagedRelationCatalog::schema(Table::VideoAssets)
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        let payload = fields
            .iter()
            .position(|field| field.name() == column::PAYLOAD)
            .expect("video assets contain a payload field");
        fields[payload] = Field::new(column::PAYLOAD, DataType::BinaryView, false);

        ManagedRelationCatalog::validate(Table::VideoAssets, &Schema::new(fields), "project")
            .expect("a required portable payload safely satisfies the nullable binary contract");
    }

    #[test]
    fn managed_decode_rejects_semantically_wrong_field_types() {
        let batch = null_typed_batch(Table::Datasets, 1);

        let error = ProjectBundle
            .decode_managed_entries(vec![(Table::Datasets, arrow_payload(&batch))])
            .unwrap_err();

        assert!(error.contains("semantic type"), "{error}");
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

    fn agent_message(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "sequence": 1,
            "role": "user",
            "phase": "complete",
            "text": "Inspect relations",
            "tools": [],
            "created_ms": 1
        })
    }

    fn agent_conversation(id: &str) -> serde_json::Value {
        serde_json::json!({
            "conversation_id": id,
            "title": "Inspect relations",
            "created_ms": 1,
            "updated_ms": 1,
            "session": {
                "workflow_id": "default",
                "conversation_id": id,
                "revision": 1,
                "phase": "idle",
                "messages": [agent_message(&format!("message-{id}"))],
                "updated_ms": 1
            },
            "provider_messages": [{"role": "user", "content": "Inspect relations"}]
        })
    }

    #[test]
    fn project_agent_history_validates_selection_identity_and_bounds() {
        let valid = serde_json::json!({
            "schema_version": 2,
            "revision": 3,
            "active_conversation_id": "conversation-a",
            "conversations": [
                agent_conversation("conversation-a"),
                agent_conversation("conversation-b")
            ]
        });
        AssistantBundleHistory::validate_json(&valid.to_string()).unwrap();

        let mut schema_less = valid.clone();
        schema_less
            .as_object_mut()
            .unwrap()
            .remove("schema_version");
        assert!(
            AssistantBundleHistory::validate_json(&schema_less.to_string())
                .unwrap_err()
                .contains("schema 0 is not supported")
        );

        let mut unknown_selection = valid.clone();
        unknown_selection["active_conversation_id"] = serde_json::json!("missing");
        assert!(
            AssistantBundleHistory::validate_json(&unknown_selection.to_string())
                .unwrap_err()
                .contains("active conversation")
        );

        let mut duplicate = valid.clone();
        duplicate["conversations"] =
            serde_json::json!([agent_conversation("same"), agent_conversation("same")]);
        duplicate["active_conversation_id"] = serde_json::json!("same");
        assert!(
            AssistantBundleHistory::validate_json(&duplicate.to_string())
                .unwrap_err()
                .contains("duplicate")
        );

        let mut too_many = Vec::new();
        for index in 0..=MAX_WORKFLOW_AGENT_CONVERSATIONS {
            too_many.push(agent_conversation(&format!("conversation-{index}")));
        }
        let over_limit = serde_json::json!({
            "schema_version": 2,
            "revision": 1,
            "active_conversation_id": "conversation-0",
            "conversations": too_many
        });
        assert!(
            AssistantBundleHistory::validate_json(&over_limit.to_string())
                .unwrap_err()
                .contains("1–20")
        );

        let mut missing_timestamp = valid.clone();
        missing_timestamp["conversations"][0]
            .as_object_mut()
            .unwrap()
            .remove("created_ms");
        assert!(
            AssistantBundleHistory::validate_json(&missing_timestamp.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut unknown_nested = valid.clone();
        unknown_nested["conversations"][0]["session"]["messages"][0]["future_field"] =
            serde_json::json!(true);
        assert!(
            AssistantBundleHistory::validate_json(&unknown_nested.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut unknown_top_level = valid;
        unknown_top_level["future_field"] = serde_json::json!(true);
        assert!(
            AssistantBundleHistory::validate_json(&unknown_top_level.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut foreign_workflow = serde_json::json!({
            "schema_version": 2,
            "revision": 3,
            "active_conversation_id": "conversation-a",
            "conversations": [agent_conversation("conversation-a")]
        });
        foreign_workflow["conversations"][0]["session"]["workflow_id"] = serde_json::json!("other");
        assert!(
            AssistantBundleHistory::validate_json(&foreign_workflow.to_string())
                .unwrap_err()
                .contains("unsupported Workflow")
        );

        for role in ["system", "developer"] {
            let mut injected = serde_json::json!({
                "schema_version": 2,
                "revision": 3,
                "active_conversation_id": "conversation-a",
                "conversations": [agent_conversation("conversation-a")]
            });
            injected["conversations"][0]["provider_messages"][0]["role"] = serde_json::json!(role);
            let error = AssistantBundleHistory::validate_json(&injected.to_string()).unwrap_err();
            assert!(error.contains("invalid role"), "unexpected error: {error}");
        }
    }

    #[test]
    fn project_decode_rejects_invalid_agent_history_before_restore_planning() {
        let invalid = serde_json::json!({
            "schema_version": 2,
            "revision": 1,
            "active_conversation_id": "missing",
            "conversations": [agent_conversation("conversation-a")]
        })
        .to_string();
        let workflow_runs = record_batch(
            vec![
                Field::new("run_id", DataType::Utf8, false),
                Field::new("document_version", DataType::UInt32, false),
                Field::new("snapshot_json", DataType::Utf8, false),
                Field::new("updated_ms", DataType::Int64, false),
            ],
            vec![
                Arc::new(StringArray::from(vec!["agent:default"])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![
                    WORKFLOW_AGENT_DOCUMENT_SCHEMA_VERSION as u32,
                ])) as ArrayRef,
                Arc::new(StringArray::from(vec![invalid])) as ArrayRef,
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
            ],
        );
        let error = match ProjectBundle::decode_entries(vec![(
            Table::WorkflowRuns.name().to_owned(),
            arrow_payload(&workflow_runs),
        )]) {
            Ok(_) => panic!("invalid Assistant history should be rejected before restore"),
            Err(error) => error,
        };
        assert!(error.contains("active conversation"));
    }

    fn workflow_definition(source: &str, name: &str) -> WorkflowFeatureDefinition {
        let applicability = WorkflowFeatureApplicability::new(source, "image")
            .with_stream_ids(vec!["camera_front".to_owned()]);
        WorkflowFeatureDefinition::new(
            name,
            format!("Detect {name}"),
            name.trim_end_matches("_feature").replace('_', " "),
        )
        .with_applicability(applicability)
    }

    fn canonical_registry_batch(definitions: &[WorkflowFeatureDefinition]) -> RecordBatch {
        let text = |value: fn(&WorkflowFeatureDefinition) -> String| {
            Arc::new(StringArray::from_iter_values(definitions.iter().map(value))) as ArrayRef
        };
        let mut streams = ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
            "item",
            DataType::Utf8,
            false,
        )));
        for definition in definitions {
            for stream in definition.applicability().stream_ids() {
                streams.values().append_value(stream);
            }
            streams.append(true);
        }
        let fields = vec![
            Field::new("source_relation", DataType::Utf8, false),
            Field::new("cache_relation", DataType::Utf8, false),
            Field::new("feature_name", DataType::Utf8, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("concept", DataType::Utf8, false),
            Field::new("media_field", DataType::Utf8, false),
            Field::new(
                "stream_ids",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
                false,
            ),
            Field::new("producer_model_id", DataType::Utf8, false),
            Field::new("producer_repository", DataType::Utf8, false),
            Field::new("artifact_revision", DataType::Utf8, false),
            Field::new("value_semantics", DataType::Utf8, false),
            Field::new("definition_version", DataType::UInt32, false),
            Field::new("created_at_ms", DataType::UInt64, false),
            Field::new("updated_at_ms", DataType::UInt64, false),
        ];
        RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            vec![
                text(|definition| definition.applicability().source_relation().to_owned()),
                text(|definition| {
                    spiral::StorageSchema::workflow_feature_cache_relation(
                        WORKFLOW_TEST_NAMESPACE,
                        definition.applicability().source_relation(),
                    )
                }),
                text(|definition| definition.name().to_owned()),
                text(|definition| definition.description().to_owned()),
                text(|definition| definition.concept().to_owned()),
                text(|definition| definition.applicability().media_field().to_owned()),
                Arc::new(streams.finish()) as ArrayRef,
                Arc::new(StringArray::from_iter_values(
                    definitions.iter().map(|_| SIGLIP_BASE_MODEL_ID),
                )),
                Arc::new(StringArray::from_iter_values(
                    definitions.iter().map(|_| SIGLIP_BASE_REPOSITORY),
                )),
                Arc::new(StringArray::from_iter_values(
                    definitions.iter().map(|_| SIGLIP_BASE_REVISION),
                )),
                Arc::new(StringArray::from_iter_values(
                    definitions.iter().map(|_| FEATURE_VALUE_SEMANTICS),
                )),
                Arc::new(UInt32Array::from(vec![1; definitions.len()])),
                Arc::new(UInt64Array::from(vec![10; definitions.len()])),
                Arc::new(UInt64Array::from(vec![20; definitions.len()])),
            ],
        )
        .unwrap()
    }

    fn canonical_cache_batch(
        definitions: &[WorkflowFeatureDefinition],
        frame_indices: &[u64],
    ) -> RecordBatch {
        let mut fields = CACHE_KEY_FIELDS
            .iter()
            .map(|(name, data_type)| Field::new(*name, data_type.clone(), false))
            .collect::<Vec<_>>();
        fields.extend(
            definitions
                .iter()
                .map(|definition| Field::new(definition.name(), DataType::Float32, true)),
        );
        let rows = frame_indices.len();
        let mut columns = vec![
            Arc::new(StringArray::from(vec!["episode_a"; rows])) as ArrayRef,
            Arc::new(StringArray::from(vec!["camera_front"; rows])) as ArrayRef,
            Arc::new(UInt32Array::from(vec![0; rows])) as ArrayRef,
            Arc::new(UInt64Array::from(frame_indices.to_vec())) as ArrayRef,
            Arc::new(UInt64Array::from_iter_values(
                frame_indices.iter().map(|frame| frame * 1_000_000),
            )) as ArrayRef,
        ];
        columns.extend(
            definitions
                .iter()
                .map(|_| Arc::new(Float32Array::from(vec![Some(0.5); rows])) as ArrayRef),
        );
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
    }

    fn replace_batch_column(batch: &RecordBatch, index: usize, column: ArrayRef) -> RecordBatch {
        let mut columns = batch.columns().to_vec();
        columns[index] = column;
        RecordBatch::try_new(batch.schema(), columns).unwrap()
    }

    fn workflow_project_bundle(registry: &RecordBatch, caches: &[(&str, RecordBatch)]) -> Vec<u8> {
        let registry_payload = arrow_payload(registry);
        let cache_payloads = caches
            .iter()
            .map(|(name, batch)| ((*name).to_owned(), arrow_payload(batch)))
            .collect::<Vec<_>>();
        let mut names = vec![Table::FeatureRegistry.name().to_owned()];
        names.extend(cache_payloads.iter().map(|(name, _)| name.clone()));
        let mut entries = vec![(Table::FeatureRegistry.name(), registry_payload.as_slice())];
        entries.extend(
            cache_payloads
                .iter()
                .map(|(name, payload)| (name.as_str(), payload.as_slice())),
        );
        envelope(manifest(names), &entries)
    }

    #[test]
    fn empty_project_envelope_restores_every_managed_relation_as_empty() {
        let bundle = envelope(manifest(Vec::new()), &[]);
        assert_eq!(
            ProjectBundle.parse_managed_entries(&bundle).unwrap(),
            Vec::new()
        );
        let decoded =
            ProjectBundle::decode_entries(ProjectBundle.parse_raw(&bundle).unwrap()).unwrap();
        let complete = ProjectBundle::with_empty_managed_relations(decoded);
        assert_eq!(complete.len(), Table::ALL.len());
        for (entry, table) in complete.iter().zip(Table::ALL) {
            assert_eq!(entry.relation, ProjectRelation::Managed(table));
            assert_eq!(entry.batches.len(), 1);
            assert_eq!(entry.batches[0].num_rows(), 0);
            assert_eq!(
                entry.batches[0].schema().as_ref(),
                &ManagedRelationCatalog::schema(table)
            );
        }
    }

    #[test]
    fn manifest_and_payload_relations_must_match_exactly() {
        let name = Table::Datasets.name().to_owned();
        let missing = envelope(manifest(vec![name.clone()]), &[]);
        assert!(
            ProjectBundle
                .parse_managed_entries(&missing)
                .unwrap_err()
                .contains("missing relations")
        );

        let unexpected = envelope(manifest(Vec::new()), &[(name.as_str(), b"arrow")]);
        assert!(
            ProjectBundle
                .parse_managed_entries(&unexpected)
                .unwrap_err()
                .contains("unexpected or duplicate")
        );
    }

    #[test]
    fn duplicate_manifest_relations_are_rejected() {
        let name = Table::Datasets.name().to_owned();
        let bundle = envelope(manifest(vec![name.clone(), name]), &[]);
        assert!(
            ProjectBundle
                .parse_managed_entries(&bundle)
                .unwrap_err()
                .contains("duplicate relations")
        );
    }

    #[test]
    fn project_video_assets_are_retargeted_to_the_destination_namespace() {
        let batch = text_batch(&[
            (column::EPISODE_ID, &["episode_a", "episode-b"]),
            (
                column::MEDIA_RELATION,
                &["source.video_a.assets", "source.video_b.assets"],
            ),
        ]);

        let retargeted =
            ProjectBundle::retarget_video_media_relations(batch, "destination").unwrap();

        assert_eq!(
            ProjectBundle::utf8_values(&retargeted, column::MEDIA_RELATION).unwrap(),
            vec![SpiralSql::video_dataset_output("destination"); 2]
        );
        assert_eq!(
            ProjectBundle::video_episode_ids(&[(Table::VideoAssets, vec![retargeted])]).unwrap(),
            BTreeSet::from(["episode-b".to_owned(), "episode_a".to_owned()])
        );
    }

    #[test]
    fn forged_payload_length_is_rejected_before_allocation() {
        let name = Table::Datasets.name().to_owned();
        let mut bundle = envelope(manifest(vec![name.clone()]), &[(name.as_str(), b"")]);
        let length_offset = bundle.len() - 2 - 8;
        bundle[length_offset..length_offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        let error = ProjectBundle.parse_raw(&bundle).unwrap_err();
        assert!(error.contains("too large for this machine") || error.contains("truncated"));
    }

    #[test]
    fn export_payload_writer_enforces_its_bound() {
        let mut payload = LimitedPayload::new(3);
        payload.write_all(b"abc").unwrap();
        assert!(
            payload
                .write_all(b"d")
                .unwrap_err()
                .to_string()
                .contains("portable-file limit")
        );
    }

    #[test]
    fn project_workflow_relations_are_exactly_registry_allowlisted() {
        let source = "media.frames";
        let cache =
            spiral::StorageSchema::workflow_feature_cache_relation(WORKFLOW_TEST_NAMESPACE, source);
        let definition = workflow_definition(source, "holding_feature");
        let registry = canonical_registry_batch(std::slice::from_ref(&definition));
        let cache_batch = canonical_cache_batch(std::slice::from_ref(&definition), &[1]);
        let valid = workflow_project_bundle(&registry, &[(cache.as_str(), cache_batch.clone())]);
        let decoded = ProjectBundle::decode_entries(ProjectBundle.parse_raw(&valid).unwrap())
            .expect("a registry and its exact declared cache must decode");
        assert_eq!(
            decoded
                .iter()
                .map(|entry| entry.relation.bundle_name())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([Table::FeatureRegistry.name(), cache.as_str()])
        );
        let target_namespace = "restored_studio";
        let imported_cache = decoded
            .iter()
            .find(|entry| {
                matches!(
                    &entry.relation,
                    ProjectRelation::WorkflowFeatureCache { .. }
                )
            })
            .map(|entry| entry.relation.target(target_namespace))
            .expect("the decoded cache has an import target");
        assert_eq!(
            imported_cache,
            spiral::StorageSchema::workflow_feature_cache_relation(target_namespace, source)
        );
        let retargeted_registry =
            WorkflowBundleRegistry::retarget_batch(registry.clone(), target_namespace)
                .expect("the registry cache column retargets with the imported cache");
        WorkflowBundleRegistry::validate_for_namespace(&[retargeted_registry], target_namespace)
            .expect("the retargeted registry names the configured import table prefix");

        let registry_payload = arrow_payload(&registry);
        let missing = envelope(
            manifest(vec![Table::FeatureRegistry.name().to_owned()]),
            &[(Table::FeatureRegistry.name(), &registry_payload)],
        );
        assert!(
            ProjectBundle::decode_entries(ProjectBundle.parse_raw(&missing).unwrap())
                .unwrap_err()
                .contains("missing declared Workflow cache")
        );

        let undeclared_name = "studio.features_undeclared";
        let cache_payload = arrow_payload(&cache_batch);
        let undeclared = envelope(
            manifest(vec![
                Table::FeatureRegistry.name().to_owned(),
                cache.clone(),
                undeclared_name.to_owned(),
            ]),
            &[
                (Table::FeatureRegistry.name(), &registry_payload),
                (cache.as_str(), &cache_payload),
                (undeclared_name, &cache_payload),
            ],
        );
        assert!(
            ProjectBundle::decode_entries(ProjectBundle.parse_raw(&undeclared).unwrap())
                .unwrap_err()
                .contains("undeclared Workflow relation")
        );
    }

    #[test]
    fn project_registry_rejects_non_derived_cache_identity() {
        let definition = workflow_definition("media.frames", "holding_feature");
        let registry = replace_batch_column(
            &canonical_registry_batch(&[definition]),
            1,
            Arc::new(StringArray::from(vec!["studio.features_forged"])),
        );
        let payload = arrow_payload(&registry);
        let bundle = envelope(
            manifest(vec![Table::FeatureRegistry.name().to_owned()]),
            &[(Table::FeatureRegistry.name(), &payload)],
        );
        assert!(
            ProjectBundle::decode_entries(ProjectBundle.parse_raw(&bundle).unwrap())
                .unwrap_err()
                .contains("does not match server-derived name component")
        );
    }

    #[test]
    fn project_registry_requires_canonical_shape_rows_and_provenance() {
        let permissive_old_shape = text_batch(&[
            ("source_relation", &["media.frames"]),
            ("cache_relation", &["studio.features_media_frames"]),
        ]);
        let bundle = workflow_project_bundle(&permissive_old_shape, &[]);
        assert!(
            ProjectBundle::decode_entries(ProjectBundle.parse_raw(&bundle).unwrap())
                .unwrap_err()
                .contains("feature_registry has 2 columns; expected 14")
        );

        let definition = workflow_definition("media.frames", "holding_feature");
        let forged = replace_batch_column(
            &canonical_registry_batch(std::slice::from_ref(&definition)),
            7,
            Arc::new(StringArray::from(vec!["forged-model"])),
        );
        let bundle = workflow_project_bundle(&forged, &[]);
        assert!(
            ProjectBundle::decode_entries(ProjectBundle.parse_raw(&bundle).unwrap())
                .unwrap_err()
                .contains("incompatible producer provenance")
        );

        let duplicate = canonical_registry_batch(&[definition.clone(), definition.clone()]);
        assert!(
            WorkflowBundleRegistry::validate(&[duplicate])
                .unwrap_err()
                .contains("duplicate source/name")
        );
        let second = workflow_definition("media.frames", "still_feature");
        let bounded = canonical_registry_batch(&[definition, second]);
        assert!(
            WorkflowBundleRegistry::validate_with_limits(&[bounded], 1, MAX_FEATURE_CACHE_COLUMNS)
                .unwrap_err()
                .contains("bounded limit of 1")
        );
    }

    #[test]
    fn project_cache_uses_atomic_registry_provenance_after_spiral_drops_field_metadata() {
        let definition = workflow_definition("media.frames", "holding_feature");
        let registry_batch = canonical_registry_batch(std::slice::from_ref(&definition));
        let registry =
            WorkflowBundleRegistry::validate(std::slice::from_ref(&registry_batch)).unwrap();
        let relation = spiral::StorageSchema::workflow_feature_cache_relation(
            WORKFLOW_TEST_NAMESPACE,
            "media.frames",
        );

        let missing_feature = canonical_cache_batch(&[], &[1]);
        let bundle = workflow_project_bundle(
            &registry_batch,
            &[(relation.as_str(), missing_feature.clone())],
        );
        assert!(
            ProjectBundle::decode_entries(ProjectBundle.parse_raw(&bundle).unwrap())
                .unwrap_err()
                .contains("do not exactly match")
        );

        let canonical = canonical_cache_batch(std::slice::from_ref(&definition), &[1]);
        assert!(canonical.schema().field(5).metadata().is_empty());
        WorkflowCacheValidator::validate_bundle(
            &relation,
            std::slice::from_ref(&canonical),
            &registry,
        )
        .expect("metadata-free cache fields use the validated atomic registry provenance");

        let invalid_score =
            replace_batch_column(&canonical, 5, Arc::new(Float32Array::from(vec![Some(1.5)])));
        assert!(
            WorkflowCacheValidator::validate_bundle(&relation, &[invalid_score], &registry)
                .unwrap_err()
                .contains("sigmoid probability semantics")
        );
    }

    #[test]
    fn project_registry_accepts_pinned_spiral_nested_utf8view_normalization_only() {
        let expected = DataType::List(Arc::new(Field::new("item", DataType::Utf8, false)));
        let normalized = DataType::List(Arc::new(Field::new("item", DataType::Utf8View, false)));
        assert!(WorkflowFeatureSchema::storage_type_matches(
            &normalized,
            &expected
        ));
        assert!(!WorkflowFeatureSchema::storage_type_matches(
            &DataType::List(Arc::new(Field::new("item", DataType::BinaryView, false,))),
            &expected
        ));
        assert!(!WorkflowFeatureSchema::storage_type_matches(
            &DataType::List(Arc::new(Field::new("item", DataType::Utf8View, true,))),
            &expected
        ));
    }

    #[test]
    fn project_cache_rejects_duplicate_keys_and_bounded_overflow() {
        let first = workflow_definition("media.frames", "holding_feature");
        let second = workflow_definition("media.frames", "still_feature");
        let registry_batch = canonical_registry_batch(&[first.clone(), second.clone()]);
        let registry = WorkflowBundleRegistry::validate(&[registry_batch]).unwrap();
        let relation = spiral::StorageSchema::workflow_feature_cache_relation(
            WORKFLOW_TEST_NAMESPACE,
            "media.frames",
        );
        let duplicate = canonical_cache_batch(&[first.clone(), second.clone()], &[1, 1]);
        assert!(
            WorkflowCacheValidator::validate_bundle(&relation, &[duplicate], &registry)
                .unwrap_err()
                .contains("duplicate stable frame identities")
        );

        let rows = canonical_cache_batch(&[first.clone(), second.clone()], &[1, 2]);
        assert!(
            WorkflowCacheValidator::new(
                &relation,
                &registry,
                WorkflowCacheLimits::new(1, MAX_FEATURE_CACHE_COLUMNS),
            )
            .validate(std::slice::from_ref(&rows))
            .unwrap_err()
            .contains("bounded limit of 1 rows")
        );
        assert!(
            WorkflowCacheValidator::new(
                &relation,
                &registry,
                WorkflowCacheLimits::new(MAX_FEATURE_CACHE_ROWS, 1),
            )
            .validate(&[rows])
            .unwrap_err()
            .contains("exceeds 1 feature columns")
        );
    }

    #[test]
    fn workflow_restore_plan_retires_only_prior_caches_absent_from_project() {
        let current = BTreeSet::from([
            "studio.features_a".to_owned(),
            "studio.features_b".to_owned(),
        ]);
        let incoming = BTreeSet::from([
            "studio.features_b".to_owned(),
            "studio.features_c".to_owned(),
        ]);
        assert_eq!(
            WorkflowRestorePlan::from_caches(&current, &incoming),
            WorkflowRestorePlan {
                retire_caches: vec!["studio.features_a".to_owned()],
            }
        );
        let empty = BTreeSet::new();
        assert_eq!(
            WorkflowRestorePlan::from_caches(&current, &empty),
            WorkflowRestorePlan {
                retire_caches: vec![
                    "studio.features_a".to_owned(),
                    "studio.features_b".to_owned(),
                ],
            }
        );
    }

    #[derive(Default)]
    struct MockProjectTransaction {
        events: Vec<&'static str>,
        begin_error: Option<&'static str>,
        commit_error: Option<&'static str>,
        rollback_error: Option<&'static str>,
        reconnect_error: Option<&'static str>,
        discarded: bool,
    }

    impl ProjectImportTransaction for MockProjectTransaction {
        fn begin_project_import(&mut self) -> Result<(), String> {
            self.events.push("begin");
            self.begin_error
                .map_or(Ok(()), |error| Err(error.to_owned()))
        }

        fn commit_project_import(&mut self) -> Result<(), String> {
            self.events.push("commit");
            self.commit_error
                .map_or(Ok(()), |error| Err(error.to_owned()))
        }

        fn rollback_project_import(&mut self) -> Result<(), String> {
            self.events.push("rollback");
            self.rollback_error
                .map_or(Ok(()), |error| Err(error.to_owned()))
        }

        fn reconnect_project(&mut self) -> Result<(), String> {
            self.events.push("reconnect");
            self.reconnect_error
                .map_or(Ok(()), |error| Err(error.to_owned()))
        }

        fn discard_project_connection(&mut self) {
            self.events.push("discard");
            self.discarded = true;
        }
    }

    #[test]
    fn project_transaction_success_and_mutation_failure_have_deterministic_cleanup() {
        let mut success = MockProjectTransaction::default();
        let result = success
            .run_project_import_transaction("project import", |connection| {
                connection.events.push("mutation");
                Ok(7)
            })
            .unwrap();
        assert_eq!(result, (7, None));
        assert_eq!(success.events, ["begin", "mutation", "commit", "reconnect"]);
        assert!(!success.discarded);

        let mut failure = MockProjectTransaction::default();
        let error = failure
            .run_project_import_transaction::<()>("project import", |connection| {
                connection.events.push("mutation");
                Err("mutation failed".to_owned())
            })
            .unwrap_err();
        assert_eq!(error, "mutation failed");
        assert_eq!(failure.events, ["begin", "mutation", "rollback"]);
        assert!(!failure.discarded);
    }

    #[test]
    fn project_transaction_discards_every_uncertain_failure_and_preserves_errors() {
        let mut begin = MockProjectTransaction {
            begin_error: Some("begin failed"),
            ..Default::default()
        };
        let error = begin
            .run_project_import_transaction("project import", |_| Ok(()))
            .unwrap_err();
        assert!(error.contains("begin failed") && error.contains("discarded"));
        assert_eq!(begin.events, ["begin", "discard"]);

        let mut rollback = MockProjectTransaction {
            rollback_error: Some("rollback failed"),
            ..Default::default()
        };
        let error = rollback
            .run_project_import_transaction::<()>("project import", |_| {
                Err("mutation failed".to_owned())
            })
            .unwrap_err();
        assert!(error.contains("mutation failed") && error.contains("rollback failed"));
        assert_eq!(rollback.events, ["begin", "rollback", "discard"]);

        let mut uncertain_commit = MockProjectTransaction {
            commit_error: Some("commit failed"),
            ..Default::default()
        };
        let error = uncertain_commit
            .run_project_import_transaction("project import", |_| Ok(()))
            .unwrap_err();
        assert!(error.contains("commit failed") && error.contains("uncertain outcome"));
        assert_eq!(uncertain_commit.events, ["begin", "commit", "discard"]);
    }

    #[test]
    fn project_transaction_returns_committed_value_with_reconnect_warning() {
        let mut transaction = MockProjectTransaction {
            reconnect_error: Some("connect failed"),
            ..Default::default()
        };
        let (value, warning) = transaction
            .run_project_import_transaction("project import", |_| Ok(9))
            .unwrap();
        assert_eq!(value, 9);
        assert!(warning.unwrap().contains("post-commit"));
        assert_eq!(
            transaction.events,
            ["begin", "commit", "reconnect", "discard"]
        );
        assert!(transaction.discarded);
    }

    /// Run with a real SpiralDB stack:
    ///
    /// ```text
    /// SPIRAL_PROJECT_ID=project-id \
    /// cargo test -p spiral-robotics-backend \
    ///   backend::storage::bundle::tests::spiral_export_import_round_trip_overwrites_existing_data \
    ///   -- --ignored --exact
    /// ```
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires a real SpiralDB server, SpFS, and disposable project access"]
    async fn spiral_export_import_round_trip_overwrites_existing_data() {
        let project = required_project();
        let table_prefix = format!(
            "spiral_robotics_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let project_store = configured_project_store(project, table_prefix);
        let _cleanup = TestStudioTables::new(project_store.clone());
        let storage = StorageManager::new(project_store.clone());
        let readiness = storage.test_spiral().await;
        assert!(readiness.0, "{}", readiness.1);

        {
            let mut connection = project_store
                .connect_exclusive()
                .await
                .expect("the exported Spiral project must connect");
            connection
                .ingest_batch(
                    &Table::StudioSettings.qualified_name(project_store.table_prefix()),
                    settings_batch("exported-setting", r#"{"source":"export"}"#, 10),
                )
                .await
                .expect("the exported setting must save");
            connection
                .ingest_batch(
                    &Table::TrainingCheckpoints.qualified_name(project_store.table_prefix()),
                    checkpoint_batch(),
                )
                .await
                .expect("the exported checkpoint must save");
        }

        let exported = storage
            .export_project_bundle()
            .await
            .expect("exporting the Spiral project must succeed");
        let exported_payloads = relation_payloads(&exported);
        assert_eq!(exported_payloads.len(), Table::ALL.len());
        assert!(
            Table::ALL
                .iter()
                .all(|table| exported_payloads.contains_key(table.name()))
        );

        {
            let mut connection = project_store
                .connect_exclusive()
                .await
                .expect("the replacement Spiral project must connect");
            connection
                .replace_batches(
                    &Table::StudioSettings.qualified_name(project_store.table_prefix()),
                    vec![settings_batch(
                        "replacement-setting",
                        r#"{"source":"replacement"}"#,
                        20,
                    )],
                )
                .await
                .expect("the replacement setting must save");
            connection
                .replace_batches(
                    &Table::RolloutRuns.qualified_name(project_store.table_prefix()),
                    vec![snapshot_batch("replacement-run")],
                )
                .await
                .expect("the replacement rollout must save");
        }

        let replacement = storage
            .export_project_bundle()
            .await
            .expect("exporting the replacement state must succeed");
        assert_ne!(relation_payloads(&replacement), exported_payloads);

        let summary = storage
            .import_project_bundle(&exported)
            .await
            .expect("importing the Spiral project must succeed");
        assert_eq!(summary.relations(), Table::ALL.len() as u64);
        assert_eq!(summary.rows(), 2);

        let restored = storage
            .export_project_bundle()
            .await
            .expect("re-exporting the imported Spiral project must succeed");
        assert_eq!(relation_payloads(&restored), exported_payloads);
        let mut connection = project_store
            .connect()
            .await
            .expect("the restored Spiral project must connect");
        let rollout = Table::RolloutRuns.qualified_name(project_store.table_prefix());
        assert!(
            connection
                .query_rows(&format!("SELECT * FROM {rollout}"), 1)
                .await
                .expect("checking overwrite semantics must succeed")
                .rows()
                .is_empty()
        );
        drop(connection);
        _cleanup.cleanup().await;
    }

    /// Run with the same real-Spiral environment as the project transfer E2E.
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires a real SpiralDB server, SpFS, disposable project access, and FFmpeg"]
    async fn spiral_project_import_rebuilds_the_shared_managed_video_dataset() {
        let project = required_project();
        let table_prefix = format!(
            "spiral_robotics_video_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let project_store = configured_project_store(project, table_prefix);
        let _cleanup = TestStudioTables::new(project_store.clone());
        let storage = StorageManager::new(project_store.clone());
        let readiness = storage.test_spiral().await;
        assert!(readiness.0, "{}", readiness.1);
        let episode_id = "episode_video_import";
        let episode = EpisodeFixture::new(episode_id, "video-transfer")
            .with_timing(10, 2.0)
            .with_arms(vec![spiral_robotics_common::recording::EpisodeArm::new(
                "follower",
                Some("test".to_owned()),
                vec!["joint".to_owned()],
            )])
            .with_duration_seconds(1.0)
            .into_episode();
        let mut mp4 = test_mp4();
        // A valid top-level `free` box makes the fixture large enough to cross
        // Spiral's managed-Blob outlining threshold without making FFmpeg
        // encode hundreds of thousands of incompressible video bytes.
        let free_box_size = 300_000_u32;
        mp4.extend_from_slice(&free_box_size.to_be_bytes());
        mp4.extend_from_slice(b"free");
        mp4.resize(mp4.len() + free_box_size as usize - 8, 0);
        assert!(mp4.len() > 256 * 1024);

        storage
            .commit_episode(&episode, &[])
            .await
            .expect("the video episode must save");
        let mut connection = project_store
            .connect_exclusive()
            .await
            .expect("the video project must connect");
        connection
            .ingest_batch(
                &Table::VideoAssets.qualified_name(project_store.table_prefix()),
                ProjectBundle::prepare_video_asset_import(
                    video_asset_batch(episode_id, &mp4),
                    project_store.table_prefix(),
                )
                .expect("the portable video fixture must normalize to Blob"),
            )
            .await
            .expect("the video asset must save");
        drop(connection);
        let exported = storage
            .export_project_bundle()
            .await
            .expect("exporting the video project must succeed");
        let empty_project = ProjectBundle
            .build(Vec::new())
            .expect("the empty project bundle must build");
        storage
            .import_project_bundle(&empty_project)
            .await
            .expect("clearing the relational fixture before import must succeed");

        let first_import = storage
            .import_project_bundle(&exported)
            .await
            .expect("importing a video project must rebuild its managed dataset");
        assert!(
            first_import.warnings().is_empty(),
            "first import warnings: {:?}",
            first_import.warnings()
        );
        let second_import = storage
            .import_project_bundle(&exported)
            .await
            .expect("re-importing a video project must replace its managed dataset outputs");
        assert!(
            second_import.warnings().is_empty(),
            "second import warnings: {:?}",
            second_import.warnings()
        );

        let mut connection = project_store
            .connect()
            .await
            .expect("the imported video project must connect");
        let assets = Table::VideoAssets.qualified_name(project_store.table_prefix());
        let schema = connection
            .relation_schema(&assets)
            .await
            .expect("the imported video schema must load")
            .expect("imported video asset relation is missing");
        assert_eq!(
            schema
                .field_with_name(column::PAYLOAD)
                .ok()
                .and_then(|field| {
                    field
                        .metadata()
                        .get(arrow_schema::extension::EXTENSION_TYPE_NAME_KEY)
                })
                .map(String::as_str),
            Some("spql.io.Blob"),
        );
        let descriptor = connection
            .query_rows(
                &format!(
                    "SELECT spql.blob.uri({payload}) AS uri FROM {assets}",
                    payload = column::PAYLOAD,
                ),
                1,
            )
            .await
            .expect("the video Blob descriptor must load");
        assert!(
            descriptor.rows()[0]
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|uri| !uri.is_empty()),
            "large MP4 must be outlined into a provider-backed Blob",
        );
        let range = connection
            .run_binary_query(&SpiralSql::video_asset_range(
                project_store.table_prefix(),
                &crate::episodes::video::VideoAssetKey::new(
                    episode_id.to_owned(),
                    "camera.main".to_owned(),
                    Some(0),
                ),
                crate::episodes::video::VideoByteRange::new(512, 1024),
            ))
            .await
            .expect("the video Blob range must load");
        assert_eq!(range, mp4[512..1536]);

        let output = SpiralSql::video_dataset_output(project_store.table_prefix());
        let rows = connection
            .query_rows(&SpiralSql::count_rows(&output), 1)
            .await
            .expect("the rebuilt video dataset must be countable");
        assert_eq!(
            rows.rows()[0]
                .get("rows")
                .and_then(serde_json::Value::as_u64),
            Some(1)
        );
        let frames = connection
            .query_rows(
                &format!(
                    "SELECT COUNT(*) AS count FROM {output} AS asset, \
                         LATERAL spql.video.frames(asset.media, \
                         spql.video.frame_indices(ARRAY[0])) AS frame"
                ),
                1,
            )
            .await
            .expect("the rebuilt video frame must be queryable");
        assert_eq!(
            frames.rows()[0]
                .get("count")
                .and_then(serde_json::Value::as_u64),
            Some(1)
        );
        drop(connection);
        _cleanup.cleanup().await;
    }

    /// Profile the production project-import algorithm against a real archive
    /// with an isolated table prefix in the configured Spiral project.
    ///
    /// ```text
    /// SPIRAL_PROJECT_BUNDLE=/absolute/path/to/project.spiral-project \
    /// cargo test -p spiral-robotics-backend \
    ///   backend::storage::bundle::tests::spiral_profile_project_import \
    ///   -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "profiles a real project import and creates temporary Spiral resources"]
    async fn spiral_profile_project_import() {
        let bundle_path = std::env::var("SPIRAL_PROJECT_BUNDLE")
            .expect("SPIRAL_PROJECT_BUNDLE must name the project archive to profile");
        let process_start = Instant::now();
        eprintln!(
            "PROFILE\tmeta\tpid={}\tpath={bundle_path}",
            std::process::id()
        );
        let file_start = Instant::now();
        let bytes = fs::read(&bundle_path).expect("the profiling archive must be readable");
        profile_event("studio.file_read", file_start, (0, bytes.len()));

        let project = required_project();
        let table_prefix = format!(
            "spiral_robotics_import_profile_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository_start = Instant::now();
        let project_store = configured_project_store(project, table_prefix);
        profile_event("studio.repository_config", repository_start, (0, 0));
        let cleanup = TestStudioTables::new(project_store.clone());
        let storage = StorageManager::new(project_store.clone());

        let namespace_start = Instant::now();
        let readiness = storage.test_spiral().await;
        assert!(readiness.0, "{}", readiness.1);
        let mut connection = project_store
            .connect_exclusive()
            .await
            .expect("the profiling project must connect");
        profile_event("spiral.setup_namespace", namespace_start, (0, 0));

        let exclusive_start = Instant::now();
        let summary =
            profile_project_import_into(&mut connection, project_store.table_prefix(), &bytes)
                .await
                .expect("the profiled project import must succeed");
        drop(connection);
        profile_event(
            "studio.exclusive_import_total",
            exclusive_start,
            (summary.rows() as usize, bytes.len()),
        );

        let preferences_start = Instant::now();
        storage
            .load_studio_preferences()
            .await
            .expect("profiled preferences reload must succeed");
        profile_event("studio.reload_preferences", preferences_start, (0, 0));
        let training_start = Instant::now();
        storage
            .load_training_history()
            .await
            .expect("profiled training reload must succeed");
        profile_event("studio.reload_training", training_start, (0, 0));
        let rollout_start = Instant::now();
        storage
            .load_rollout_session()
            .await
            .expect("profiled rollout reload must succeed");
        profile_event("studio.reload_rollout", rollout_start, (0, 0));
        let catalog_start = Instant::now();
        let episodes = EpisodeStore::new(project_store.clone()).list().await;
        profile_event(
            "studio.refresh_episode_catalog",
            catalog_start,
            (episodes.len(), 0),
        );

        profile_event(
            "studio.route_equivalent_total",
            process_start,
            (summary.rows() as usize, bytes.len()),
        );
        eprintln!(
            "PROFILE\tmeta\trelations={}\trows={}\twarnings={}",
            summary.relations(),
            summary.rows(),
            summary.warnings().len()
        );
        let cleanup_start = Instant::now();
        cleanup.cleanup().await;
        profile_event("spiral.cleanup", cleanup_start, (0, 0));
    }

    /// Measure hosted Arbor's fixed create, append, and replacement costs
    /// separately from project parsing, video reconciliation, and Studio state
    /// reloads.
    ///
    /// ```text
    /// SPIRAL_PROJECT_BUNDLE=/absolute/path/to/project.spiral-project \
    /// cargo test -p spiral-robotics-backend \
    ///   backend::storage::bundle::tests::spiral_profile_write_fixed_cost \
    ///   -- --ignored --exact --nocapture
    /// ```
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "profiles real hosted Arbor replacements in temporary Spiral resources"]
    async fn spiral_profile_write_fixed_cost() {
        let bundle_path = std::env::var("SPIRAL_PROJECT_BUNDLE")
            .expect("SPIRAL_PROJECT_BUNDLE must name the project archive to profile");
        let bytes = fs::read(&bundle_path).expect("the profiling archive must be readable");
        let mut decoded = ProjectBundle
            .decode_managed_entries(
                ProjectBundle
                    .parse_managed_entries(&bytes)
                    .expect("the profiling archive must parse"),
            )
            .expect("the profiling archive must decode");
        ManagedRelationCatalog::sort(&mut decoded);

        let project = required_project();
        let table_prefix = format!(
            "spiral_robotics_replace_profile_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let project_store = configured_project_store(project, table_prefix);
        let cleanup = TestStudioTables::new(project_store.clone());
        let storage = StorageManager::new(project_store.clone());
        let readiness = storage.test_spiral().await;
        assert!(readiness.0, "{}", readiness.1);

        let mut connection = project_store
            .connect_exclusive()
            .await
            .expect("the scoped profiling project must connect");
        for (label, table, row_limit) in [
            ("dataset_1", Table::Datasets, 1),
            ("joint_0", Table::JointSamples, 0),
            ("joint_1", Table::JointSamples, 1),
            ("joint_512", Table::JointSamples, 512),
            ("joint_1000", Table::JointSamples, 1_000),
            ("joint_full", Table::JointSamples, usize::MAX),
            ("video_1", Table::VideoAssets, 1),
            ("video_full", Table::VideoAssets, usize::MAX),
        ] {
            if std::env::var("SPIRAL_PROFILE_CASE").is_ok_and(|requested| requested != label) {
                continue;
            }
            let source = decoded
                .iter()
                .find_map(|(candidate, batches)| (*candidate == table).then_some(batches))
                .unwrap_or_else(|| panic!("profiling archive omits {}", table.name()));
            let batches = profile_prefix_batches(source, row_limit);
            let rows = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
            let encoded_bytes =
                profile_encoded_bytes(&batches).expect("the profiling Arrow batches must encode");
            let target = format!("{}_{label}", project_store.table_prefix());
            let ingest_target = format!("{}_{label}_ingest", project_store.table_prefix());
            let autocommit_target = format!("{}_{label}_autocommit", project_store.table_prefix());

            ProfileWrite::new(label, "ingest_missing_autocommit", &autocommit_target)
                .with_batches(&batches, (rows, encoded_bytes))
                .creating_if_missing()
                .write_autocommit(&mut connection)
                .await
                .expect("the missing autocommit profile write must succeed");
            ProfileWrite::new(label, "append_existing_autocommit", &autocommit_target)
                .with_batches(&batches, (rows, encoded_bytes))
                .write_autocommit(&mut connection)
                .await
                .expect("the existing autocommit profile write must succeed");
            ProfileWrite::new(label, "ingest_missing", &ingest_target)
                .with_batches(&batches, (rows, encoded_bytes))
                .creating_if_missing()
                .write_transaction(&mut connection)
                .await
                .expect("the missing ingest profile write must succeed");
            ProfileWrite::new(label, "replace_missing", &target)
                .with_batches(&batches, (rows, encoded_bytes))
                .with_replace()
                .creating_if_missing()
                .write_transaction(&mut connection)
                .await
                .expect("the missing replacement profile write must succeed");
            ProfileWrite::new(label, "existing_cold", &target)
                .with_batches(&batches, (rows, encoded_bytes))
                .with_replace()
                .write_transaction(&mut connection)
                .await
                .expect("the cold replacement profile write must succeed");

            for bind_index in 1..=10 {
                let bind_start = Instant::now();
                let exists = connection
                    .relation_schema(&target)
                    .await
                    .expect("the existing profile relation must bind")
                    .is_some();
                eprintln!(
                    "PROFILE_SCOPED\tcase={label}\tphase=bind_existing_{bind_index}\tseconds={:.6}\trows={rows}\tbytes={encoded_bytes}\texists={exists}",
                    bind_start.elapsed().as_secs_f64()
                );
            }
            let missing_target = format!("{}_missing", project_store.table_prefix());
            for bind_index in 1..=5 {
                let bind_start = Instant::now();
                let exists = connection
                    .relation_schema(&missing_target)
                    .await
                    .expect("the missing profile relation must bind")
                    .is_some();
                eprintln!(
                    "PROFILE_SCOPED\tcase={label}\tphase=bind_missing_{bind_index}\tseconds={:.6}\trows=0\tbytes=0\texists={exists}",
                    bind_start.elapsed().as_secs_f64()
                );
            }
            ProfileWrite::new(label, "existing_after_bind", &target)
                .with_batches(&batches, (rows, encoded_bytes))
                .with_replace()
                .write_transaction(&mut connection)
                .await
                .expect("the post-bind replacement profile write must succeed");
        }
        drop(connection);

        let cleanup_start = Instant::now();
        cleanup.cleanup().await;
        profile_event("spiral.cleanup", cleanup_start, (0, 0));
    }

    struct ProfileWrite<'a> {
        label: &'a str,
        phase: &'a str,
        target: &'a str,
        batches: &'a [RecordBatch],
        metrics: (usize, usize),
        replace: bool,
        create_if_missing: bool,
    }

    impl<'a> ProfileWrite<'a> {
        fn new(label: &'a str, phase: &'a str, target: &'a str) -> Self {
            Self {
                label,
                phase,
                target,
                batches: &[],
                metrics: (0, 0),
                replace: false,
                create_if_missing: false,
            }
        }

        fn with_batches(mut self, batches: &'a [RecordBatch], metrics: (usize, usize)) -> Self {
            self.batches = batches;
            self.metrics = metrics;
            self
        }

        fn with_replace(mut self) -> Self {
            self.replace = true;
            self
        }

        fn creating_if_missing(mut self) -> Self {
            self.create_if_missing = true;
            self
        }

        async fn write_autocommit(self, connection: &mut ProjectConnection) -> Result<(), String> {
            let write_start = Instant::now();
            if self.create_if_missing && self.replace {
                connection
                    .replace_or_create_batches(self.target, self.batches.to_vec())
                    .await?;
            } else if self.create_if_missing {
                connection
                    .create_or_append_batches(self.target, self.batches.to_vec())
                    .await?;
            } else {
                connection
                    .ingest_batches(self.target, self.batches.to_vec())
                    .await?;
            }
            let write_seconds = write_start.elapsed().as_secs_f64();
            let (rows, bytes) = self.metrics;
            let label = self.label;
            let phase = self.phase;
            eprintln!(
                "PROFILE_SCOPED\tcase={label}\tphase={phase}\tseconds={write_seconds:.6}\trows={rows}\tbytes={bytes}"
            );
            Ok(())
        }

        async fn write_transaction(self, connection: &mut ProjectConnection) -> Result<(), String> {
            let begin_start = Instant::now();
            connection.begin_transaction().await?;
            let begin_seconds = begin_start.elapsed().as_secs_f64();

            let write_start = Instant::now();
            if self.create_if_missing && self.replace {
                connection
                    .replace_or_create_batches(self.target, self.batches.to_vec())
                    .await?;
            } else if self.create_if_missing {
                connection
                    .create_or_append_batches(self.target, self.batches.to_vec())
                    .await?;
            } else if self.replace {
                connection
                    .replace_batches(self.target, self.batches.to_vec())
                    .await?;
            } else {
                connection
                    .ingest_batches(self.target, self.batches.to_vec())
                    .await?;
            }
            let write_seconds = write_start.elapsed().as_secs_f64();

            let commit_start = Instant::now();
            connection.commit().await?;
            let commit_seconds = commit_start.elapsed().as_secs_f64();
            connection.reconnect().await.map_err(|error| {
                format!(
                    "profile transaction committed, but its post-commit connection could not open: {error}"
                )
            })?;
            let (rows, bytes) = self.metrics;
            let label = self.label;
            let phase = self.phase;
            eprintln!(
                "PROFILE_SCOPED\tcase={label}\tphase={phase}\tseconds={write_seconds:.6}\tbegin_seconds={begin_seconds:.6}\tcommit_seconds={commit_seconds:.6}\trows={rows}\tbytes={bytes}"
            );
            Ok(())
        }
    }

    fn profile_prefix_batches(batches: &[RecordBatch], limit: usize) -> Vec<RecordBatch> {
        if limit == 0 {
            return batches
                .first()
                .map(|batch| vec![batch.slice(0, 0)])
                .unwrap_or_default();
        }
        let mut remaining = limit;
        let mut prefix = Vec::new();
        for batch in batches {
            if remaining == 0 {
                break;
            }
            let rows = remaining.min(batch.num_rows());
            prefix.push(batch.slice(0, rows));
            remaining -= rows;
        }
        prefix
    }

    fn profile_encoded_bytes(batches: &[RecordBatch]) -> Result<usize, String> {
        let schema = batches
            .first()
            .map(RecordBatch::schema)
            .ok_or_else(|| "scoped profiling requires at least one batch".to_owned())?;
        let mut payload = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut payload, schema.as_ref())
                .map_err(|error| error.to_string())?;
            for batch in batches {
                writer.write(batch).map_err(|error| error.to_string())?;
            }
            writer.finish().map_err(|error| error.to_string())?;
        }
        Ok(payload.len())
    }

    async fn profile_project_import_into(
        connection: &mut ProjectConnection,
        table_prefix: &str,
        bundle: &[u8],
    ) -> Result<ProjectImportSummary, String> {
        let parse_start = Instant::now();
        let entries = ProjectBundle.parse_managed_entries(bundle)?;
        profile_event("studio.bundle_parse", parse_start, (0, bundle.len()));

        let mut decoded = Vec::new();
        for (table, payload) in entries {
            let payload_bytes = payload.len();
            let decode_start = Instant::now();
            let mut relation = ProjectBundle.decode_managed_entries(vec![(table, payload)])?;
            let rows = relation
                .iter()
                .flat_map(|(_, batches)| batches)
                .map(RecordBatch::num_rows)
                .sum();
            profile_event(
                &format!("studio.arrow_decode.{}", table.name()),
                decode_start,
                (rows, payload_bytes),
            );
            decoded.append(&mut relation);
        }
        for table in Table::ALL {
            if decoded.iter().any(|(incoming, _)| *incoming == table) {
                continue;
            }
            decoded.push((
                table,
                vec![RecordBatch::new_empty(Arc::new(
                    ManagedRelationCatalog::schema(table),
                ))],
            ));
        }
        ManagedRelationCatalog::sort(&mut decoded);
        if let Some(limit) = profile_usize("SPIRAL_PROFILE_VIDEO_LIMIT") {
            limit_relation_rows(&mut decoded, Table::VideoAssets, limit);
        }

        let video_ids_start = Instant::now();
        let imported_video_episodes = ProjectBundle::video_episode_ids(&decoded)?;
        profile_event(
            "studio.collect_video_episode_ids",
            video_ids_start,
            (imported_video_episodes.len(), 0),
        );
        let transaction_start = Instant::now();
        connection
            .begin_transaction()
            .await
            .map_err(|error| format!("starting profiled project import transaction: {error}"))?;
        profile_event("spiral.transaction_begin", transaction_start, (0, 0));

        let outcome = async {
            let video_chunk_rows = profile_usize("SPIRAL_PROFILE_VIDEO_CHUNK_ROWS");
            let mut summary = ProjectImportSummary::default();
            for (table, batches) in decoded {
                let relation = table.qualified_name(table_prefix);
                let relation_ingest_start = Instant::now();
                let mut relation_rows = 0usize;
                let mut relation_bytes = 0usize;
                let mut stream_batches = Vec::new();
                for batch in batches {
                    let transform_start = Instant::now();
                    let batch = if table == Table::VideoAssets {
                        ProjectBundle::prepare_video_asset_import(batch, table_prefix)?
                    } else {
                        batch
                    };
                    profile_event(
                        &format!("studio.transform.{}", table.name()),
                        transform_start,
                        (batch.num_rows(), batch.get_array_memory_size()),
                    );
                    if batch.num_rows() == 0 {
                        stream_batches.push(batch);
                        continue;
                    }
                    let chunk_rows = if table == Table::VideoAssets {
                        video_chunk_rows.unwrap_or(batch.num_rows().max(1))
                    } else {
                        batch.num_rows().max(1)
                    };
                    for offset in (0..batch.num_rows()).step_by(chunk_rows) {
                        let len = chunk_rows.min(batch.num_rows() - offset);
                        let chunk = batch.slice(offset, len);
                        let chunk_bytes = chunk.get_array_memory_size();
                        stream_batches.push(chunk);
                        relation_rows = relation_rows.saturating_add(len);
                        relation_bytes = relation_bytes.saturating_add(chunk_bytes);
                    }
                }
                summary.record_relation(relation_rows as u64);
                let stream_ingest_start = Instant::now();
                connection
                    .replace_batches(&relation, stream_batches)
                    .await?;
                profile_event(
                    &format!("spiral.replace.{}.stream", table.name()),
                    stream_ingest_start,
                    (relation_rows, relation_bytes),
                );
                profile_event(
                    &format!("spiral.replace_total.{}", table.name()),
                    relation_ingest_start,
                    (relation_rows, 0),
                );
            }
            Ok(summary)
        }
        .await;

        let mut summary = match outcome {
            Ok(summary) => {
                let commit_start = Instant::now();
                connection
                    .commit()
                    .await
                    .map_err(|error| format!("committing profiled project import: {error}"))?;
                profile_event(
                    "spiral.transaction_commit",
                    commit_start,
                    (summary.rows() as usize, 0),
                );
                summary
            }
            Err(error) => {
                let rollback_start = Instant::now();
                let rollback = connection.rollback().await;
                profile_event("spiral.transaction_rollback", rollback_start, (0, 0));
                return Err(match rollback {
                    Ok(()) => error,
                    Err(rollback) => format!("{error}; rolling back also failed: {rollback}"),
                });
            }
        };
        if let Err(error) = connection.reconnect().await {
            connection.discard();
            summary.push_warning(format!(
                "profiled project import committed, but its post-commit connection could not open; video reconciliation was skipped: {error}"
            ));
            return Ok(summary);
        }

        if !profile_bool("SPIRAL_PROFILE_SKIP_VIDEO_RECONCILE") {
            let reconcile_total_start = Instant::now();
            if imported_video_episodes.is_empty() {
                connection.drop_video_dataset(table_prefix).await?;
            } else {
                let _ = profile_rebuild_video_dataset(connection, table_prefix).await?;
            }
            profile_event(
                "spiral.reconcile_video_total",
                reconcile_total_start,
                (imported_video_episodes.len(), 0),
            );
        }
        Ok(summary)
    }

    async fn profile_rebuild_video_dataset(
        connection: &mut ProjectConnection,
        table_prefix: &str,
    ) -> Result<bool, String> {
        let exists_start = Instant::now();
        let dataset_exists = connection.video_dataset_exists(table_prefix).await?;
        profile_event("spiral.reconcile_video_exists", exists_start, (1, 0));

        let sources = SpiralSql::video_sources_relation(table_prefix);
        let assets = Table::VideoAssets.qualified_name(table_prefix);
        let dataset = SpiralSql::video_dataset_relation(table_prefix);
        let view_start = Instant::now();
        connection
            .execute_update_sql(&SpiralSql::create_video_sources(&sources, &assets))
            .await
            .map_err(|error| format!("creating Spiral video source view: {error}"))?;
        profile_event("spiral.reconcile_video_view", view_start, (1, 0));
        if !dataset_exists {
            let create_start = Instant::now();
            connection
                .execute_update_sql(&SpiralSql::create_video_dataset(&dataset, &sources))
                .await
                .map_err(|error| format!("creating Spiral MP4 dataset: {error}"))?;
            profile_event("spiral.reconcile_video_create", create_start, (1, 0));
        }
        let refresh_start = Instant::now();
        connection
            .execute_update_sql(&SpiralSql::refresh_dataset(&dataset))
            .await
            .map_err(|error| format!("refreshing Spiral MP4 dataset: {error}"))?;
        profile_event("spiral.reconcile_video_refresh", refresh_start, (1, 0));
        Ok(dataset_exists)
    }

    fn profile_event(stage: &str, start: Instant, metrics: (usize, usize)) {
        let (rows, bytes) = metrics;
        eprintln!(
            "PROFILE\t{stage}\tseconds={:.6}\trows={rows}\tbytes={bytes}",
            start.elapsed().as_secs_f64()
        );
    }

    fn profile_usize(name: &str) -> Option<usize> {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
    }

    fn profile_bool(name: &str) -> bool {
        std::env::var(name)
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes"))
    }

    fn limit_relation_rows(decoded: &mut [(Table, Vec<RecordBatch>)], target: Table, limit: usize) {
        let Some((_, batches)) = decoded.iter_mut().find(|(table, _)| *table == target) else {
            return;
        };
        let mut remaining = limit;
        let mut limited = Vec::new();
        for batch in batches.drain(..) {
            if remaining == 0 {
                break;
            }
            let rows = remaining.min(batch.num_rows());
            limited.push(batch.slice(0, rows));
            remaining -= rows;
        }
        *batches = limited;
    }

    fn required_project() -> String {
        std::env::var("SPIRAL_PROJECT_ID")
            .ok()
            .filter(|project| !project.trim().is_empty())
            .or_else(|| BackendStateStore::open_default().ok()?.project_id())
            .expect(
                "SPIRAL_PROJECT_ID or a project saved in Studio Settings is required for the Spiral E2E test",
            )
    }

    fn stored_credentials() -> Option<crate::settings::SpiralCredentials> {
        BackendStateStore::open_default().ok()?.credentials()
    }

    fn configured_spiral_endpoint() -> String {
        BackendStateStore::open_default()
            .ok()
            .and_then(|store| store.spiral_endpoint())
            .unwrap_or_else(|| ServiceEndpoint::environment_spiral().into_string())
    }

    fn configured_spiral_spfs_endpoint() -> String {
        BackendStateStore::open_default()
            .ok()
            .and_then(|store| store.spiral_spfs_endpoint())
            .unwrap_or_else(|| ServiceEndpoint::environment_spiral_spfs().into_string())
    }

    fn configured_project_store(project: String, table_prefix: String) -> Arc<ProjectStore> {
        let settings = SpiralConnectionSettings::new(
            configured_spiral_endpoint(),
            configured_spiral_spfs_endpoint(),
        )
        .expect("the configured Spiral endpoints must be valid")
        .with_project(&project)
        .expect("the Spiral E2E project must be valid")
        .with_credentials(stored_credentials());
        Arc::new(
            {
                let database = Arc::new(crate::AdbcDatabase::new(settings.clone()));
                ProjectStore::new(settings, table_prefix, database)
            }
            .expect("the Spiral E2E project store must be valid"),
        )
    }

    fn relation_payloads(bundle: &[u8]) -> BTreeMap<String, Vec<u8>> {
        ProjectBundle
            .parse_raw(bundle)
            .expect("the exported project must parse")
            .into_iter()
            .collect()
    }

    fn settings_batch(key: &str, value: &str, updated_ms: i64) -> RecordBatch {
        record_batch(
            vec![
                Field::new(spiral::column::KEY, DataType::Utf8, false),
                Field::new(spiral::column::VALUE_JSON, DataType::Utf8, false),
                Field::new(spiral::column::UPDATED_MS, DataType::Int64, false),
            ],
            vec![
                Arc::new(StringArray::from(vec![key])) as ArrayRef,
                Arc::new(StringArray::from(vec![value])) as ArrayRef,
                Arc::new(Int64Array::from(vec![updated_ms])) as ArrayRef,
            ],
        )
    }

    fn checkpoint_batch() -> RecordBatch {
        record_batch(
            vec![
                Field::new(spiral::column::RUN_ID, DataType::Utf8, false),
                Field::new(spiral::column::RUN_NAME, DataType::Utf8, false),
                Field::new(spiral::column::POLICY, DataType::Utf8, false),
                Field::new(spiral::column::STEP, DataType::UInt64, false),
                Field::new(spiral::column::CREATED_MS, DataType::Int64, false),
                Field::new(spiral::column::VALIDATION_LOSS, DataType::Float64, true),
                Field::new(spiral::column::CONFIG_JSON, DataType::Utf8, false),
                Field::new(spiral::column::CHECKPOINT, DataType::LargeBinary, false),
            ],
            vec![
                Arc::new(StringArray::from(vec!["exported-run"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["Exported run"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["act"])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![42])) as ArrayRef,
                Arc::new(Int64Array::from(vec![10])) as ArrayRef,
                Arc::new(Float64Array::from(vec![Some(0.125)])) as ArrayRef,
                Arc::new(StringArray::from(vec![r#"{"chunk_size":16}"#])) as ArrayRef,
                Arc::new(LargeBinaryArray::from_iter_values([
                    b"checkpoint-binary".as_slice()
                ])) as ArrayRef,
            ],
        )
    }

    fn snapshot_batch(run_id: &str) -> RecordBatch {
        record_batch(
            vec![
                Field::new(spiral::column::RUN_ID, DataType::Utf8, false),
                Field::new(spiral::column::SNAPSHOT_JSON, DataType::Utf8, false),
                Field::new(spiral::column::UPDATED_MS, DataType::Int64, false),
            ],
            vec![
                Arc::new(StringArray::from(vec![run_id])) as ArrayRef,
                Arc::new(StringArray::from(vec![r#"{"source":"replacement"}"#])) as ArrayRef,
                Arc::new(Int64Array::from(vec![20])) as ArrayRef,
            ],
        )
    }

    fn test_mp4() -> Vec<u8> {
        let ffmpeg = std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned());
        let output = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=16x16:r=2:d=1",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "frag_keyframe+empty_moov+default_base_moof",
                "-f",
                "mp4",
                "pipe:1",
            ])
            .output()
            .unwrap_or_else(|error| panic!("could not run {ffmpeg:?}: {error}"));
        assert!(
            output.status.success(),
            "FFmpeg fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty(), "FFmpeg fixture emitted no MP4");
        output.stdout
    }

    fn video_asset_batch(episode_id: &str, payload: &[u8]) -> RecordBatch {
        record_batch(
            vec![
                Field::new(spiral::column::EPISODE_ID, DataType::Utf8, false),
                Field::new(spiral::column::STREAM_ID, DataType::Utf8, false),
                Field::new(spiral::column::SEGMENT_INDEX, DataType::UInt32, false),
                Field::new(spiral::column::TIMELINE_OFFSET_NS, DataType::UInt64, true),
                Field::new(spiral::column::CAMERA_ID, DataType::Utf8, false),
                Field::new(spiral::column::PAYLOAD, DataType::Binary, false),
                Field::new(spiral::column::BYTE_SIZE, DataType::UInt64, false),
                Field::new(spiral::column::WIDTH, DataType::UInt32, false),
                Field::new(spiral::column::HEIGHT, DataType::UInt32, false),
                Field::new(spiral::column::FPS, DataType::UInt32, false),
                Field::new(spiral::column::FRAMES, DataType::UInt64, false),
                Field::new(spiral::column::MEDIA_RELATION, DataType::Utf8, false),
            ],
            vec![
                Arc::new(StringArray::from(vec![episode_id])) as ArrayRef,
                Arc::new(StringArray::from(vec!["camera.main"])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![0])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![Some(0)])) as ArrayRef,
                Arc::new(StringArray::from(vec!["camera-e2e"])) as ArrayRef,
                Arc::new(BinaryArray::from_iter_values([payload])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![payload.len() as u64])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![16])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![16])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![2])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![2])) as ArrayRef,
                Arc::new(StringArray::from(vec!["source.wrong.assets"])) as ArrayRef,
            ],
        )
    }

    fn record_batch(fields: Vec<Field>, columns: Vec<ArrayRef>) -> RecordBatch {
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .expect("the E2E fixture batch must match its schema")
    }

    async fn drop_studio_relations(
        connection: &mut ProjectConnection,
        table_prefix: &str,
    ) -> Result<(), String> {
        connection.drop_video_dataset(table_prefix).await?;
        for table in Table::ALL.into_iter().rev() {
            let relation = table.qualified_name(table_prefix);
            connection
                .execute_update_sql(&format!("DROP TABLE IF EXISTS {relation}"))
                .await?;
        }
        Ok(())
    }

    struct TestStudioTables {
        project_store: Arc<ProjectStore>,
    }

    impl TestStudioTables {
        fn new(project_store: Arc<ProjectStore>) -> Self {
            Self { project_store }
        }

        async fn cleanup(&self) {
            let table_prefix = self.project_store.table_prefix().to_owned();
            if let Ok(mut connection) = self.project_store.connect_exclusive().await {
                let _ = drop_studio_relations(&mut connection, &table_prefix).await;
            }
        }
    }
}
