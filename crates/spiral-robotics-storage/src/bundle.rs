//! Portable single-file snapshots of the Studio-owned Spiral namespace.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Cursor, Read as _};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::schema::{self as spiral, Table, column};
use arrow_array::{
    Array as _, ArrayRef, Float32Array, LargeStringArray, ListArray, RecordBatch, StringArray,
    StringViewArray, UInt32Array, UInt64Array,
};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};
use spiral_robotics_api::datasets::{EpisodeImportSummary, ProjectImportSummary};
use spiral_robotics_api::sql::{QualifiedRelation, UnquotedSqlIdentifier};
use spiral_robotics_api::workflow::{
    DEFAULT_WORKFLOW_ID, MAX_SIGLIP_CONCEPT_LENGTH, MAX_WORKFLOW_AGENT_CONVERSATION_TITLE_CHARS,
    MAX_WORKFLOW_AGENT_CONVERSATIONS, MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION,
    MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH, MAX_WORKFLOW_FEATURE_STREAMS,
    WorkflowAgentMessagePhase, WorkflowAgentPhase, WorkflowAgentRole, WorkflowAgentToolPhase,
    WorkflowFeatureApplicability, WorkflowFeatureDefinition,
};

use crate::adbc::StorageConnection;
use crate::manager::VideoAssetRecordBatch as _;
use crate::sql::SpiralSql;

const PROJECT_MAGIC: &[u8; 8] = b"SPRPROJ1";
const EPISODES_MAGIC: &[u8; 8] = b"SPREPS01";
const PROJECT_FORMAT: &str = "spiral-robotics-project";
const EPISODES_FORMAT: &str = "spiral-robotics-episodes";
const FORMAT_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ENTRY_NAME_BYTES: usize = 256;
const MAX_FEATURE_REGISTRY_ROWS: usize = 4_096;
const MAX_FEATURE_CACHE_ROWS: usize = 1_000_000;
const MAX_FEATURE_CACHE_COLUMNS: usize = 256;
pub const SIGLIP_BASE_MODEL_ID: &str = "siglip-base";
pub const SIGLIP_BASE_REPOSITORY: &str = "google/siglip-base-patch16-224";
pub const SIGLIP_BASE_REVISION: &str = "7fd15f0689c79d79e38b1c2e2e2370a7bf2761ed";
pub const WORKFLOW_FEATURE_REGISTRY_RELATION: &str = "workflow.feature_registry";
const MAX_ARROW_MESSAGE_METADATA_BYTES: usize = MAX_MANIFEST_BYTES;
const MAX_ARROW_DECODED_BYTES: usize = ProjectBundle::MAX_BYTES;
const MAX_ARROW_RECORD_BATCH_ROWS: usize = MAX_FEATURE_CACHE_ROWS;
const MAX_ARROW_DECODED_ROWS: usize = MAX_FEATURE_CACHE_ROWS;
const MAX_ARROW_SCHEMA_FIELDS: usize = MAX_FEATURE_CACHE_COLUMNS + CACHE_KEY_FIELDS.len();
const WORKFLOW_AGENT_DOCUMENT_SCHEMA_VERSION: u64 = 2;
const MAX_WORKFLOW_AGENT_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const FEATURE_VALUE_SEMANTICS: &str = "sigmoid_probability_0_1";
const CACHE_KEY_FIELDS: [(&str, DataType); 5] = [
    ("episode_id", DataType::Utf8),
    ("stream_id", DataType::Utf8),
    ("segment_index", DataType::UInt32),
    ("frame_index", DataType::UInt64),
    ("episode_time_ns", DataType::UInt64),
];

struct WorkflowArrow;

impl WorkflowArrow {
    fn cache_key_may_be_nullable(name: &str) -> bool {
        matches!(name, "frame_index" | "episode_time_ns")
    }
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentDocumentV2 {
    schema_version: u64,
    revision: u64,
    active_conversation_id: String,
    conversations: Vec<BundleAgentConversation>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentConversation {
    conversation_id: String,
    title: String,
    created_ms: u64,
    updated_ms: u64,
    session: BundleAgentSession,
    provider_messages: Vec<BundleProviderMessage>,
    #[serde(default)]
    owned_run_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentSession {
    #[serde(flatten)]
    identity: BundleAgentSessionIdentity,
    messages: Vec<BundleAgentMessage>,
    #[serde(flatten)]
    lifecycle: BundleAgentSessionLifecycle,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentSessionIdentity {
    workflow_id: String,
    #[serde(default, alias = "session_id")]
    conversation_id: String,
    revision: u64,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentSessionLifecycle {
    phase: WorkflowAgentPhase,
    #[serde(default)]
    active_turn_id: Option<String>,
    #[serde(default)]
    deferred_run_id: Option<String>,
    #[serde(default)]
    error: Option<String>,
    updated_ms: u64,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentMessage {
    #[serde(flatten)]
    identity: BundleAgentMessageIdentity,
    #[serde(flatten)]
    content: BundleAgentMessageContent,
    #[serde(flatten)]
    provenance: BundleAgentMessageProvenance,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentMessageIdentity {
    id: String,
    sequence: u64,
    role: WorkflowAgentRole,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentMessageContent {
    phase: WorkflowAgentMessagePhase,
    text: String,
    tools: Vec<BundleAgentToolSummary>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentMessageProvenance {
    #[serde(default)]
    run_id: Option<String>,
    created_ms: u64,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleAgentToolSummary {
    call_id: String,
    name: String,
    phase: WorkflowAgentToolPhase,
    summary: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    receipt: Option<BundleAgentArtifactReceipt>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BundleAgentArtifactReceipt {
    Graph {
        label: String,
        revision: u64,
    },
    Run {
        label: String,
        run_id: String,
    },
    Results {
        label: String,
        run_id: String,
        #[serde(default)]
        row_count: Option<usize>,
    },
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleProviderMessage {
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    tool_calls: Vec<BundleProviderToolCall>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleProviderToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: BundleProviderFunctionCall,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleProviderFunctionCall {
    name: String,
    arguments: String,
}

/// Browser transfers are memory-backed at the Axum/storage boundary. Keep the
/// cap low enough that one import and its decoded Arrow batch cannot exhaust a
/// typical Studio workstation.
pub struct ProjectBundle;

impl ProjectBundle {
    pub const MAX_BYTES: usize = 256 * 1024 * 1024;
}

pub struct EpisodeBundle;

pub(crate) struct BundleSource<'a> {
    namespace: &'a str,
    bytes: &'a [u8],
}

impl<'a> BundleSource<'a> {
    pub(crate) const fn new(namespace: &'a str, bytes: &'a [u8]) -> Self {
        Self { namespace, bytes }
    }
}

pub(crate) struct EpisodeBundleSelection<'a> {
    namespace: &'a str,
    dataset_id: &'a str,
}

impl<'a> EpisodeBundleSelection<'a> {
    pub(crate) const fn new(namespace: &'a str, dataset_id: &'a str) -> Self {
        Self {
            namespace,
            dataset_id,
        }
    }
}

pub(crate) struct EpisodeBundleImport<'a> {
    selection: EpisodeBundleSelection<'a>,
    bytes: &'a [u8],
}

impl<'a> EpisodeBundleImport<'a> {
    pub(crate) const fn new(selection: EpisodeBundleSelection<'a>, bytes: &'a [u8]) -> Self {
        Self { selection, bytes }
    }
}

impl EpisodeBundle {
    pub const MAX_BYTES: usize = ProjectBundle::MAX_BYTES;
}

const EPISODE_TABLES: [Table; 5] = [
    Table::Datasets,
    Table::Episodes,
    Table::EpisodeStreams,
    Table::JointSamples,
    Table::VideoAssets,
];

#[derive(Clone, Copy)]
struct BundleSpec {
    magic: &'static [u8; 8],
    format: &'static str,
    label: &'static str,
    tables: &'static [Table],
    max_bytes: usize,
}

const PROJECT_SPEC: BundleSpec = BundleSpec {
    magic: PROJECT_MAGIC,
    format: PROJECT_FORMAT,
    label: "project",
    tables: &Table::ALL,
    max_bytes: ProjectBundle::MAX_BYTES,
};

const EPISODES_SPEC: BundleSpec = BundleSpec {
    magic: EPISODES_MAGIC,
    format: EPISODES_FORMAT,
    label: "episode bundle",
    tables: &EPISODE_TABLES,
    max_bytes: EpisodeBundle::MAX_BYTES,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Manifest {
    format: String,
    format_version: u32,
    schema_version: u32,
    exported_ms: i64,
    relations: Vec<String>,
}

struct ExportedBundle {
    bytes: Vec<u8>,
    row_counts: Vec<(Table, u64)>,
}

struct NamedExportedBundle {
    bytes: Vec<u8>,
    row_counts: Vec<(String, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProjectRelation {
    Managed(Table),
    WorkflowFeatureRegistry,
    WorkflowFeatureCache(String),
}

impl ProjectRelation {
    #[cfg(test)]
    fn bundle_name(&self) -> &str {
        match self {
            Self::Managed(table) => table.name(),
            Self::WorkflowFeatureRegistry => WORKFLOW_FEATURE_REGISTRY_RELATION,
            Self::WorkflowFeatureCache(relation) => relation,
        }
    }

    fn target(&self, namespace: &str) -> String {
        match self {
            Self::Managed(table) => table.qualified_name(namespace),
            Self::WorkflowFeatureRegistry => WORKFLOW_FEATURE_REGISTRY_RELATION.to_owned(),
            Self::WorkflowFeatureCache(relation) => relation.clone(),
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
    clear_registry: bool,
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
}

impl WorkflowBundleRegistry {
    fn load(connection: &mut StorageConnection) -> Result<Option<WorkflowBundleRegistry>, String> {
        if !connection.table_exists(WORKFLOW_FEATURE_REGISTRY_RELATION)? {
            return Ok(None);
        }
        let mut statement = connection
            .new_statement()
            .map_err(|error| format!("opening Workflow feature registry: {error}"))?;
        statement
            .set_sql_query(format!(
                "SELECT * FROM {WORKFLOW_FEATURE_REGISTRY_RELATION}"
            ))
            .map_err(|error| format!("binding Workflow feature registry: {error}"))?;
        let reader = statement
            .execute()
            .map_err(|error| format!("reading Workflow feature registry: {error}"))?;
        let schema = reader.schema();
        let mut batches = reader
            .map(|batch| {
                batch.map_err(|error| format!("reading Workflow feature registry: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if batches.is_empty() {
            batches.push(RecordBatch::new_empty(schema));
        }
        Self::validate(&batches).map(Some)
    }
}

impl ProjectBundle {
    pub(crate) fn export(
        &self,
        connection: &mut StorageConnection,
        namespace: &str,
    ) -> Result<Vec<u8>, String> {
        let mut queries = Vec::new();
        for table in Table::ALL {
            let relation = table.qualified_name(namespace);
            if !connection.table_exists(&relation)? {
                continue;
            }
            let query = if table == Table::VideoAssets {
                BundleSpec::portable_video_assets_query(&relation, None)
            } else {
                format!("SELECT * FROM {relation}")
            };
            queries.push((table.name().to_owned(), query));
        }
        if let Some(registry) = WorkflowBundleRegistry::load(connection)? {
            queries.push((
                WORKFLOW_FEATURE_REGISTRY_RELATION.to_owned(),
                format!("SELECT * FROM {WORKFLOW_FEATURE_REGISTRY_RELATION}"),
            ));
            for relation in registry.cache_relations() {
                if !connection.table_exists(&relation)? {
                    return Err(format!(
                        "Workflow feature registry declares missing cache relation {relation:?}"
                    ));
                }
                queries.push((relation.clone(), format!("SELECT * FROM {relation}")));
            }
        }
        PROJECT_SPEC
            .export_named_queries(connection, queries)
            .map(|export| export.bytes)
    }
}

impl EpisodeBundle {
    pub(crate) fn export(
        &self,
        connection: &mut StorageConnection,
        selection: EpisodeBundleSelection<'_>,
    ) -> Result<Vec<u8>, String> {
        let EpisodeBundleSelection {
            namespace,
            dataset_id,
        } = selection;
        EpisodeBundle::validate_dataset_id(dataset_id)?;
        let datasets = Table::Datasets.qualified_name(namespace);
        let episodes = Table::Episodes.qualified_name(namespace);
        if !connection.table_exists(&datasets)? || !connection.table_exists(&episodes)? {
            return Err(format!("dataset {dataset_id:?} has no episodes to export"));
        }
        let dataset = SpiralSql::string_literal(dataset_id);
        let episode_ids = format!(
            "SELECT {episode_id} FROM {episodes} WHERE {dataset_id_column} = {dataset}",
            episode_id = column::EPISODE_ID,
            dataset_id_column = column::DATASET_ID,
        );
        let mut queries = Vec::new();
        for table in EPISODE_TABLES {
            let relation = table.qualified_name(namespace);
            if !connection.table_exists(&relation)? {
                continue;
            }
            let query = match table {
                Table::Datasets | Table::Episodes => format!(
                    "SELECT * FROM {relation} WHERE {} = {dataset}",
                    column::DATASET_ID
                ),
                Table::EpisodeStreams | Table::JointSamples => format!(
                    "SELECT * FROM {relation} WHERE {} IN ({episode_ids})",
                    column::EPISODE_ID
                ),
                Table::VideoAssets => BundleSpec::portable_video_assets_query(
                    &relation,
                    Some(&format!("{} IN ({episode_ids})", column::EPISODE_ID)),
                ),
                _ => unreachable!("episode bundle table set is closed"),
            };
            queries.push((table, query));
        }
        let export = EPISODES_SPEC.export_queries(connection, queries)?;
        if export
            .row_counts
            .iter()
            .find_map(|(table, rows)| (*table == Table::Episodes).then_some(*rows))
            .unwrap_or(0)
            == 0
        {
            return Err(format!("dataset {dataset_id:?} has no episodes to export"));
        }
        Ok(export.bytes)
    }
}

impl BundleSpec {
    fn export_queries(
        &self,
        connection: &mut StorageConnection,
        queries: Vec<(Table, String)>,
    ) -> Result<ExportedBundle, String> {
        let export = self.export_named_queries(
            connection,
            queries
                .into_iter()
                .map(|(table, query)| (table.name().to_owned(), query))
                .collect(),
        )?;
        let row_counts = export
            .row_counts
            .into_iter()
            .map(|(name, rows)| {
                self.table_named(&name)
                    .map(|table| (table, rows))
                    .ok_or_else(|| {
                        format!("{} export contains unknown relation {name:?}", self.label)
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ExportedBundle {
            bytes: export.bytes,
            row_counts,
        })
    }

    fn export_named_queries(
        &self,
        connection: &mut StorageConnection,
        queries: Vec<(String, String)>,
    ) -> Result<NamedExportedBundle, String> {
        let mut entries = Vec::<(String, Vec<u8>)>::new();
        let mut row_counts = Vec::new();
        for (relation, query) in queries {
            let mut statement = connection.new_statement().map_err(|error| {
                format!("opening {} export for {relation}: {error}", self.label)
            })?;
            statement.set_sql_query(query).map_err(|error| {
                format!("binding {} export for {relation}: {error}", self.label)
            })?;
            let reader = statement
                .execute()
                .map_err(|error| format!("reading {} relation {relation}: {error}", self.label))?;
            let occupied = Self::encoded_entries_size(&entries)
                .saturating_add(self.magic.len() + 4 + MAX_MANIFEST_BYTES + 2);
            let payload_limit = self
                .max_bytes
                .saturating_sub(occupied)
                .saturating_sub(2 + relation.len() + 8);
            let mut payload = LimitedPayload::new(payload_limit, self.label);
            let mut rows = 0_u64;
            {
                let mut writer = StreamWriter::try_new(&mut payload, reader.schema().as_ref())
                    .map_err(|error| format!("opening Arrow stream for {relation}: {error}"))?;
                for batch in reader {
                    let batch = batch.map_err(|error| {
                        format!("reading {} relation {relation}: {error}", self.label)
                    })?;
                    rows = rows.saturating_add(batch.num_rows() as u64);
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
                .saturating_add(self.magic.len() + 4 + MAX_MANIFEST_BYTES);
            if projected_size > self.max_bytes {
                return Err(format!(
                    "{} export exceeds the {} MiB portable-file limit",
                    self.label,
                    self.max_bytes / (1024 * 1024),
                ));
            }
            row_counts.push((relation.clone(), rows));
            entries.push((relation, payload));
        }
        self.build(entries)
            .map(|bytes| NamedExportedBundle { bytes, row_counts })
    }
}

impl BundleSpec {
    fn portable_video_assets_query(relation: &str, predicate: Option<&str>) -> String {
        let filter = predicate
            .map(|predicate| format!(" WHERE {predicate}"))
            .unwrap_or_default();
        format!(
            "SELECT {episode_id}, {stream_id}, {segment_index}, {timeline_offset}, {camera_id}, \
         spql.io.read_bytes({payload}) AS {payload}, {byte_size}, {width}, {height}, {fps}, \
         {frames}, {media_relation} FROM {relation}{filter}",
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

impl BundleSpec {
    fn build(&self, entries: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
        if let Some((name, _)) = entries
            .iter()
            .find(|(name, _)| name.len() > MAX_ENTRY_NAME_BYTES)
        {
            return Err(format!("{} relation name {name:?} is too long", self.label));
        }
        let manifest = Manifest {
            format: self.format.to_owned(),
            format_version: FORMAT_VERSION,
            schema_version: spiral::SCHEMA_VERSION,
            exported_ms: BundleClock::unix_millis(),
            relations: entries.iter().map(|(name, _)| name.clone()).collect(),
        };
        let manifest = serde_json::to_vec(&manifest)
            .map_err(|error| format!("encoding {} manifest: {error}", self.label))?;
        let mut bundle = Vec::new();
        bundle.extend_from_slice(self.magic);
        BundleCodec::write_u32(&mut bundle, manifest.len())?;
        bundle.extend_from_slice(&manifest);
        for (name, payload) in entries {
            BundleCodec::write_u16(&mut bundle, name.len())?;
            bundle.extend_from_slice(name.as_bytes());
            BundleCodec::write_u64(&mut bundle, payload.len())?;
            bundle.extend_from_slice(&payload);
        }
        bundle.extend_from_slice(&0_u16.to_le_bytes());
        if bundle.len() > self.max_bytes {
            return Err(format!(
                "{} export exceeds the {} MiB portable-file limit",
                self.label,
                self.max_bytes / (1024 * 1024),
            ));
        }
        Ok(bundle)
    }
}

impl ProjectBundle {
    pub(crate) fn import(
        &self,
        connection: &mut StorageConnection,
        source: BundleSource<'_>,
    ) -> Result<ProjectImportSummary, String> {
        let BundleSource { namespace, bytes } = source;
        let decoded = ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(bytes)?)?;
        let fixed_relations = decoded
            .iter()
            .filter_map(|entry| match &entry.relation {
                ProjectRelation::Managed(table) => Some((*table, entry.batches.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        let imported_video_episodes = EpisodeBundle::video_episode_ids(&fixed_relations)?;
        let current_caches = WorkflowBundleRegistry::load(connection)?
            .map(|registry| registry.cache_relations())
            .unwrap_or_default();
        let incoming_caches = decoded
            .iter()
            .filter_map(|entry| match &entry.relation {
                ProjectRelation::WorkflowFeatureCache(relation) => Some(relation.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let incoming_registry = decoded
            .iter()
            .any(|entry| entry.relation == ProjectRelation::WorkflowFeatureRegistry);
        let workflow_plan = WorkflowRestorePlan::from_caches(
            &current_caches,
            incoming_registry.then_some(&incoming_caches),
        );

        let (mut summary, transaction_warning) =
            connection.run_project_import_transaction("project import", |connection| {
                // Retire only relations absent from the incoming project. Relations in
                // the bundle are atomically replaced below through hosted Arbor. All
                // namespace swaps remain staged in this one project transaction, so a
                // reader observes either the old project or the complete replacement.
                for table in Table::ALL.into_iter().rev() {
                    if decoded
                        .iter()
                        .any(|entry| entry.relation == ProjectRelation::Managed(table))
                    {
                        continue;
                    }
                    let relation = table.qualified_name(namespace);
                    if connection.table_exists(&relation)? {
                        connection
                            .execute_update_sql(&format!("DROP TABLE {relation}"))
                            .map_err(|error| {
                                format!("clearing project relation {relation}: {error}")
                            })?;
                    }
                }
                for relation in &workflow_plan.retire_caches {
                    if connection.table_exists(relation)? {
                        connection
                            .execute_update_sql(&format!("DROP TABLE {relation}"))
                            .map_err(|error| {
                                format!("clearing Workflow cache {relation}: {error}")
                            })?;
                    }
                }
                if workflow_plan.clear_registry
                    && connection.table_exists(WORKFLOW_FEATURE_REGISTRY_RELATION)?
                {
                    connection
                        .execute_update_sql(&format!(
                            "DROP TABLE {WORKFLOW_FEATURE_REGISTRY_RELATION}"
                        ))
                        .map_err(|error| format!("clearing Workflow feature registry: {error}"))?;
                }

                let mut summary = ProjectImportSummary::default();
                for entry in decoded {
                    let relation = entry.relation.target(namespace);
                    let batches = entry
                        .batches
                        .into_iter()
                        .map(|batch| {
                            if entry.relation == ProjectRelation::Managed(Table::VideoAssets) {
                                EpisodeBundle::prepare_video_asset_import(batch, namespace)
                            } else {
                                Ok(batch)
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    summary.record_relation(
                        batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64,
                    );
                    connection.replace_batches(&relation, batches)?;
                }
                Ok(summary)
            })?;
        let can_reconcile = transaction_warning.is_none();
        if let Some(warning) = transaction_warning {
            summary.push_warning(warning);
        }
        // ADBC ingest can create a relation inside the import transaction, but
        // Spiral's SQL binder cannot resolve that new relation until the commit is
        // published. Keep the project_store's exclusive checkout while reconciling
        // the derived MP4 resources immediately after the durable rows commit.
        if can_reconcile {
            if let Err(error) = EpisodeBundle::reconcile_project_video_dataset(
                connection,
                namespace,
                &imported_video_episodes,
            ) {
                summary.push_warning(format!(
                    "project rows committed, but the derived video dataset needs repair: {error}"
                ));
            }
        } else {
            summary.push_warning(
            "project rows committed, but derived video reconciliation was skipped because the transaction connection could not be reused"
                .to_owned(),
        );
        }
        Ok(summary)
    }
}

impl EpisodeBundle {
    pub(crate) fn import(
        &self,
        connection: &mut StorageConnection,
        request: EpisodeBundleImport<'_>,
    ) -> Result<EpisodeImportSummary, String> {
        let EpisodeBundleImport { selection, bytes } = request;
        let EpisodeBundleSelection {
            namespace,
            dataset_id,
        } = selection;
        EpisodeBundle::validate_dataset_id(dataset_id)?;
        let mut decoded = EPISODES_SPEC.decode_entries(EPISODES_SPEC.parse(bytes)?)?;
        EpisodeBundle::sort_relations(&mut decoded, &EPISODE_TABLES);
        let episode_ids = EpisodeBundle::validate_relations(&decoded)?;
        let video_episode_ids = EpisodeBundle::video_episode_ids(&decoded)?;
        let existing_video_episodes = connection.stored_video_episode_ids(namespace)?;
        EpisodeBundle::reject_existing_episodes(connection, namespace, &episode_ids)?;
        connection.ensure_dataset_namespace(namespace)?;
        if !video_episode_ids.is_empty() {
            connection.ensure_video_assets_blob_relation(namespace)?;
        }
        if !existing_video_episodes.is_empty() && !connection.video_dataset_exists(namespace)? {
            connection.retarget_stored_video_media_relations(namespace)?;
        }

        connection
            .set_autocommit(false)
            .map_err(|error| format!("starting episode import transaction: {error}"))?;
        let outcome = (|| {
            let datasets = Table::Datasets.qualified_name(namespace);
            let dataset_exists = connection.table_exists(&datasets)?
                && !connection
                    .run_query(&SpiralSql::dataset_exists(&datasets, dataset_id), 1)?
                    .rows()
                    .is_empty();
            let mut imported_rows = 0_u64;
            for (table, batches) in decoded {
                if table == Table::Datasets && dataset_exists {
                    continue;
                }
                let relation = table.qualified_name(namespace);
                let batches = batches
                    .into_iter()
                    .map(|batch| match table {
                        Table::Datasets | Table::Episodes => EpisodeBundle::replace_utf8_column(
                            batch,
                            column::DATASET_ID,
                            dataset_id,
                        ),
                        Table::VideoAssets => {
                            EpisodeBundle::prepare_video_asset_import(batch, namespace)
                        }
                        _ => Ok(batch),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                imported_rows =
                    imported_rows.saturating_add(
                        batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64,
                    );
                connection.ingest_batches(&relation, batches)?;
            }
            Ok(
                EpisodeImportSummary::new(dataset_id, episode_ids.len() as u64, imported_rows)
                    .with_first_episode(episode_ids.first().cloned()),
            )
        })();
        let (mut summary, transaction_warning) =
            connection.finish_transaction(outcome, "episode import")?;
        let can_reconcile = transaction_warning.is_none();
        if let Some(warning) = transaction_warning {
            summary.push_warning(warning);
        }
        if can_reconcile {
            if !video_episode_ids.is_empty()
                && let Err(error) =
                    EpisodeBundle::reconcile_appended_video_dataset(connection, namespace)
            {
                summary.push_warning(format!(
                    "episode rows committed, but the derived video dataset needs repair: {error}"
                ));
            }
        } else if !video_episode_ids.is_empty() {
            summary.push_warning(
            "episode rows committed, but derived video reconciliation was skipped because the transaction connection could not be reused"
                .to_owned(),
        );
        }
        Ok(summary)
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
    label: &'a str,
}

struct ArrowPreflight<'a> {
    relation: &'a str,
    label: &'a str,
    body: &'a [u8],
    limits: ArrowDecodeLimits,
}

impl<'a> ArrowPreflight<'a> {
    const fn new(relation: &'a str, label: &'a str, limits: ArrowDecodeLimits) -> Self {
        Self {
            relation,
            label,
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
    fn new(relation: &'a str, bytes: Vec<u8>, label: &'a str) -> Self {
        Self {
            relation,
            bytes,
            label,
        }
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
        let ArrowPayload {
            relation,
            bytes,
            label,
        } = payload;
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
                "{label} relation {relation} changes schema between Arrow batches"
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
        let label = payload.label;
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
                        "{label} Arrow stream for {relation} has bytes after its end marker"
                    ));
                }
                break;
            }
            let metadata_len = usize::try_from(metadata_len)
                .expect("Arrow metadata length always fits usize on supported targets");
            if metadata_len > limits.message_metadata_bytes {
                return Err(format!(
                    "{label} Arrow metadata for {relation} exceeds the {} MiB message limit",
                    limits.message_metadata_bytes / (1024 * 1024),
                ));
            }
            counters.claimed_decoded_bytes = counters
            .claimed_decoded_bytes
            .checked_add(metadata_len)
            .filter(|bytes| *bytes <= limits.decoded_bytes)
            .ok_or_else(|| {
                format!(
                    "{label} Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                    limits.decoded_bytes / (1024 * 1024),
                )
            })?;
            let metadata_end = position
                .checked_add(metadata_len)
                .filter(|end| *end <= payload.len())
                .ok_or_else(|| format!("{label} Arrow metadata for {relation} is truncated"))?;
            let message = arrow_ipc::root_as_message(&payload[position..metadata_end])
                .map_err(|error| format!("opening Arrow metadata for {relation}: {error}"))?;
            position = metadata_end;

            let body_len = usize::try_from(message.bodyLength()).map_err(|_| {
                format!("{label} Arrow message for {relation} has an invalid body length")
            })?;
            if body_len > limits.decoded_bytes {
                return Err(format!(
                    "{label} Arrow message body for {relation} exceeds the {} MiB decoded-memory limit",
                    limits.decoded_bytes / (1024 * 1024),
                ));
            }
            let body_end = position
                .checked_add(body_len)
                .filter(|end| *end <= payload.len())
                .ok_or_else(|| format!("{label} Arrow message body for {relation} is truncated"))?;
            let body = &payload[position..body_end];

            match message.header_type() {
                arrow_ipc::MessageHeader::Schema => {
                    let schema = message.header_as_schema().ok_or_else(|| {
                        format!("{label} Arrow schema metadata for {relation} is invalid")
                    })?;
                    ArrowPreflight::new(relation, label, limits).validate_schema(schema)?;
                }
                arrow_ipc::MessageHeader::RecordBatch => {
                    let batch = message.header_as_record_batch().ok_or_else(|| {
                        format!("{label} Arrow record metadata for {relation} is invalid")
                    })?;
                    ArrowPreflight::new(relation, label, limits)
                        .with_body(body)
                        .validate_record_batch(batch, &mut counters)?;
                }
                arrow_ipc::MessageHeader::DictionaryBatch => {
                    let dictionary = message.header_as_dictionary_batch().ok_or_else(|| {
                        format!("{label} Arrow dictionary metadata for {relation} is invalid")
                    })?;
                    let batch = dictionary.data().ok_or_else(|| {
                        format!("{label} Arrow dictionary for {relation} has no record metadata")
                    })?;
                    ArrowPreflight::new(relation, label, limits)
                        .with_body(body)
                        .validate_record_batch(batch, &mut counters)?;
                }
                _ => {}
            }
            position = body_end;
        }

        if counters.rows > limits.relation_rows {
            return Err(format!(
                "{label} Arrow relation {relation} exceeds the {}-row relation limit",
                limits.relation_rows,
            ));
        }
        self.claimed_decoded_bytes = self
        .claimed_decoded_bytes
        .checked_add(counters.claimed_decoded_bytes)
        .filter(|bytes| *bytes <= limits.decoded_bytes)
        .ok_or_else(|| {
            format!(
                "{label} Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                limits.decoded_bytes / (1024 * 1024),
            )
        })?;
        self.rows = self
            .rows
            .checked_add(counters.rows)
            .filter(|rows| *rows <= limits.decoded_rows)
            .ok_or_else(|| {
                format!(
                    "{label} Arrow data exceeds the {}-row decoded limit at relation {relation}",
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
        let label = self.label;
        let max_fields = self.limits.schema_fields;
        let Some(fields) = schema.fields() else {
            return Ok(());
        };
        if fields.len() > max_fields {
            return Err(format!(
                "{label} Arrow schema for {relation} exceeds the {max_fields}-column limit"
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
                        "{label} Arrow schema for {relation} exceeds the {max_fields}-field limit"
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
        let label = self.label;
        let body = self.body;
        let limits = self.limits;
        let batch_rows = usize::try_from(batch.length()).map_err(|_| {
            format!("{label} Arrow record batch for {relation} has an invalid row count")
        })?;
        if batch_rows > limits.record_batch_rows {
            return Err(format!(
                "{label} Arrow record batch for {relation} exceeds the {}-row batch limit",
                limits.record_batch_rows,
            ));
        }
        counters.rows = counters.rows.checked_add(batch_rows).ok_or_else(|| {
            format!("{label} Arrow row count overflows while reading relation {relation}")
        })?;

        let compressed = batch.compression().is_some();
        for buffer in batch.buffers().into_iter().flatten() {
            let offset = usize::try_from(buffer.offset()).map_err(|_| {
                format!("{label} Arrow buffer for {relation} has an invalid offset")
            })?;
            let length = usize::try_from(buffer.length()).map_err(|_| {
                format!("{label} Arrow buffer for {relation} has an invalid length")
            })?;
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= body.len())
                .ok_or_else(|| {
                    format!("{label} Arrow buffer for {relation} exceeds its message body")
                })?;
            let decoded_length = if length == 0 || !compressed {
                length
            } else {
                if length < 8 {
                    return Err(format!(
                        "{label} compressed Arrow buffer for {relation} has no length prefix"
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
                        "{label} compressed Arrow buffer for {relation} is too large for this machine"
                    )
                })?,
                _ => {
                    return Err(format!(
                        "{label} compressed Arrow buffer for {relation} has an invalid decoded length"
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
                    "{label} Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                    limits.decoded_bytes / (1024 * 1024),
                )
            })?;
            debug_assert!(end <= body.len());
        }
        Ok(())
    }
}

impl BundleSpec {
    fn decode_entries(
        &self,
        entries: Vec<(Table, Vec<u8>)>,
    ) -> Result<Vec<(Table, Vec<RecordBatch>)>, String> {
        let mut budget = ArrowDecodeBudget::default();
        self.decode_entries_with_budget(entries, &mut budget)
    }

    fn decode_entries_with_budget(
        &self,
        entries: Vec<(Table, Vec<u8>)>,
        budget: &mut ArrowDecodeBudget,
    ) -> Result<Vec<(Table, Vec<RecordBatch>)>, String> {
        let mut decoded = Vec::new();
        for (table, payload) in entries {
            let batches = budget.decode(ArrowPayload::new(table.name(), payload, self.label))?;
            let schema = batches[0].schema();
            table.validate_managed_relation_schema(schema.as_ref(), self.label)?;
            decoded.push((table, batches));
        }
        Ok(decoded)
    }
}

trait ManagedRelationSchema {
    fn managed_relation_schema(self) -> Schema;

    fn validate_managed_relation_schema(self, schema: &Schema, label: &str) -> Result<(), String>;

    fn managed_relation_field_matches(self, actual: &Field, expected: &Field) -> bool;
}

impl ManagedRelationSchema for Table {
    fn validate_managed_relation_schema(self, schema: &Schema, label: &str) -> Result<(), String> {
        let expected = self.managed_relation_schema();
        if !schema.metadata().is_empty() {
            return Err(format!(
                "{label} relation {self} has unsupported schema metadata"
            ));
        }
        if schema.fields().len() != expected.fields().len() {
            return Err(format!(
                "{label} relation {self} has {} columns; expected {}",
                schema.fields().len(),
                expected.fields().len(),
            ));
        }
        for (actual, expected) in schema.fields().iter().zip(expected.fields()) {
            if !self.managed_relation_field_matches(actual, expected) {
                return Err(format!(
                    "{label} relation {self} field {:?} has type {} (nullable {}); expected semantic type {} (nullable {})",
                    actual.name(),
                    actual.data_type(),
                    actual.is_nullable(),
                    expected.data_type(),
                    expected.is_nullable(),
                ));
            }
        }
        Ok(())
    }

    fn managed_relation_field_matches(self, actual: &Field, expected: &Field) -> bool {
        let portable_video_payload_is_required = self == Table::VideoAssets
            && actual.name() == column::PAYLOAD
            && expected.name() == column::PAYLOAD
            && !actual.is_nullable()
            && expected.is_nullable();
        (portable_video_payload_is_required || actual.is_nullable() == expected.is_nullable())
            && actual.name() == expected.name()
            && actual.metadata() == expected.metadata()
            && actual
                .data_type()
                .matches_managed_data_type(expected.data_type())
    }

    fn managed_relation_schema(self) -> Schema {
        let table = self;
        let text = |name, nullable| Field::new(name, DataType::Utf8, nullable);
        let timestamp = |name| Field::new(name, DataType::Int64, false);
        let snapshot = || {
            vec![
                text(column::RUN_ID, false),
                text(column::SNAPSHOT_JSON, false),
                timestamp(column::UPDATED_MS),
            ]
        };
        let fields = match table {
            Table::Datasets => vec![
                text(column::DATASET_ID, false),
                text(column::NAME, false),
                timestamp(column::CREATED_MS),
                text(column::DESCRIPTION, true),
            ],
            Table::Episodes => vec![
                text(column::EPISODE_ID, false),
                text(column::DATASET_ID, false),
                Field::new(column::SCHEMA_VERSION, DataType::UInt32, false),
                timestamp(column::STARTED_MS),
                Field::new(column::SAMPLE_HZ, DataType::Float64, false),
                text(column::SOURCE, false),
                text(column::NOTE, true),
                Field::new(column::FRAMES, DataType::UInt64, false),
                Field::new(column::DURATION_NS, DataType::UInt64, false),
                Field::new(column::MAX_FOLLOW_ERROR_DEGREES, DataType::Float64, true),
                Field::new(column::DISCARDED, DataType::Boolean, false),
                text(column::METADATA_JSON, false),
            ],
            Table::EpisodeStreams => vec![
                text(column::EPISODE_ID, false),
                text(column::STREAM_ID, false),
                text(column::MODALITY, false),
                text(column::ROBOT_TYPE, true),
                Field::new(
                    column::JOINT_NAMES,
                    DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
                    true,
                ),
                text(column::CAMERA_ID, true),
                Field::new(column::WIDTH, DataType::UInt32, true),
                Field::new(column::HEIGHT, DataType::UInt32, true),
                Field::new(column::FPS, DataType::UInt32, true),
            ],
            Table::JointSamples => {
                let float_list =
                    || DataType::List(Arc::new(Field::new("item", DataType::Float64, true)));
                vec![
                    text(column::EPISODE_ID, false),
                    text(column::STREAM_ID, false),
                    Field::new(column::SAMPLE_INDEX, DataType::UInt64, false),
                    Field::new(column::EPISODE_TIME_NS, DataType::UInt64, false),
                    Field::new(column::STATE_VALID, DataType::Boolean, false),
                    Field::new(column::ACTION_VALID, DataType::Boolean, false),
                    Field::new(column::STATE_POSITION, float_list(), false),
                    Field::new(column::ACTION_POSITION, float_list(), true),
                    Field::new(
                        column::RAW_POSITION,
                        DataType::List(Arc::new(Field::new("item", DataType::Int64, false))),
                        false,
                    ),
                    Field::new(column::VELOCITY, float_list(), true),
                    Field::new(column::EFFORT, float_list(), true),
                    Field::new(column::FOLLOW_ERROR_DEGREES, DataType::Float64, true),
                ]
            }
            Table::VideoAssets => vec![
                text(column::EPISODE_ID, false),
                text(column::STREAM_ID, false),
                Field::new(column::SEGMENT_INDEX, DataType::UInt32, false),
                Field::new(column::TIMELINE_OFFSET_NS, DataType::UInt64, true),
                text(column::CAMERA_ID, false),
                Field::new(column::PAYLOAD, DataType::Binary, true),
                Field::new(column::BYTE_SIZE, DataType::UInt64, false),
                Field::new(column::WIDTH, DataType::UInt32, false),
                Field::new(column::HEIGHT, DataType::UInt32, false),
                Field::new(column::FPS, DataType::UInt32, false),
                Field::new(column::FRAMES, DataType::UInt64, false),
                text(column::MEDIA_RELATION, false),
            ],
            Table::TrainingCheckpoints => vec![
                text(column::RUN_ID, false),
                text(column::RUN_NAME, false),
                text(column::POLICY, false),
                Field::new(column::STEP, DataType::UInt64, false),
                timestamp(column::CREATED_MS),
                Field::new(column::VALIDATION_LOSS, DataType::Float64, true),
                text(column::CONFIG_JSON, false),
                Field::new(column::CHECKPOINT, DataType::LargeBinary, false),
            ],
            Table::Robots => vec![
                text(column::ROBOT_ID, false),
                text(column::CONFIG_JSON, false),
                timestamp(column::UPDATED_MS),
            ],
            Table::StudioSettings => vec![
                text(column::KEY, false),
                text(column::VALUE_JSON, false),
                timestamp(column::UPDATED_MS),
            ],
            Table::TrainingRuns | Table::RolloutRuns | Table::WorkflowRuns => snapshot(),
        };
        Schema::new(fields)
    }
}

trait ManagedFieldCompatibility {
    fn matches_managed_field(&self, expected: &Field) -> bool;
}

impl ManagedFieldCompatibility for Field {
    fn matches_managed_field(&self, expected: &Field) -> bool {
        self.name() == expected.name()
            && self.is_nullable() == expected.is_nullable()
            && self.metadata() == expected.metadata()
            && self
                .data_type()
                .matches_managed_data_type(expected.data_type())
    }
}

trait ManagedDataTypeCompatibility {
    fn matches_managed_data_type(&self, expected: &DataType) -> bool;
}

impl ManagedDataTypeCompatibility for DataType {
    fn matches_managed_data_type(&self, expected: &DataType) -> bool {
        match expected {
            DataType::Utf8 => matches!(
                self,
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
            ),
            DataType::Binary | DataType::LargeBinary => matches!(
                self,
                DataType::Binary | DataType::LargeBinary | DataType::BinaryView
            ),
            DataType::List(expected_child) => {
                let DataType::List(actual_child) = self else {
                    return false;
                };
                actual_child.matches_managed_field(expected_child)
            }
            _ => self == expected,
        }
    }
}

impl ProjectBundle {
    fn decode_entries(
        entries: Vec<(String, Vec<u8>)>,
    ) -> Result<Vec<DecodedProjectRelation>, String> {
        let mut budget = ArrowDecodeBudget::default();
        let mut payloads = entries.into_iter().collect::<BTreeMap<_, _>>();
        let registry_batches = payloads
            .remove(WORKFLOW_FEATURE_REGISTRY_RELATION)
            .map(|payload| {
                budget.decode_with_row_limit(
                    ArrowPayload::new(
                        WORKFLOW_FEATURE_REGISTRY_RELATION,
                        payload,
                        PROJECT_SPEC.label,
                    ),
                    MAX_FEATURE_REGISTRY_ROWS,
                )
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
            .filter(|name| PROJECT_SPEC.table_named(name).is_none())
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
        for table in Table::ALL {
            if let Some(payload) = payloads.remove(table.name()) {
                let mut relation =
                    PROJECT_SPEC.decode_entries_with_budget(vec![(table, payload)], &mut budget)?;
                if table == Table::WorkflowRuns {
                    BundleAgentDocumentV2::validate_batches(&relation[0].1)?;
                }
                decoded.push(DecodedProjectRelation {
                    relation: ProjectRelation::Managed(table),
                    batches: relation.pop().expect("one project relation was decoded").1,
                });
            }
        }
        for relation in declared_caches {
            let payload = payloads
                .remove(&relation)
                .expect("declared project cache presence was checked");
            let batches = budget.decode_with_row_limit(
                ArrowPayload::new(&relation, payload, PROJECT_SPEC.label),
                MAX_FEATURE_CACHE_ROWS,
            )?;
            WorkflowCacheValidator::validate_bundle(&relation, &batches, &registry)?;
            decoded.push(DecodedProjectRelation {
                relation: ProjectRelation::WorkflowFeatureCache(relation.clone()),
                batches,
            });
        }
        if let Some(batches) = registry_batches {
            decoded.push(DecodedProjectRelation {
                relation: ProjectRelation::WorkflowFeatureRegistry,
                batches,
            });
        }
        debug_assert!(payloads.is_empty());
        Ok(decoded)
    }
}

impl BundleAgentDocumentV2 {
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
                BundleAgentDocumentV2::validate_json(snapshot)?;
            }
        }
        Ok(())
    }

    fn validate_json(encoded: &str) -> Result<(), String> {
        if encoded.len() > MAX_WORKFLOW_AGENT_DOCUMENT_BYTES {
            return Err("project Assistant history exceeds the 16 MiB bound".to_owned());
        }
        let value: serde_json::Value = serde_json::from_str(encoded)
            .map_err(|error| format!("project Assistant history is invalid JSON: {error}"))?;
        let Some(object) = value.as_object() else {
            return Err("project Assistant history must be a JSON object".to_owned());
        };

        let conversations = object.get("conversations").ok_or_else(|| {
            "project Assistant history does not match the current document shape".to_owned()
        })?;
        let canonical =
            serde_json::from_value::<BundleAgentDocumentV2>(value.clone()).map_err(|error| {
                format!(
                    "project Assistant history does not match the current document shape: {error}"
                )
            })?;
        if canonical.conversations.iter().any(|conversation| {
            !BundleAgentDocumentV2::provider_history_is_structurally_valid(
                &conversation.provider_messages,
                conversation.session.lifecycle.phase.active(),
            )
        }) {
            return Err(
                "project Assistant provider history has an invalid role or tool exchange"
                    .to_owned(),
            );
        }
        if canonical.schema_version != WORKFLOW_AGENT_DOCUMENT_SCHEMA_VERSION {
            return Err("project Assistant history uses an unsupported schema version".to_owned());
        }
        let conversations = conversations
            .as_array()
            .ok_or_else(|| "project Assistant conversations must be a JSON array".to_owned())?;
        if conversations.is_empty() || conversations.len() > MAX_WORKFLOW_AGENT_CONVERSATIONS {
            return Err(format!(
                "project Assistant history must contain 1–{MAX_WORKFLOW_AGENT_CONVERSATIONS} conversations"
            ));
        }
        let mut ids = BTreeSet::new();
        for conversation in conversations {
            let conversation = conversation
                .as_object()
                .ok_or_else(|| "project Assistant conversation must be a JSON object".to_owned())?;
            let id = BundleAgentDocumentV2::bounded_json_string(
                conversation.get("conversation_id"),
                128,
                "conversation id",
            )?;
            if !ids.insert(id) {
                return Err(
                    "project Assistant history has duplicate conversation identities".to_owned(),
                );
            }
            let title = BundleAgentDocumentV2::bounded_json_string(
                conversation.get("title"),
                MAX_WORKFLOW_AGENT_CONVERSATION_TITLE_CHARS,
                "conversation title",
            )?;
            if title.trim().is_empty() {
                return Err("project Assistant conversation title cannot be empty".to_owned());
            }
            let session = conversation
                .get("session")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| {
                    "project Assistant conversation is missing its session".to_owned()
                })?;
            if session
                .get("workflow_id")
                .and_then(serde_json::Value::as_str)
                != Some(DEFAULT_WORKFLOW_ID)
            {
                return Err(
                    "project Assistant session belongs to an unsupported Workflow".to_owned(),
                );
            }
            if session
                .get("conversation_id")
                .and_then(serde_json::Value::as_str)
                != Some(id)
            {
                return Err(
                    "project Assistant session identity does not match its conversation".to_owned(),
                );
            }
            BundleAgentDocumentV2::validate_agent_messages(session.get("messages"))?;
            BundleAgentDocumentV2::validate_provider_messages(
                conversation.get("provider_messages"),
            )?;
        }
        let active = object
            .get("active_conversation_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "project Assistant history has no active conversation".to_owned())?;
        if !ids.contains(active) {
            return Err("project Assistant active conversation does not exist".to_owned());
        }
        Ok(())
    }

    fn validate_agent_messages(messages: Option<&serde_json::Value>) -> Result<(), String> {
        let messages = messages
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "project Assistant messages must be a JSON array".to_owned())?;
        if messages.len() > MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION {
            return Err(format!(
                "project Assistant conversation exceeds its {MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION}-message bound"
            ));
        }
        let mut ids = BTreeSet::new();
        for message in messages {
            let id = message
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "project Assistant message has no identity".to_owned())?;
            if id.is_empty() || id.len() > 128 || !ids.insert(id) {
                return Err(
                    "project Assistant history has invalid or duplicate message identities"
                        .to_owned(),
                );
            }
        }
        Ok(())
    }

    fn validate_provider_messages(messages: Option<&serde_json::Value>) -> Result<(), String> {
        let Some(messages) = messages else {
            return Ok(());
        };
        let messages = messages
            .as_array()
            .ok_or_else(|| "project Assistant provider history must be a JSON array".to_owned())?;
        if messages.len() > MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION {
            return Err(format!(
                "project Assistant provider history exceeds its {MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION}-message bound"
            ));
        }
        Ok(())
    }

    fn provider_history_is_structurally_valid(
        messages: &[BundleProviderMessage],
        allow_incomplete_tail: bool,
    ) -> bool {
        let mut pending: Option<BTreeSet<&str>> = None;
        for message in messages {
            if !matches!(message.role.as_str(), "user" | "assistant" | "tool") {
                return false;
            }
            if let Some(expected) = pending.as_mut() {
                if message.role != "tool"
                    || message
                        .tool_call_id
                        .as_deref()
                        .is_none_or(|id| !expected.remove(id))
                {
                    return false;
                }
                if expected.is_empty() {
                    pending = None;
                }
                continue;
            }
            if message.role == "tool" {
                return false;
            }
            if !message.tool_calls.is_empty() {
                if message.role != "assistant" {
                    return false;
                }
                let expected = message
                    .tool_calls
                    .iter()
                    .map(|call| call.id.as_str())
                    .collect::<BTreeSet<_>>();
                if expected.len() != message.tool_calls.len()
                    || expected.iter().any(|id| id.is_empty())
                {
                    return false;
                }
                pending = Some(expected);
            }
        }
        pending.is_none() || allow_incomplete_tail
    }

    fn bounded_json_string<'a>(
        value: Option<&'a serde_json::Value>,
        max_chars: usize,
        label: &str,
    ) -> Result<&'a str, String> {
        let value = value
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("project Assistant {label} is invalid"))?;
        if value.is_empty() || value.chars().count() > max_chars {
            return Err(format!("project Assistant {label} exceeds its bound"));
        }
        Ok(value)
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

    fn validate_with_limits(
        batches: &[RecordBatch],
        max_rows: usize,
        max_features_per_cache: usize,
    ) -> Result<WorkflowBundleRegistry, String> {
        let mut registry = WorkflowBundleRegistry::default();
        let mut features_per_cache = BTreeMap::<String, usize>::new();
        let mut cache_sources = BTreeMap::<String, String>::new();
        for batch in batches {
            Self::validate_schema(batch.schema().as_ref())?;
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
                let expected =
                spiral_robotics_api::workflow::WorkflowFeatureApplicability::derived_cache_relation(
                    &source_relation,
                );
                if cache_relation != expected {
                    return Err(format!(
                        "Workflow feature registry cache relation {cache_relation:?} does not match server-derived identity {expected:?}"
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

    fn validate_schema(schema: &Schema) -> Result<(), String> {
        let expected = [
            ("source_relation", DataType::Utf8, false),
            ("cache_relation", DataType::Utf8, false),
            ("feature_name", DataType::Utf8, false),
            ("description", DataType::Utf8, false),
            ("concept", DataType::Utf8, false),
            ("media_field", DataType::Utf8, false),
            (
                "stream_ids",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
                false,
            ),
            ("producer_model_id", DataType::Utf8, false),
            ("producer_repository", DataType::Utf8, false),
            ("artifact_revision", DataType::Utf8, false),
            ("value_semantics", DataType::Utf8, false),
            ("definition_version", DataType::UInt32, false),
            ("created_at_ms", DataType::UInt64, false),
            ("updated_at_ms", DataType::UInt64, false),
        ];
        if schema.fields().len() != expected.len()
            || schema
                .fields()
                .iter()
                .zip(expected)
                .any(|(actual, (name, data_type, nullable))| {
                    actual.name() != name
                        || actual.is_nullable() != nullable
                        || !WorkflowArrow::storage_type_matches(actual.data_type(), &data_type)
                })
        {
            return Err(
                "Workflow feature registry has an incompatible canonical 14-field schema"
                    .to_owned(),
            );
        }
        Ok(())
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
                || (field.is_nullable() && !WorkflowArrow::cache_key_may_be_nullable(name))
                || !WorkflowArrow::storage_type_matches(field.data_type(), expected)
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
        let text = |name: &str| {
            batch
                .column_by_name(name)
                .and_then(|column| WorkflowArrow::string_value(column.as_ref(), row))
                .map(str::to_owned)
                .ok_or_else(|| format!("Workflow feature cache key {name:?} is null or invalid"))
        };
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
            episode_id: text("episode_id")?,
            stream_id: text("stream_id")?,
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

    fn storage_type_matches(actual: &DataType, expected: &DataType) -> bool {
        actual == expected
            || matches!(
                (actual, expected),
                (
                    DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View,
                    DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
                )
            )
            || match (actual, expected) {
                (DataType::List(actual), DataType::List(expected)) => {
                    actual.name() == expected.name()
                        && actual.is_nullable() == expected.is_nullable()
                        && actual.metadata() == expected.metadata()
                        && Self::storage_type_matches(actual.data_type(), expected.data_type())
                }
                _ => false,
            }
    }
}

impl WorkflowRestorePlan {
    fn from_caches(
        current_caches: &BTreeSet<String>,
        incoming_caches: Option<&BTreeSet<String>>,
    ) -> WorkflowRestorePlan {
        let empty = BTreeSet::new();
        let incoming = incoming_caches.unwrap_or(&empty);
        WorkflowRestorePlan {
            retire_caches: current_caches.difference(incoming).cloned().collect(),
            clear_registry: incoming_caches.is_none(),
        }
    }
}

trait ProjectImportTransaction {
    fn begin_project_import(&mut self) -> Result<(), String>;
    fn commit_project_import(&mut self) -> Result<(), String>;
    fn rollback_project_import(&mut self) -> Result<(), String>;
    fn restore_project_autocommit(&mut self) -> Result<(), String>;
    fn discard_project_connection(&mut self);
}

impl ProjectImportTransaction for StorageConnection {
    fn begin_project_import(&mut self) -> Result<(), String> {
        self.set_autocommit(false)
    }

    fn commit_project_import(&mut self) -> Result<(), String> {
        self.commit_catalog_transaction()
    }

    fn rollback_project_import(&mut self) -> Result<(), String> {
        self.rollback()
    }

    fn restore_project_autocommit(&mut self) -> Result<(), String> {
        self.set_autocommit(true)
    }

    fn discard_project_connection(&mut self) {
        self.discard();
    }
}

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
                Ok(()) => match connection.restore_project_autocommit() {
                    Ok(()) => return Err(primary),
                    Err(restore) => {
                        connection.discard_project_connection();
                        return Err(format!(
                            "{primary}; restoring ADBC autocommit after {operation} also failed: {restore}; connection discarded"
                        ));
                    }
                },
                Err(rollback) => {
                    connection.discard_project_connection();
                    return Err(format!(
                        "{primary}; rolling back {operation} also failed: {rollback}; connection discarded"
                    ));
                }
            },
        };

        if let Err(commit) = connection.commit_project_import() {
            let rollback = connection.rollback_project_import();
            connection.discard_project_connection();
            return Err(match rollback {
                Ok(()) => format!(
                    "committing {operation} failed with an uncertain outcome; connection discarded: {commit}"
                ),
                Err(rollback) => format!(
                    "committing {operation} failed with an uncertain outcome: {commit}; rolling back also failed: {rollback}; connection discarded"
                ),
            });
        }

        match connection.restore_project_autocommit() {
            Ok(()) => Ok((value, None)),
            Err(restore) => {
                connection.discard_project_connection();
                Ok((
                    value,
                    Some(format!(
                        "{operation} committed, but its Spiral connection was discarded because restoring ADBC autocommit failed: {restore}"
                    )),
                ))
            }
        }
    }
}

impl<C: ProjectImportTransaction> ProjectImportTransactionRunner for C {}

impl StorageConnection {
    fn finish_transaction<T>(
        &mut self,
        outcome: Result<T, String>,
        operation: &str,
    ) -> Result<(T, Option<String>), String> {
        let connection = self;
        let final_result = match outcome {
            Ok(value) => connection
                .commit_catalog_transaction()
                .map(|()| value)
                .map_err(|error| format!("committing {operation}: {error}")),
            Err(error) => {
                let rollback = connection.rollback();
                Err(match rollback {
                    Ok(()) => error,
                    Err(rollback) => format!("{error}; rolling back also failed: {rollback}"),
                })
            }
        };
        let autocommit = connection
            .set_autocommit(true)
            .map_err(|error| format!("restoring Spiral autocommit after {operation}: {error}"));
        match (final_result, autocommit) {
            (Ok(value), Ok(())) => Ok((value, None)),
            (Err(error), _) => Err(error),
            (Ok(value), Err(error)) => {
                // The data is already durable. Report that truth to the operator,
                // but never return the connection to the pool with unknown
                // transaction state.
                connection.discard();
                Ok((
                    value,
                    Some(format!(
                        "{operation} committed, but its Spiral connection was discarded: {error}"
                    )),
                ))
            }
        }
    }
}

impl EpisodeBundle {
    fn sort_relations(decoded: &mut [(Table, Vec<RecordBatch>)], order: &[Table]) {
        decoded.sort_by_key(|(table, _)| {
            order
                .iter()
                .position(|candidate| candidate == table)
                .unwrap_or(usize::MAX)
        });
    }

    fn validate_relations(decoded: &[(Table, Vec<RecordBatch>)]) -> Result<Vec<String>, String> {
        if !decoded.iter().any(|(table, _)| *table == Table::Datasets)
            || !decoded.iter().any(|(table, _)| *table == Table::Episodes)
        {
            return Err("episode bundle must contain datasets and episodes relations".to_owned());
        }
        let dataset_rows = decoded
            .iter()
            .filter(|(table, _)| *table == Table::Datasets)
            .flat_map(|(_, batches)| batches)
            .map(RecordBatch::num_rows)
            .sum::<usize>();
        if dataset_rows != 1 {
            return Err("episode bundle must describe exactly one source dataset".to_owned());
        }
        let mut source_datasets = Vec::new();
        for (_, batches) in decoded
            .iter()
            .filter(|(table, _)| *table == Table::Datasets)
        {
            for batch in batches {
                source_datasets.extend(EpisodeBundle::utf8_values(batch, column::DATASET_ID)?);
            }
        }
        let source_dataset = source_datasets
            .pop()
            .ok_or_else(|| "episode bundle source dataset has no dataset ID".to_owned())?;
        EpisodeBundle::validate_dataset_id(&source_dataset)?;
        let mut episode_ids = BTreeSet::new();
        for (table, batches) in decoded {
            if *table == Table::Episodes {
                for batch in batches {
                    let ids = EpisodeBundle::utf8_values(batch, column::EPISODE_ID)?;
                    let datasets = EpisodeBundle::utf8_values(batch, column::DATASET_ID)?;
                    if datasets.iter().any(|dataset| dataset != &source_dataset) {
                        return Err(format!(
                            "episode bundle contains an episode outside source dataset {source_dataset:?}"
                        ));
                    }
                    for episode_id in ids {
                        EpisodeBundle::validate_episode_id(&episode_id)?;
                        if !episode_ids.insert(episode_id.clone()) {
                            return Err(format!(
                                "episode bundle repeats episode ID {episode_id:?}"
                            ));
                        }
                    }
                }
            }
        }
        if episode_ids.is_empty() {
            return Err("episode bundle contains no episodes".to_owned());
        }
        for (table, batches) in decoded {
            if matches!(
                table,
                Table::EpisodeStreams | Table::JointSamples | Table::VideoAssets
            ) {
                for batch in batches {
                    for episode_id in EpisodeBundle::utf8_values(batch, column::EPISODE_ID)? {
                        if !episode_ids.contains(&episode_id) {
                            return Err(format!(
                                "episode bundle relation {table} references unknown episode {episode_id:?}"
                            ));
                        }
                    }
                }
            }
        }
        Ok(episode_ids.into_iter().collect())
    }

    fn video_episode_ids(
        decoded: &[(Table, Vec<RecordBatch>)],
    ) -> Result<BTreeSet<String>, String> {
        let mut episode_ids = BTreeSet::new();
        for (_, batches) in decoded
            .iter()
            .filter(|(table, _)| *table == Table::VideoAssets)
        {
            for batch in batches {
                for episode_id in EpisodeBundle::utf8_values(batch, column::EPISODE_ID)? {
                    EpisodeBundle::validate_episode_id(&episode_id)?;
                    episode_ids.insert(episode_id);
                }
            }
        }
        Ok(episode_ids)
    }

    fn reconcile_project_video_dataset(
        connection: &mut StorageConnection,
        namespace: &str,
        imported: &BTreeSet<String>,
    ) -> Result<(), String> {
        let dataset_exists = connection.video_dataset_exists(namespace)?;
        if imported.is_empty() {
            connection.drop_video_dataset(namespace)?;
        } else {
            connection.rebuild_video_dataset(namespace, dataset_exists)?;
        }
        Ok(())
    }

    fn reconcile_appended_video_dataset(
        connection: &mut StorageConnection,
        namespace: &str,
    ) -> Result<(), String> {
        let dataset_exists = connection.video_dataset_exists(namespace)?;
        connection.rebuild_video_dataset(namespace, dataset_exists)?;
        Ok(())
    }

    fn utf8_values(batch: &RecordBatch, name: &str) -> Result<Vec<String>, String> {
        let index = batch
            .schema()
            .index_of(name)
            .map_err(|error| format!("episode bundle is missing column {name}: {error}"))?;
        let values = batch.column(index);
        if let Some(values) = values.as_any().downcast_ref::<StringArray>() {
            return EpisodeBundle::collect_utf8(values.iter(), name);
        }
        if let Some(values) = values.as_any().downcast_ref::<LargeStringArray>() {
            return EpisodeBundle::collect_utf8(values.iter(), name);
        }
        if let Some(values) = values.as_any().downcast_ref::<StringViewArray>() {
            return EpisodeBundle::collect_utf8(values.iter(), name);
        }
        Err(format!("episode bundle column {name} is not UTF-8 text"))
    }

    fn collect_utf8<'a>(
        values: impl Iterator<Item = Option<&'a str>>,
        name: &str,
    ) -> Result<Vec<String>, String> {
        values
            .map(|value| {
                value
                    .map(str::to_owned)
                    .ok_or_else(|| format!("episode bundle column {name} contains a null value"))
            })
            .collect()
    }

    fn replace_utf8_column(
        batch: RecordBatch,
        name: &str,
        value: &str,
    ) -> Result<RecordBatch, String> {
        let index = batch
            .schema()
            .index_of(name)
            .map_err(|error| format!("episode bundle is missing column {name}: {error}"))?;
        let mut columns = batch.columns().to_vec();
        let source = &columns[index];
        columns[index] = if source.as_any().is::<StringArray>() {
            Arc::new(StringArray::from(vec![value; batch.num_rows()])) as ArrayRef
        } else if source.as_any().is::<LargeStringArray>() {
            Arc::new(LargeStringArray::from(vec![value; batch.num_rows()])) as ArrayRef
        } else if source.as_any().is::<StringViewArray>() {
            Arc::new(StringViewArray::from(vec![value; batch.num_rows()])) as ArrayRef
        } else {
            return Err(format!("episode bundle column {name} is not UTF-8 text"));
        };
        RecordBatch::try_new(batch.schema(), columns)
            .map_err(|error| format!("retargeting episode bundle dataset: {error}"))
    }

    fn retarget_video_media_relations(
        batch: RecordBatch,
        namespace: &str,
    ) -> Result<RecordBatch, String> {
        let media_relation = SpiralSql::video_dataset_output(namespace);
        let index = batch
            .schema()
            .index_of(column::MEDIA_RELATION)
            .map_err(|error| {
                format!(
                    "episode bundle is missing column {}: {error}",
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
                "episode bundle column {} is not UTF-8 text",
                column::MEDIA_RELATION
            ));
        };
        RecordBatch::try_new(batch.schema(), columns)
            .map_err(|error| format!("retargeting episode bundle media relations: {error}"))
    }

    fn prepare_video_asset_import(
        batch: RecordBatch,
        namespace: &str,
    ) -> Result<RecordBatch, String> {
        EpisodeBundle::retarget_video_media_relations(batch.normalize_video_asset()?, namespace)
    }

    fn reject_existing_episodes(
        connection: &mut StorageConnection,
        namespace: &str,
        episode_ids: &[String],
    ) -> Result<(), String> {
        let episodes = Table::Episodes.qualified_name(namespace);
        if !connection.table_exists(&episodes)? {
            return Ok(());
        }
        for chunk in episode_ids.chunks(100) {
            let candidates = chunk
                .iter()
                .map(|episode_id| SpiralSql::string_literal(episode_id))
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "SELECT {episode_id} FROM {episodes} WHERE {episode_id} IN ({candidates})",
                episode_id = column::EPISODE_ID,
            );
            let existing = connection.run_query(&query, chunk.len())?;
            if let Some(value) = existing
                .rows()
                .first()
                .and_then(|row| row.get(column::EPISODE_ID))
                .and_then(serde_json::Value::as_str)
            {
                return Err(format!(
                    "episode {value:?} already exists; episode import never overwrites existing episodes"
                ));
            }
        }
        Ok(())
    }

    fn validate_dataset_id(dataset_id: &str) -> Result<(), String> {
        if !spiral::StorageSchema::is_dataset_id(dataset_id) {
            return Err(
                "dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'".to_owned(),
            );
        }
        Ok(())
    }

    fn validate_episode_id(episode_id: &str) -> Result<(), String> {
        if episode_id.is_empty()
            || episode_id.len() > 128
            || !episode_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(format!(
                "episode bundle contains invalid episode ID {episode_id:?}"
            ));
        }
        Ok(())
    }
}

impl BundleSpec {
    fn parse(&self, bundle: &[u8]) -> Result<Vec<(Table, Vec<u8>)>, String> {
        self.parse_raw(bundle)?
            .into_iter()
            .map(|(name, payload)| {
                self.table_named(&name)
                    .map(|table| (table, payload))
                    .ok_or_else(|| format!("{} contains unknown relation {name:?}", self.label))
            })
            .collect()
    }

    fn parse_raw(&self, bundle: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
        if bundle.len() > self.max_bytes {
            return Err(format!(
                "{} file exceeds the {} MiB import limit",
                self.label,
                self.max_bytes / (1024 * 1024),
            ));
        }
        let mut source = Cursor::new(bundle);
        let mut magic = [0_u8; 8];
        source
            .read_exact(&mut magic)
            .map_err(|_| format!("{} file is truncated", self.label))?;
        if &magic != self.magic {
            return Err(format!("file is not a Spiral Robotics {}", self.label));
        }
        let manifest_len = BundleCodec::read_u32(&mut source)? as usize;
        if manifest_len == 0 || manifest_len > MAX_MANIFEST_BYTES {
            return Err(format!("{} manifest has an invalid size", self.label));
        }
        let mut manifest = vec![0_u8; manifest_len];
        source
            .read_exact(&mut manifest)
            .map_err(|_| format!("{} manifest is truncated", self.label))?;
        let manifest: Manifest = serde_json::from_slice(&manifest)
            .map_err(|error| format!("{} manifest is invalid: {error}", self.label))?;
        if manifest.format != self.format || manifest.format_version != FORMAT_VERSION {
            return Err(format!(
                "{} file uses an unsupported format version",
                self.label
            ));
        }
        if manifest.schema_version != spiral::SCHEMA_VERSION {
            return Err(format!(
                "{} schema {} cannot be imported by schema {}",
                self.label,
                manifest.schema_version,
                spiral::SCHEMA_VERSION
            ));
        }

        let relation_count = manifest.relations.len();
        let mut expected = manifest.relations.into_iter().collect::<BTreeSet<_>>();
        if expected.len() != relation_count {
            return Err(format!(
                "{} manifest contains duplicate relations",
                self.label
            ));
        }
        if self.format != PROJECT_FORMAT
            && let Some(name) = expected
                .iter()
                .find(|name| self.table_named(name).is_none())
        {
            return Err(format!(
                "{} manifest contains unknown relation {name:?}",
                self.label
            ));
        }
        let mut seen = BTreeSet::new();
        let mut entries = BTreeMap::new();
        loop {
            let name_len = BundleCodec::read_u16(&mut source)? as usize;
            if name_len == 0 {
                break;
            }
            if name_len > MAX_ENTRY_NAME_BYTES {
                return Err(format!("{} relation name is too long", self.label));
            }
            let mut name = vec![0_u8; name_len];
            source
                .read_exact(&mut name)
                .map_err(|_| format!("{} relation name is truncated", self.label))?;
            let name = String::from_utf8(name)
                .map_err(|_| format!("{} relation name is not UTF-8", self.label))?;
            if !expected.remove(&name) || !seen.insert(name.clone()) {
                return Err(format!(
                    "{} contains unexpected or duplicate relation {name:?}",
                    self.label
                ));
            }
            let payload_len = BundleCodec::read_u64(&mut source)?;
            let payload_len = usize::try_from(payload_len).map_err(|_| {
                format!(
                    "{} relation {name:?} is too large for this machine",
                    self.label
                )
            })?;
            let remaining = bundle.len().saturating_sub(source.position() as usize);
            if payload_len > remaining {
                return Err(format!("{} relation {name:?} is truncated", self.label));
            }
            let mut payload = vec![0_u8; payload_len];
            source
                .read_exact(&mut payload)
                .map_err(|_| format!("{} relation {name:?} is truncated", self.label))?;
            entries.insert(name, payload);
        }
        if !expected.is_empty() {
            return Err(format!(
                "{} is missing relations: {}",
                self.label,
                expected.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        if source.position() != bundle.len() as u64 {
            return Err(format!("{} file has trailing bytes", self.label));
        }
        Ok(entries.into_iter().collect())
    }

    fn table_named(&self, name: &str) -> Option<Table> {
        self.tables
            .iter()
            .copied()
            .find(|table| table.name() == name)
    }
}

struct LimitedPayload {
    bytes: Vec<u8>,
    limit: usize,
    label: &'static str,
}

impl LimitedPayload {
    fn new(limit: usize, label: &'static str) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            label,
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
                "{} export exceeds the {} MiB portable-file limit",
                self.label,
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
    use crate::adbc::ProjectStore;
    use crate::episodes::{EpisodeCompletion, EpisodeStart, EpisodeStore};
    use crate::manager::StorageManager;
    use crate::settings::{BackendStateStore, ServiceEndpoint, SpiralConnectionSettings};

    fn envelope(manifest: Manifest, entries: &[(&str, &[u8])]) -> Vec<u8> {
        envelope_for(PROJECT_MAGIC, manifest, entries)
    }

    fn envelope_for(magic: &[u8; 8], manifest: Manifest, entries: &[(&str, &[u8])]) -> Vec<u8> {
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut bundle = Vec::new();
        bundle.extend_from_slice(magic);
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
        manifest_for(PROJECT_FORMAT, relations)
    }

    fn manifest_for(format: &str, relations: Vec<String>) -> Manifest {
        Manifest {
            format: format.to_owned(),
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
                ArrowPayload::new("compressed_bomb", payload, "project"),
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
            .decode_with_limits(
                ArrowPayload::new("first_relation", payload.clone(), "project"),
                limits,
            )
            .unwrap();

        let error = budget
            .decode_with_limits(
                ArrowPayload::new("second_relation", payload, "project"),
                limits,
            )
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
                ArrowPayload::new("video_assets", payload.clone(), "project"),
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
                ArrowPayload::new("too_many_rows", arrow_payload(&batch), "project"),
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
                    "project",
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
        let rows = 250_001;
        let batch = record_batch(
            Table::Datasets
                .columns()
                .iter()
                .map(|name| Field::new(*name, DataType::Null, true))
                .collect(),
            Table::Datasets
                .columns()
                .iter()
                .map(|_| Arc::new(NullArray::new(rows)) as ArrayRef)
                .collect(),
        );
        let payload = arrow_payload_batches(&[batch.clone(), batch.clone(), batch.clone(), batch]);
        assert!(payload.len() < 16 * 1_024);

        let error = PROJECT_SPEC
            .decode_entries(vec![(Table::Datasets, payload)])
            .unwrap_err();

        assert!(
            error.contains("1000000-row") && error.contains("limit"),
            "{error}"
        );
    }

    #[test]
    fn managed_schema_accepts_required_binary_view_for_portable_video_payload() {
        let mut fields = Table::VideoAssets
            .managed_relation_schema()
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        let payload = Table::VideoAssets
            .columns()
            .iter()
            .position(|name| *name == column::PAYLOAD)
            .expect("video assets contain a payload field");
        fields[payload] = Field::new(column::PAYLOAD, DataType::BinaryView, false);

        Table::VideoAssets
            .validate_managed_relation_schema(&Schema::new(fields), PROJECT_SPEC.label)
            .expect("a required portable payload safely satisfies the nullable binary contract");
    }

    #[test]
    fn managed_decode_rejects_semantically_wrong_field_types() {
        let batch = record_batch(
            Table::Datasets
                .columns()
                .iter()
                .map(|name| Field::new(*name, DataType::Null, true))
                .collect(),
            Table::Datasets
                .columns()
                .iter()
                .map(|_| Arc::new(NullArray::new(1)) as ArrayRef)
                .collect(),
        );

        let error = PROJECT_SPEC
            .decode_entries(vec![(Table::Datasets, arrow_payload(&batch))])
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
                ArrowPayload::new("too_many_columns", arrow_payload(&batch), "project"),
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
        BundleAgentDocumentV2::validate_json(&valid.to_string()).unwrap();

        let mut schema_less = valid.clone();
        schema_less
            .as_object_mut()
            .unwrap()
            .remove("schema_version");
        assert!(
            BundleAgentDocumentV2::validate_json(&schema_less.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut unknown_selection = valid.clone();
        unknown_selection["active_conversation_id"] = serde_json::json!("missing");
        assert!(
            BundleAgentDocumentV2::validate_json(&unknown_selection.to_string())
                .unwrap_err()
                .contains("active conversation")
        );

        let mut duplicate = valid.clone();
        duplicate["conversations"] =
            serde_json::json!([agent_conversation("same"), agent_conversation("same")]);
        duplicate["active_conversation_id"] = serde_json::json!("same");
        assert!(
            BundleAgentDocumentV2::validate_json(&duplicate.to_string())
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
            BundleAgentDocumentV2::validate_json(&over_limit.to_string())
                .unwrap_err()
                .contains("1–20")
        );

        let mut missing_timestamp = valid.clone();
        missing_timestamp["conversations"][0]
            .as_object_mut()
            .unwrap()
            .remove("created_ms");
        assert!(
            BundleAgentDocumentV2::validate_json(&missing_timestamp.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut unknown_nested = valid.clone();
        unknown_nested["conversations"][0]["session"]["messages"][0]["future_field"] =
            serde_json::json!(true);
        assert!(
            BundleAgentDocumentV2::validate_json(&unknown_nested.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut unknown_top_level = valid;
        unknown_top_level["future_field"] = serde_json::json!(true);
        assert!(
            BundleAgentDocumentV2::validate_json(&unknown_top_level.to_string())
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
            BundleAgentDocumentV2::validate_json(&foreign_workflow.to_string())
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
            let error = BundleAgentDocumentV2::validate_json(&injected.to_string()).unwrap_err();
            assert!(error.contains("invalid role"), "unexpected error: {error}");
        }
    }

    #[test]
    fn project_agent_history_rejects_legacy_and_future_shapes() {
        let legacy = serde_json::json!({
            "revision": 1,
            "session": {
                "workflow_id": "default",
                "revision": 1,
                "phase": "idle",
                "messages": [agent_message("legacy-message")],
                "updated_ms": 1
            },
            "provider_messages": [{"role": "user", "content": "Inspect relations"}]
        });
        assert!(
            BundleAgentDocumentV2::validate_json(&legacy.to_string())
                .unwrap_err()
                .contains("current document shape")
        );
        assert!(
            BundleAgentDocumentV2::validate_json(&serde_json::json!({"future": []}).to_string())
                .unwrap_err()
                .contains("current document shape")
        );
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
                Field::new("snapshot_json", DataType::Utf8, false),
                Field::new("updated_ms", DataType::Int64, false),
            ],
            vec![
                Arc::new(StringArray::from(vec!["agent:default"])) as ArrayRef,
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
                    spiral_robotics_api::workflow::WorkflowFeatureApplicability::derived_cache_relation(definition.applicability().source_relation())
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
        let mut names = vec![WORKFLOW_FEATURE_REGISTRY_RELATION.to_owned()];
        names.extend(cache_payloads.iter().map(|(name, _)| name.clone()));
        let mut entries = vec![(
            WORKFLOW_FEATURE_REGISTRY_RELATION,
            registry_payload.as_slice(),
        )];
        entries.extend(
            cache_payloads
                .iter()
                .map(|(name, payload)| (name.as_str(), payload.as_slice())),
        );
        envelope(manifest(names), &entries)
    }

    #[test]
    fn empty_project_envelope_is_valid() {
        let bundle = envelope(manifest(Vec::new()), &[]);
        assert_eq!(PROJECT_SPEC.parse(&bundle).unwrap(), Vec::new());
        assert!(
            ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(&bundle).unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn manifest_and_payload_relations_must_match_exactly() {
        let name = Table::Datasets.name().to_owned();
        let missing = envelope(manifest(vec![name.clone()]), &[]);
        assert!(
            PROJECT_SPEC
                .parse(&missing)
                .unwrap_err()
                .contains("missing relations")
        );

        let unexpected = envelope(manifest(Vec::new()), &[(name.as_str(), b"arrow")]);
        assert!(
            PROJECT_SPEC
                .parse(&unexpected)
                .unwrap_err()
                .contains("unexpected or duplicate")
        );
    }

    #[test]
    fn duplicate_manifest_relations_are_rejected() {
        let name = Table::Datasets.name().to_owned();
        let bundle = envelope(manifest(vec![name.clone(), name]), &[]);
        assert!(
            PROJECT_SPEC
                .parse(&bundle)
                .unwrap_err()
                .contains("duplicate relations")
        );
    }

    #[test]
    fn project_and_episode_bundle_formats_are_not_interchangeable() {
        let project = envelope(manifest(Vec::new()), &[]);
        assert!(
            EPISODES_SPEC
                .parse(&project)
                .unwrap_err()
                .contains("not a Spiral Robotics episode bundle")
        );

        let episodes = envelope_for(
            EPISODES_MAGIC,
            manifest_for(EPISODES_FORMAT, Vec::new()),
            &[],
        );
        assert!(
            PROJECT_SPEC
                .parse(&episodes)
                .unwrap_err()
                .contains("not a Spiral Robotics project")
        );
    }

    #[test]
    fn episode_bundle_rejects_project_only_relations() {
        let robots = Table::Robots.name().to_owned();
        let bundle = envelope_for(
            EPISODES_MAGIC,
            manifest_for(EPISODES_FORMAT, vec![robots]),
            &[],
        );
        assert!(
            EPISODES_SPEC
                .parse(&bundle)
                .unwrap_err()
                .contains("unknown relation")
        );
    }

    #[test]
    fn episode_bundle_requires_unique_ids_and_closed_references() {
        let dataset = text_batch(&[(column::DATASET_ID, &["source"])]);
        let duplicate_episodes = text_batch(&[
            (column::EPISODE_ID, &["episode_1", "episode_1"]),
            (column::DATASET_ID, &["source", "source"]),
        ]);
        let duplicate = vec![
            (Table::Datasets, vec![dataset.clone()]),
            (Table::Episodes, vec![duplicate_episodes]),
        ];
        assert!(
            EpisodeBundle::validate_relations(&duplicate)
                .unwrap_err()
                .contains("repeats episode ID")
        );

        let episodes = text_batch(&[
            (column::EPISODE_ID, &["episode_1"]),
            (column::DATASET_ID, &["source"]),
        ]);
        let unknown_child = text_batch(&[(column::EPISODE_ID, &["episode_2"])]);
        let dangling = vec![
            (Table::Datasets, vec![dataset]),
            (Table::Episodes, vec![episodes]),
            (Table::JointSamples, vec![unknown_child]),
        ];
        assert!(
            EpisodeBundle::validate_relations(&dangling)
                .unwrap_err()
                .contains("references unknown episode")
        );
    }

    #[test]
    fn episode_bundle_requires_one_consistent_source_dataset() {
        let decoded = vec![
            (
                Table::Datasets,
                vec![text_batch(&[(column::DATASET_ID, &["source"])])],
            ),
            (
                Table::Episodes,
                vec![text_batch(&[
                    (column::EPISODE_ID, &["episode_1"]),
                    (column::DATASET_ID, &["other"]),
                ])],
            ),
        ];
        assert!(
            EpisodeBundle::validate_relations(&decoded)
                .unwrap_err()
                .contains("outside source dataset")
        );
    }

    #[test]
    fn imported_video_assets_are_retargeted_to_the_destination_namespace() {
        let batch = text_batch(&[
            (column::EPISODE_ID, &["episode_a", "episode-b"]),
            (
                column::MEDIA_RELATION,
                &["source.video_a.assets", "source.video_b.assets"],
            ),
        ]);

        let retargeted =
            EpisodeBundle::retarget_video_media_relations(batch, "destination").unwrap();

        assert_eq!(
            EpisodeBundle::utf8_values(&retargeted, column::MEDIA_RELATION).unwrap(),
            vec![SpiralSql::video_dataset_output("destination"); 2]
        );
        assert_eq!(
            EpisodeBundle::video_episode_ids(&[(Table::VideoAssets, vec![retargeted])]).unwrap(),
            BTreeSet::from(["episode-b".to_owned(), "episode_a".to_owned()])
        );
    }

    #[test]
    fn forged_payload_length_is_rejected_before_allocation() {
        let name = Table::Datasets.name().to_owned();
        let mut bundle = envelope(manifest(vec![name.clone()]), &[(name.as_str(), b"")]);
        let length_offset = bundle.len() - 2 - 8;
        bundle[length_offset..length_offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        let error = PROJECT_SPEC.parse(&bundle).unwrap_err();
        assert!(error.contains("too large for this machine") || error.contains("truncated"));
    }

    #[test]
    fn export_payload_writer_enforces_its_bound() {
        let mut payload = LimitedPayload::new(3, "project");
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
            spiral_robotics_api::workflow::WorkflowFeatureApplicability::derived_cache_relation(
                source,
            );
        let definition = workflow_definition(source, "holding_feature");
        let registry = canonical_registry_batch(std::slice::from_ref(&definition));
        let cache_batch = canonical_cache_batch(std::slice::from_ref(&definition), &[1]);
        let valid = workflow_project_bundle(&registry, &[(cache.as_str(), cache_batch.clone())]);
        let decoded = ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(&valid).unwrap())
            .expect("a registry and its exact declared cache must decode");
        assert_eq!(
            decoded
                .iter()
                .map(|entry| entry.relation.bundle_name())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([WORKFLOW_FEATURE_REGISTRY_RELATION, cache.as_str()])
        );

        let registry_payload = arrow_payload(&registry);
        let missing = envelope(
            manifest(vec![WORKFLOW_FEATURE_REGISTRY_RELATION.to_owned()]),
            &[(WORKFLOW_FEATURE_REGISTRY_RELATION, &registry_payload)],
        );
        assert!(
            ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(&missing).unwrap())
                .unwrap_err()
                .contains("missing declared Workflow cache")
        );

        let undeclared_name = "workflow.features_undeclared";
        let cache_payload = arrow_payload(&cache_batch);
        let undeclared = envelope(
            manifest(vec![
                WORKFLOW_FEATURE_REGISTRY_RELATION.to_owned(),
                cache.clone(),
                undeclared_name.to_owned(),
            ]),
            &[
                (WORKFLOW_FEATURE_REGISTRY_RELATION, &registry_payload),
                (cache.as_str(), &cache_payload),
                (undeclared_name, &cache_payload),
            ],
        );
        assert!(
            ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(&undeclared).unwrap())
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
            Arc::new(StringArray::from(vec!["workflow.features_forged"])),
        );
        let payload = arrow_payload(&registry);
        let bundle = envelope(
            manifest(vec![WORKFLOW_FEATURE_REGISTRY_RELATION.to_owned()]),
            &[(WORKFLOW_FEATURE_REGISTRY_RELATION, &payload)],
        );
        assert!(
            ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(&bundle).unwrap())
                .unwrap_err()
                .contains("does not match server-derived identity")
        );
    }

    #[test]
    fn project_registry_requires_canonical_shape_rows_and_provenance() {
        let permissive_old_shape = text_batch(&[
            ("source_relation", &["media.frames"]),
            ("cache_relation", &["workflow.features_media_frames"]),
        ]);
        let bundle = workflow_project_bundle(&permissive_old_shape, &[]);
        assert!(
            ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(&bundle).unwrap())
                .unwrap_err()
                .contains("canonical 14-field schema")
        );

        let definition = workflow_definition("media.frames", "holding_feature");
        let forged = replace_batch_column(
            &canonical_registry_batch(std::slice::from_ref(&definition)),
            7,
            Arc::new(StringArray::from(vec!["forged-model"])),
        );
        let bundle = workflow_project_bundle(&forged, &[]);
        assert!(
            ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(&bundle).unwrap())
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
        let relation =
            spiral_robotics_api::workflow::WorkflowFeatureApplicability::derived_cache_relation(
                "media.frames",
            );

        let missing_feature = canonical_cache_batch(&[], &[1]);
        let bundle = workflow_project_bundle(
            &registry_batch,
            &[(relation.as_str(), missing_feature.clone())],
        );
        assert!(
            ProjectBundle::decode_entries(PROJECT_SPEC.parse_raw(&bundle).unwrap())
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
        assert!(WorkflowArrow::storage_type_matches(&normalized, &expected));
        assert!(!WorkflowArrow::storage_type_matches(
            &DataType::List(Arc::new(Field::new("item", DataType::BinaryView, false,))),
            &expected
        ));
        assert!(!WorkflowArrow::storage_type_matches(
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
        let relation =
            spiral_robotics_api::workflow::WorkflowFeatureApplicability::derived_cache_relation(
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
            "workflow.features_a".to_owned(),
            "workflow.features_b".to_owned(),
        ]);
        let incoming = BTreeSet::from([
            "workflow.features_b".to_owned(),
            "workflow.features_c".to_owned(),
        ]);
        assert_eq!(
            WorkflowRestorePlan::from_caches(&current, Some(&incoming)),
            WorkflowRestorePlan {
                retire_caches: vec!["workflow.features_a".to_owned()],
                clear_registry: false,
            }
        );
        assert_eq!(
            WorkflowRestorePlan::from_caches(&current, None),
            WorkflowRestorePlan {
                retire_caches: vec![
                    "workflow.features_a".to_owned(),
                    "workflow.features_b".to_owned(),
                ],
                clear_registry: true,
            }
        );
    }

    #[derive(Default)]
    struct MockProjectTransaction {
        events: Vec<&'static str>,
        begin_error: Option<&'static str>,
        commit_error: Option<&'static str>,
        rollback_error: Option<&'static str>,
        restore_error: Option<&'static str>,
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

        fn restore_project_autocommit(&mut self) -> Result<(), String> {
            self.events.push("restore");
            self.restore_error
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
        assert_eq!(success.events, ["begin", "mutation", "commit", "restore"]);
        assert!(!success.discarded);

        let mut failure = MockProjectTransaction::default();
        let error = failure
            .run_project_import_transaction::<()>("project import", |connection| {
                connection.events.push("mutation");
                Err("mutation failed".to_owned())
            })
            .unwrap_err();
        assert_eq!(error, "mutation failed");
        assert_eq!(failure.events, ["begin", "mutation", "rollback", "restore"]);
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
        assert_eq!(
            uncertain_commit.events,
            ["begin", "commit", "rollback", "discard"]
        );

        let mut commit = MockProjectTransaction {
            commit_error: Some("commit failed"),
            rollback_error: Some("rollback after commit failed"),
            ..Default::default()
        };
        let error = commit
            .run_project_import_transaction("project import", |_| Ok(()))
            .unwrap_err();
        assert!(
            error.contains("commit failed")
                && error.contains("rollback after commit failed")
                && error.contains("uncertain outcome")
        );
        assert_eq!(commit.events, ["begin", "commit", "rollback", "discard"]);

        let mut rollback_restore = MockProjectTransaction {
            restore_error: Some("restore failed"),
            ..Default::default()
        };
        let error = rollback_restore
            .run_project_import_transaction::<()>("project import", |_| {
                Err("mutation failed".to_owned())
            })
            .unwrap_err();
        assert!(error.contains("mutation failed") && error.contains("restore failed"));
        assert_eq!(
            rollback_restore.events,
            ["begin", "rollback", "restore", "discard"]
        );
    }

    #[test]
    fn project_transaction_returns_committed_value_with_restore_warning() {
        let mut transaction = MockProjectTransaction {
            restore_error: Some("restore failed"),
            ..Default::default()
        };
        let (value, warning) = transaction
            .run_project_import_transaction("project import", |_| Ok(9))
            .unwrap();
        assert_eq!(value, 9);
        assert!(warning.unwrap().contains("restore failed"));
        assert_eq!(
            transaction.events,
            ["begin", "commit", "restore", "discard"]
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
    #[test]
    #[ignore = "requires a real SpiralDB server, SpFS, and disposable project access"]
    fn spiral_export_import_round_trip_overwrites_existing_data() {
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let project_store = configured_project_store(project, namespace);
        let _cleanup = TestNamespace::new(project_store.clone());
        let storage = StorageManager::new(project_store.clone());

        project_store
            .run_exclusive_blocking(|connection| {
                connection.ensure_dataset_namespace(project_store.namespace())?;
                connection.ingest_batch(
                    &Table::StudioSettings.qualified_name(project_store.namespace()),
                    settings_batch("exported-setting", r#"{"source":"export"}"#, 10),
                )?;
                connection.ingest_batch(
                    &Table::TrainingCheckpoints.qualified_name(project_store.namespace()),
                    checkpoint_batch(),
                )
            })
            .expect("seeding the exported Spiral project must succeed");

        let exported = storage
            .export_project_bundle()
            .expect("exporting the Spiral project must succeed");
        let exported_payloads = relation_payloads(&exported);
        assert_eq!(
            exported_payloads.keys().cloned().collect::<Vec<_>>(),
            vec![
                Table::StudioSettings.name().to_owned(),
                Table::TrainingCheckpoints.name().to_owned(),
            ]
        );

        project_store
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, project_store.namespace())?;
                connection.ingest_batch(
                    &Table::StudioSettings.qualified_name(project_store.namespace()),
                    settings_batch("replacement-setting", r#"{"source":"replacement"}"#, 20),
                )?;
                connection.ingest_batch(
                    &Table::RolloutRuns.qualified_name(project_store.namespace()),
                    snapshot_batch("replacement-run"),
                )
            })
            .expect("seeding conflicting Spiral data must succeed");

        let replacement = storage
            .export_project_bundle()
            .expect("exporting the replacement state must succeed");
        assert_ne!(relation_payloads(&replacement), exported_payloads);

        let summary = storage
            .import_project_bundle(&exported)
            .expect("importing the Spiral project must succeed");
        assert_eq!(summary.relations(), 2);
        assert_eq!(summary.rows(), 2);

        let restored = storage
            .export_project_bundle()
            .expect("re-exporting the imported Spiral project must succeed");
        assert_eq!(relation_payloads(&restored), exported_payloads);
        project_store
            .run_blocking(|connection| {
                assert!(
                    !connection.table_exists(
                        &Table::RolloutRuns.qualified_name(project_store.namespace())
                    )?
                );
                Ok(())
            })
            .expect("checking overwrite semantics must succeed");
    }

    /// Run with the same real-Spiral environment as the project transfer E2E.
    #[test]
    #[ignore = "requires a real SpiralDB server, SpFS, and disposable project access"]
    fn spiral_episode_export_import_round_trip_merges_without_overwrite() {
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_episode_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let project_store = configured_project_store(project, namespace);
        let _cleanup = TestNamespace::new(project_store.clone());
        let storage = StorageManager::new(project_store.clone());

        project_store
            .run_exclusive_blocking(|connection| {
                connection.ensure_dataset_namespace(project_store.namespace())?;
                EpisodeSeed::new(project_store.namespace(), "source", "episode_exported")
                    .with_started_ms(10)
                    .save(connection)
            })
            .expect("seeding the source episode must succeed");

        let exported = storage
            .export_episode_bundle("source")
            .expect("exporting source episodes must succeed");
        let exported_relations = EPISODES_SPEC
            .parse(&exported)
            .expect("the exported episode bundle must parse")
            .into_iter()
            .map(|(table, _)| table)
            .collect::<Vec<_>>();
        assert_eq!(exported_relations, vec![Table::Datasets, Table::Episodes]);

        project_store
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, project_store.namespace())?;
                EpisodeSeed::new(project_store.namespace(), "target", "episode_existing")
                    .with_started_ms(20)
                    .save(connection)
            })
            .expect("seeding the target dataset must succeed");

        let summary = storage
            .import_episode_bundle("target", &exported)
            .expect("importing episodes into the target dataset must succeed");
        assert_eq!(summary.dataset_id(), "target");
        assert_eq!(summary.episodes(), 1);
        assert_eq!(summary.first_episode(), Some("episode_exported"));

        project_store
            .run_blocking(|connection| {
                let episodes = Table::Episodes.qualified_name(project_store.namespace());
                let result = connection.run_query(
                    &format!(
                        "SELECT {episode_id}, {dataset_id} FROM {episodes} ORDER BY {episode_id}",
                        episode_id = column::EPISODE_ID,
                        dataset_id = column::DATASET_ID,
                    ),
                    10,
                )?;
                assert_eq!(result.rows().len(), 2);
                assert!(result.rows().iter().all(|row| {
                    row.get(column::DATASET_ID)
                        .and_then(serde_json::Value::as_str)
                        == Some("target")
                }));
                Ok(())
            })
            .expect("the imported and pre-existing episodes must both remain");

        let collision = storage
            .import_episode_bundle("target", &exported)
            .expect_err("a repeated episode import must refuse the ID collision");
        assert!(collision.contains("never overwrites existing episodes"));
    }

    /// Run with the same real-Spiral environment as the project transfer E2E.
    #[test]
    #[ignore = "requires a real SpiralDB server, SpFS, disposable project access, and FFmpeg"]
    fn spiral_project_import_rebuilds_the_shared_managed_video_dataset() {
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_video_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let project_store = configured_project_store(project, namespace);
        let _cleanup = TestNamespace::new(project_store.clone());
        let storage = StorageManager::new(project_store.clone());
        let episode_id = "episode_video_import";
        let start = EpisodeStart::new(episode_id.to_owned(), "video-transfer".to_owned())
            .with_timing(10, 2.0)
            .with_source("transfer-e2e".to_owned(), None)
            .with_arms(
                vec![spiral_robotics_api::recording::EpisodeArm::new(
                    "follower",
                    Some("test".to_owned()),
                    vec!["joint".to_owned()],
                )],
                Vec::new(),
            );
        let completion = EpisodeCompletion::new(1.0, Vec::new(), Vec::new());
        let mut mp4 = test_mp4();
        // A valid top-level `free` box makes the fixture large enough to cross
        // Spiral's managed-Blob outlining threshold without making FFmpeg
        // encode hundreds of thousands of incompressible video bytes.
        let free_box_size = 300_000_u32;
        mp4.extend_from_slice(&free_box_size.to_be_bytes());
        mp4.extend_from_slice(b"free");
        mp4.resize(mp4.len() + free_box_size as usize - 8, 0);
        assert!(mp4.len() > 256 * 1024);

        project_store
            .run_exclusive_blocking(|connection| {
                connection.ensure_dataset_namespace(project_store.namespace())?;
                connection.ingest_batch(
                    &Table::Datasets.qualified_name(project_store.namespace()),
                    start.dataset_batch().map_err(|error| error.to_string())?,
                )?;
                connection.ingest_batch(
                    &Table::Episodes.qualified_name(project_store.namespace()),
                    completion
                        .episode_batch(&start, 0)
                        .map_err(|error| error.to_string())?,
                )?;
                connection.ingest_batch(
                    &Table::VideoAssets.qualified_name(project_store.namespace()),
                    video_asset_batch(episode_id, &mp4),
                )?;
                connection.ensure_video_assets_blob_relation(project_store.namespace())
            })
            .expect("legacy Binary video rows must migrate to Blob without derived resources");
        let exported = storage
            .export_project_bundle()
            .expect("exporting the video project must succeed");
        project_store
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, project_store.namespace())
            })
            .expect("clearing the relational fixture before import must succeed");

        let first_import = storage
            .import_project_bundle(&exported)
            .expect("importing a video project must rebuild its managed dataset");
        assert!(
            first_import.warnings().is_empty(),
            "first import warnings: {:?}",
            first_import.warnings()
        );
        let second_import = storage
            .import_project_bundle(&exported)
            .expect("re-importing a video project must replace its managed dataset outputs");
        assert!(
            second_import.warnings().is_empty(),
            "second import warnings: {:?}",
            second_import.warnings()
        );

        project_store
            .run_blocking(|connection| {
                let assets = Table::VideoAssets.qualified_name(project_store.namespace());
                let schema = connection
                    .table_schema(&assets)?
                    .ok_or_else(|| "imported video asset relation is missing".to_owned())?;
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
                let descriptor = connection.run_query(
                    &format!(
                        "SELECT spql.blob.uri({payload}) AS uri FROM {assets}",
                        payload = column::PAYLOAD,
                    ),
                    1,
                )?;
                assert!(
                    descriptor.rows()[0]
                        .get("uri")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|uri| !uri.is_empty()),
                    "large MP4 must be outlined into a provider-backed Blob",
                );
                let range = connection.run_binary_query(&SpiralSql::video_asset_range(
                    project_store.namespace(),
                    &crate::video::VideoAssetKey::new(
                        episode_id.to_owned(),
                        "camera.main".to_owned(),
                        Some(0),
                    ),
                    crate::video::VideoByteRange::new(512, 1024),
                ))?;
                assert_eq!(range, mp4[512..1536]);

                let output = SpiralSql::video_dataset_output(project_store.namespace());
                let rows = connection.run_query(&SpiralSql::count_rows(&output), 1)?;
                assert_eq!(
                    rows.rows()[0]
                        .get("rows")
                        .and_then(serde_json::Value::as_u64),
                    Some(1)
                );
                let frames = connection.run_query(
                    &format!(
                        "SELECT COUNT(*) AS count FROM {output} AS asset, \
                         LATERAL spql.video.frames(asset.media, \
                         spql.video.frame_indices(ARRAY[0])) AS frame"
                    ),
                    1,
                )?;
                assert_eq!(
                    frames.rows()[0]
                        .get("count")
                        .and_then(serde_json::Value::as_u64),
                    Some(1)
                );
                Ok(())
            })
            .expect("the rebuilt managed dataset must serve an indexed video frame");
    }

    /// Profile the production project-import algorithm against a real archive
    /// in an isolated namespace of the configured Spiral project.
    ///
    /// ```text
    /// SPIRAL_PROJECT_BUNDLE=/absolute/path/to/project.spiral-project \
    /// cargo test -p spiral-robotics-backend \
    ///   backend::storage::bundle::tests::spiral_profile_project_import \
    ///   -- --ignored --exact --nocapture
    /// ```
    #[test]
    #[ignore = "profiles a real project import and creates temporary Spiral resources"]
    fn spiral_profile_project_import() {
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
        let namespace = format!(
            "spiral_robotics_import_profile_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository_start = Instant::now();
        let project_store = configured_project_store(project, namespace);
        profile_event("studio.repository_config", repository_start, (0, 0));
        let cleanup = TestNamespace::new(project_store.clone());

        let namespace_start = Instant::now();
        project_store
            .run_exclusive_blocking(|connection| {
                connection.ensure_dataset_namespace(project_store.namespace())
            })
            .expect("the profiling namespace must be created");
        profile_event("spiral.setup_namespace", namespace_start, (0, 0));

        let exclusive_start = Instant::now();
        let summary = project_store
            .run_exclusive_blocking(|connection| {
                profile_project_import_into(connection, project_store.namespace(), &bytes)
            })
            .expect("the profiled project import must succeed");
        profile_event(
            "studio.exclusive_import_total",
            exclusive_start,
            (summary.rows() as usize, bytes.len()),
        );

        let storage = StorageManager::new(project_store.clone());
        let preferences_start = Instant::now();
        storage
            .load_studio_preferences()
            .expect("profiled preferences reload must succeed");
        profile_event("studio.reload_preferences", preferences_start, (0, 0));
        let training_start = Instant::now();
        storage
            .load_training_snapshot::<spiral_robotics_api::training::TrainingSnapshot>()
            .expect("profiled training reload must succeed");
        profile_event("studio.reload_training", training_start, (0, 0));
        let rollout_start = Instant::now();
        storage
            .load_rollout_snapshot::<spiral_robotics_api::rollout::RolloutSnapshot>()
            .expect("profiled rollout reload must succeed");
        profile_event("studio.reload_rollout", rollout_start, (0, 0));
        let catalog_start = Instant::now();
        let episodes = EpisodeStore::new(project_store.clone()).list();
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
        drop(cleanup);
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
    #[test]
    #[ignore = "profiles real hosted Arbor replacements in temporary Spiral resources"]
    fn spiral_profile_write_fixed_cost() {
        let bundle_path = std::env::var("SPIRAL_PROJECT_BUNDLE")
            .expect("SPIRAL_PROJECT_BUNDLE must name the project archive to profile");
        let bytes = fs::read(&bundle_path).expect("the profiling archive must be readable");
        let mut decoded = PROJECT_SPEC
            .decode_entries(
                PROJECT_SPEC
                    .parse(&bytes)
                    .expect("the profiling archive must parse"),
            )
            .expect("the profiling archive must decode");
        EpisodeBundle::sort_relations(&mut decoded, &Table::ALL);

        let project = required_project();
        let namespace = format!(
            "spiral_robotics_replace_profile_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let project_store = configured_project_store(project, namespace);
        let cleanup = TestNamespace::new(project_store.clone());

        project_store
            .run_exclusive_blocking(|connection| {
                connection.ensure_dataset_namespace(project_store.namespace())?;
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
                    if std::env::var("SPIRAL_PROFILE_CASE")
                        .is_ok_and(|requested| requested != label)
                    {
                        continue;
                    }
                    let source = decoded
                        .iter()
                        .find_map(|(candidate, batches)| (*candidate == table).then_some(batches))
                        .ok_or_else(|| format!("profiling archive omits {}", table.name()))?;
                    let batches = profile_prefix_batches(source, row_limit);
                    let rows = batches.iter().map(RecordBatch::num_rows).sum::<usize>();
                    let encoded_bytes = profile_encoded_bytes(&batches)?;
                    let target = format!("{}.{label}", project_store.namespace());
                    let ingest_target = format!("{}.{label}_ingest", project_store.namespace());
                    let autocommit_target =
                        format!("{}.{label}_autocommit", project_store.namespace());

                    ProfileWrite::new(label, "ingest_missing_autocommit", &autocommit_target)
                        .with_batches(&batches, (rows, encoded_bytes))
                        .write_autocommit(connection)?;
                    ProfileWrite::new(label, "append_existing_autocommit", &autocommit_target)
                        .with_batches(&batches, (rows, encoded_bytes))
                        .write_autocommit(connection)?;
                    ProfileWrite::new(label, "ingest_missing", &ingest_target)
                        .with_batches(&batches, (rows, encoded_bytes))
                        .write_transaction(connection)?;
                    ProfileWrite::new(label, "replace_missing", &target)
                        .with_batches(&batches, (rows, encoded_bytes))
                        .with_replace()
                        .write_transaction(connection)?;
                    ProfileWrite::new(label, "existing_cold", &target)
                        .with_batches(&batches, (rows, encoded_bytes))
                        .with_replace()
                        .write_transaction(connection)?;

                    for bind_index in 1..=10 {
                        let bind_start = Instant::now();
                        let exists = connection.table_schema(&target)?.is_some();
                        eprintln!(
                            "PROFILE_SCOPED\tcase={label}\tphase=bind_existing_{bind_index}\tseconds={:.6}\trows={rows}\tbytes={encoded_bytes}\texists={exists}",
                            bind_start.elapsed().as_secs_f64()
                        );
                    }
                    let missing_target = format!("{}.missing", project_store.namespace());
                    for bind_index in 1..=5 {
                        let bind_start = Instant::now();
                        let exists = connection.table_schema(&missing_target)?.is_some();
                        eprintln!(
                            "PROFILE_SCOPED\tcase={label}\tphase=bind_missing_{bind_index}\tseconds={:.6}\trows=0\tbytes=0\texists={exists}",
                            bind_start.elapsed().as_secs_f64()
                        );
                    }
                    ProfileWrite::new(label, "existing_after_bind", &target)
                        .with_batches(&batches, (rows, encoded_bytes))
                        .with_replace()
                        .write_transaction(connection)?;
                }
                Ok(())
            })
            .expect("scoped Spiral replacement profiling must succeed");

        let cleanup_start = Instant::now();
        drop(cleanup);
        profile_event("spiral.cleanup", cleanup_start, (0, 0));
    }

    struct ProfileWrite<'a> {
        label: &'a str,
        phase: &'a str,
        target: &'a str,
        batches: &'a [RecordBatch],
        metrics: (usize, usize),
        replace: bool,
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

        fn write_autocommit(self, connection: &mut StorageConnection) -> Result<(), String> {
            let write_start = Instant::now();
            connection.ingest_batches(self.target, self.batches.to_vec())?;
            let write_seconds = write_start.elapsed().as_secs_f64();
            let (rows, bytes) = self.metrics;
            let label = self.label;
            let phase = self.phase;
            eprintln!(
                "PROFILE_SCOPED\tcase={label}\tphase={phase}\tseconds={write_seconds:.6}\trows={rows}\tbytes={bytes}"
            );
            Ok(())
        }

        fn write_transaction(self, connection: &mut StorageConnection) -> Result<(), String> {
            let begin_start = Instant::now();
            connection.set_autocommit(false)?;
            let begin_seconds = begin_start.elapsed().as_secs_f64();

            let write_start = Instant::now();
            if self.replace {
                connection.replace_batches(self.target, self.batches.to_vec())?;
            } else {
                connection.ingest_batches(self.target, self.batches.to_vec())?;
            }
            let write_seconds = write_start.elapsed().as_secs_f64();

            let commit_start = Instant::now();
            connection.commit_catalog_transaction()?;
            let commit_seconds = commit_start.elapsed().as_secs_f64();
            connection.set_autocommit(true)?;
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

    fn profile_project_import_into(
        connection: &mut StorageConnection,
        namespace: &str,
        bundle: &[u8],
    ) -> Result<ProjectImportSummary, String> {
        let parse_start = Instant::now();
        let entries = PROJECT_SPEC.parse(bundle)?;
        profile_event("studio.bundle_parse", parse_start, (0, bundle.len()));

        let mut decoded = Vec::new();
        for (table, payload) in entries {
            let payload_bytes = payload.len();
            let decode_start = Instant::now();
            let mut relation = PROJECT_SPEC.decode_entries(vec![(table, payload)])?;
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
        EpisodeBundle::sort_relations(&mut decoded, &Table::ALL);
        if let Some(limit) = profile_usize("SPIRAL_PROFILE_VIDEO_LIMIT") {
            limit_relation_rows(&mut decoded, Table::VideoAssets, limit);
        }

        let video_ids_start = Instant::now();
        let imported_video_episodes = EpisodeBundle::video_episode_ids(&decoded)?;
        profile_event(
            "studio.collect_video_episode_ids",
            video_ids_start,
            (imported_video_episodes.len(), 0),
        );
        let transaction_start = Instant::now();
        connection
            .set_autocommit(false)
            .map_err(|error| format!("starting profiled project import transaction: {error}"))?;
        profile_event("spiral.transaction_begin", transaction_start, (0, 0));

        let outcome = (|| {
            for table in Table::ALL.into_iter().rev() {
                if decoded.iter().any(|(incoming, _)| *incoming == table) {
                    continue;
                }
                let relation = table.qualified_name(namespace);
                let exists_start = Instant::now();
                let exists = connection.table_exists(&relation)?;
                profile_event(
                    &format!("spiral.table_exists.{}", table.name()),
                    exists_start,
                    (0, 0),
                );
                if exists {
                    let drop_start = Instant::now();
                    connection.execute_update_sql(&format!("DROP TABLE {relation}"))?;
                    profile_event(
                        &format!("spiral.drop_relation.{}", table.name()),
                        drop_start,
                        (0, 0),
                    );
                }
            }

            let video_chunk_rows = profile_usize("SPIRAL_PROFILE_VIDEO_CHUNK_ROWS");
            let mut summary = ProjectImportSummary::default();
            for (table, batches) in decoded {
                let relation = table.qualified_name(namespace);
                let relation_ingest_start = Instant::now();
                let mut relation_rows = 0usize;
                let mut relation_bytes = 0usize;
                let mut stream_batches = Vec::new();
                for batch in batches {
                    let transform_start = Instant::now();
                    let batch = if table == Table::VideoAssets {
                        EpisodeBundle::prepare_video_asset_import(batch, namespace)?
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
                connection.replace_batches(&relation, stream_batches)?;
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
        })();

        let mut summary = match outcome {
            Ok(summary) => {
                let commit_start = Instant::now();
                connection
                    .commit_catalog_transaction()
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
                let rollback = connection.rollback();
                profile_event("spiral.transaction_rollback", rollback_start, (0, 0));
                return Err(match rollback {
                    Ok(()) => error,
                    Err(rollback) => format!("{error}; rolling back also failed: {rollback}"),
                });
            }
        };
        let autocommit_start = Instant::now();
        if let Err(error) = connection.set_autocommit(true) {
            connection.discard();
            summary.push_warning(format!(
                "profiled project import committed, but its Spiral connection was discarded: {error}"
            ));
        }
        profile_event(
            "spiral.transaction_restore_autocommit",
            autocommit_start,
            (0, 0),
        );

        if !profile_bool("SPIRAL_PROFILE_SKIP_VIDEO_RECONCILE") {
            let reconcile_total_start = Instant::now();
            if imported_video_episodes.is_empty() {
                connection.drop_video_dataset(namespace)?;
            } else {
                let _ = profile_rebuild_video_dataset(connection, namespace)?;
            }
            profile_event(
                "spiral.reconcile_video_total",
                reconcile_total_start,
                (imported_video_episodes.len(), 0),
            );
        }
        Ok(summary)
    }

    fn profile_rebuild_video_dataset(
        connection: &mut StorageConnection,
        namespace: &str,
    ) -> Result<bool, String> {
        let exists_start = Instant::now();
        let dataset_exists = connection.video_dataset_exists(namespace)?;
        profile_event("spiral.reconcile_video_exists", exists_start, (1, 0));

        let sources = SpiralSql::video_sources_relation(namespace);
        let assets = Table::VideoAssets.qualified_name(namespace);
        let dataset = SpiralSql::video_dataset_relation(namespace);
        let view_start = Instant::now();
        connection
            .execute_update_sql(&SpiralSql::create_video_sources(&sources, &assets))
            .map_err(|error| format!("creating Spiral video source view: {error}"))?;
        profile_event("spiral.reconcile_video_view", view_start, (1, 0));
        if !dataset_exists {
            let create_start = Instant::now();
            connection
                .execute_update_sql(&SpiralSql::create_video_dataset(&dataset, &sources))
                .map_err(|error| format!("creating Spiral MP4 dataset: {error}"))?;
            profile_event("spiral.reconcile_video_create", create_start, (1, 0));
        }
        let refresh_start = Instant::now();
        connection
            .execute_update_sql(&SpiralSql::refresh_dataset(&dataset))
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

    fn configured_project_store(project: String, namespace: String) -> Arc<ProjectStore> {
        let settings = SpiralConnectionSettings::new(
            configured_spiral_endpoint(),
            configured_spiral_spfs_endpoint(),
        )
        .expect("the configured Spiral endpoints must be valid")
        .with_project(&project)
        .expect("the Spiral E2E project must be valid")
        .with_credentials(stored_credentials());
        Arc::new(
            ProjectStore::new(settings, namespace)
                .expect("the Spiral E2E project store must be valid"),
        )
    }

    fn relation_payloads(bundle: &[u8]) -> BTreeMap<String, Vec<u8>> {
        PROJECT_SPEC
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

    struct EpisodeSeed<'a> {
        namespace: &'a str,
        dataset_id: &'a str,
        episode_id: &'a str,
        started_ms: i64,
    }

    impl<'a> EpisodeSeed<'a> {
        fn new(namespace: &'a str, dataset_id: &'a str, episode_id: &'a str) -> Self {
            Self {
                namespace,
                dataset_id,
                episode_id,
                started_ms: 0,
            }
        }

        fn with_started_ms(mut self, started_ms: i64) -> Self {
            self.started_ms = started_ms;
            self
        }

        fn save(self, connection: &mut StorageConnection) -> Result<(), String> {
            let start = EpisodeStart::new(self.episode_id.to_owned(), self.dataset_id.to_owned())
                .with_timing(self.started_ms, 60.0)
                .with_source("transfer-e2e".to_owned(), None);
            let completion = EpisodeCompletion::new(0.0, Vec::new(), Vec::new());
            connection.ingest_batch(
                &Table::Datasets.qualified_name(self.namespace),
                start.dataset_batch().map_err(|error| error.to_string())?,
            )?;
            connection.ingest_batch(
                &Table::Episodes.qualified_name(self.namespace),
                completion
                    .episode_batch(&start, 0)
                    .map_err(|error| error.to_string())?,
            )
        }
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

    fn drop_studio_relations(
        connection: &mut StorageConnection,
        namespace: &str,
    ) -> Result<(), String> {
        connection.drop_video_dataset(namespace)?;
        for table in Table::ALL.into_iter().rev() {
            let relation = table.qualified_name(namespace);
            if connection.table_exists(&relation)? {
                connection.execute_update_sql(&format!("DROP TABLE {relation}"))?;
            }
        }
        Ok(())
    }

    struct TestNamespace {
        project_store: Arc<ProjectStore>,
    }

    impl TestNamespace {
        fn new(project_store: Arc<ProjectStore>) -> Self {
            Self { project_store }
        }
    }

    impl Drop for TestNamespace {
        fn drop(&mut self) {
            let namespace = self.project_store.namespace().to_owned();
            let _ = self.project_store.run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, &namespace)?;
                if connection.table_exists(&namespace)? {
                    connection.execute_update_sql(&format!("DROP TABLE {namespace}"))?;
                }
                Ok(())
            });
        }
    }
}
