//! Portable single-file snapshots of the Studio-owned Spiral namespace.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Cursor, Read as _};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::{
    Array as _, ArrayRef, Float32Array, LargeStringArray, ListArray, RecordBatch, StringArray,
    StringViewArray, UInt32Array, UInt64Array,
};
use arrow_ipc::reader::StreamReader;
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};
use spiral_robotics_contracts::api::{EpisodeImportSummary, ProjectImportSummary};
use spiral_robotics_contracts::spiral::{self, Table, column};
use spiral_robotics_contracts::workflow::{
    DEFAULT_WORKFLOW_ID, MAX_SIGLIP_CONCEPT_LENGTH, MAX_WORKFLOW_AGENT_CONVERSATION_TITLE_CHARS,
    MAX_WORKFLOW_AGENT_CONVERSATIONS, MAX_WORKFLOW_AGENT_MESSAGES_PER_CONVERSATION,
    MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH, MAX_WORKFLOW_FEATURE_STREAMS, SIGLIP_BASE_MODEL_ID,
    SIGLIP_BASE_REPOSITORY, SIGLIP_BASE_REVISION, WORKFLOW_FEATURE_REGISTRY_RELATION,
    WorkflowAgentArtifactKind, WorkflowAgentMessagePhase, WorkflowAgentPhase, WorkflowAgentRole,
    WorkflowAgentToolPhase, WorkflowFeatureApplicability, WorkflowFeatureDefinition,
    derived_feature_cache_relation, is_workflow_feature_name,
};
use spiral_robotics_core::query::is_identifier;
use spiral_robotics_core::workflow::validate_qualified_relation;

#[cfg(test)]
use crate::backend::storage::manager::run_binary_query;
use crate::backend::storage::manager::{
    commit_catalog_transaction, drop_video_dataset, ensure_dataset_namespace,
    ensure_video_assets_blob_relation, execute_update_sql, ingest_batches,
    normalize_video_asset_batch, rebuild_video_dataset, replace_batches,
    retarget_stored_video_media_relations, run_query, stored_video_episode_ids, table_exists,
    video_dataset_exists,
};
use crate::backend::storage::repository::RepositoryConnection;
use crate::backend::storage::sql;

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
const MAX_ARROW_MESSAGE_METADATA_BYTES: usize = MAX_MANIFEST_BYTES;
const MAX_ARROW_DECODED_BYTES: usize = MAX_PROJECT_BUNDLE_BYTES;
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

fn workflow_cache_key_declaration_may_be_nullable(name: &str) -> bool {
    matches!(name, "frame_index" | "episode_time_ns")
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
    workflow_id: String,
    #[serde(default, alias = "session_id")]
    conversation_id: String,
    revision: u64,
    phase: WorkflowAgentPhase,
    messages: Vec<BundleAgentMessage>,
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
    id: String,
    sequence: u64,
    role: WorkflowAgentRole,
    phase: WorkflowAgentMessagePhase,
    text: String,
    tools: Vec<BundleAgentToolSummary>,
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
#[serde(deny_unknown_fields)]
struct BundleAgentArtifactReceipt {
    kind: WorkflowAgentArtifactKind,
    label: String,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    row_count: Option<usize>,
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
pub(crate) const MAX_PROJECT_BUNDLE_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_EPISODE_BUNDLE_BYTES: usize = MAX_PROJECT_BUNDLE_BYTES;

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
    max_bytes: MAX_PROJECT_BUNDLE_BYTES,
};

const EPISODES_SPEC: BundleSpec = BundleSpec {
    magic: EPISODES_MAGIC,
    format: EPISODES_FORMAT,
    label: "episode bundle",
    tables: &EPISODE_TABLES,
    max_bytes: MAX_EPISODE_BUNDLE_BYTES,
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
            Self::Managed(table) => sql::relation(namespace, *table),
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
            .map(|entry| (entry.definition.name.clone(), entry.definition.clone()))
            .collect()
    }
}

fn current_workflow_registry(
    connection: &mut RepositoryConnection,
) -> Result<Option<WorkflowBundleRegistry>, String> {
    if !table_exists(connection, WORKFLOW_FEATURE_REGISTRY_RELATION)? {
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
        .map(|batch| batch.map_err(|error| format!("reading Workflow feature registry: {error}")))
        .collect::<Result<Vec<_>, _>>()?;
    if batches.is_empty() {
        batches.push(RecordBatch::new_empty(schema));
    }
    validate_workflow_registry(&batches).map(Some)
}

pub(crate) fn export(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<Vec<u8>, String> {
    let mut queries = Vec::new();
    for table in Table::ALL {
        let relation = sql::relation(namespace, table);
        if !table_exists(connection, &relation)? {
            continue;
        }
        let query = if table == Table::VideoAssets {
            portable_video_assets_query(&relation, None)
        } else {
            format!("SELECT * FROM {relation}")
        };
        queries.push((table.name().to_owned(), query));
    }
    if let Some(registry) = current_workflow_registry(connection)? {
        queries.push((
            WORKFLOW_FEATURE_REGISTRY_RELATION.to_owned(),
            format!("SELECT * FROM {WORKFLOW_FEATURE_REGISTRY_RELATION}"),
        ));
        for relation in registry.cache_relations() {
            if !table_exists(connection, &relation)? {
                return Err(format!(
                    "Workflow feature registry declares missing cache relation {relation:?}"
                ));
            }
            queries.push((relation.clone(), format!("SELECT * FROM {relation}")));
        }
    }
    export_named_queries(connection, queries, PROJECT_SPEC).map(|export| export.bytes)
}

pub(crate) fn export_episodes(
    connection: &mut RepositoryConnection,
    namespace: &str,
    dataset_id: &str,
) -> Result<Vec<u8>, String> {
    validate_dataset_id(dataset_id)?;
    let datasets = sql::relation(namespace, Table::Datasets);
    let episodes = sql::relation(namespace, Table::Episodes);
    if !table_exists(connection, &datasets)? || !table_exists(connection, &episodes)? {
        return Err(format!("dataset {dataset_id:?} has no episodes to export"));
    }
    let dataset = sql::string_literal(dataset_id);
    let episode_ids = format!(
        "SELECT {episode_id} FROM {episodes} WHERE {dataset_id_column} = {dataset}",
        episode_id = column::EPISODE_ID,
        dataset_id_column = column::DATASET_ID,
    );
    let mut queries = Vec::new();
    for table in EPISODE_TABLES {
        let relation = sql::relation(namespace, table);
        if !table_exists(connection, &relation)? {
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
            Table::VideoAssets => portable_video_assets_query(
                &relation,
                Some(&format!("{} IN ({episode_ids})", column::EPISODE_ID)),
            ),
            _ => unreachable!("episode bundle table set is closed"),
        };
        queries.push((table, query));
    }
    let export = export_queries(connection, queries, EPISODES_SPEC)?;
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

fn export_queries(
    connection: &mut RepositoryConnection,
    queries: Vec<(Table, String)>,
    spec: BundleSpec,
) -> Result<ExportedBundle, String> {
    let export = export_named_queries(
        connection,
        queries
            .into_iter()
            .map(|(table, query)| (table.name().to_owned(), query))
            .collect(),
        spec,
    )?;
    let row_counts = export
        .row_counts
        .into_iter()
        .map(|(name, rows)| {
            table_named_in(&name, spec.tables)
                .map(|table| (table, rows))
                .ok_or_else(|| format!("{} export contains unknown relation {name:?}", spec.label))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ExportedBundle {
        bytes: export.bytes,
        row_counts,
    })
}

fn export_named_queries(
    connection: &mut RepositoryConnection,
    queries: Vec<(String, String)>,
    spec: BundleSpec,
) -> Result<NamedExportedBundle, String> {
    let mut entries = Vec::<(String, Vec<u8>)>::new();
    let mut row_counts = Vec::new();
    for (relation, query) in queries {
        let mut statement = connection
            .new_statement()
            .map_err(|error| format!("opening {} export for {relation}: {error}", spec.label))?;
        statement
            .set_sql_query(query)
            .map_err(|error| format!("binding {} export for {relation}: {error}", spec.label))?;
        let reader = statement
            .execute()
            .map_err(|error| format!("reading {} relation {relation}: {error}", spec.label))?;
        let occupied = encoded_entries_size(&entries)
            .saturating_add(spec.magic.len() + 4 + MAX_MANIFEST_BYTES + 2);
        let payload_limit = spec
            .max_bytes
            .saturating_sub(occupied)
            .saturating_sub(2 + relation.len() + 8);
        let mut payload = LimitedPayload::new(payload_limit, spec.label);
        let mut rows = 0_u64;
        {
            let mut writer = StreamWriter::try_new(&mut payload, reader.schema().as_ref())
                .map_err(|error| format!("opening Arrow stream for {relation}: {error}"))?;
            for batch in reader {
                let batch = batch.map_err(|error| {
                    format!("reading {} relation {relation}: {error}", spec.label)
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
        let projected_size = encoded_entries_size(&entries)
            .saturating_add(2 + relation.len() + 8 + payload.len())
            .saturating_add(spec.magic.len() + 4 + MAX_MANIFEST_BYTES);
        if projected_size > spec.max_bytes {
            return Err(format!(
                "{} export exceeds the {} MiB portable-file limit",
                spec.label,
                spec.max_bytes / (1024 * 1024),
            ));
        }
        row_counts.push((relation.clone(), rows));
        entries.push((relation, payload));
    }
    build_bundle(entries, spec).map(|bytes| NamedExportedBundle { bytes, row_counts })
}

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

fn build_bundle(entries: Vec<(String, Vec<u8>)>, spec: BundleSpec) -> Result<Vec<u8>, String> {
    if let Some((name, _)) = entries
        .iter()
        .find(|(name, _)| name.len() > MAX_ENTRY_NAME_BYTES)
    {
        return Err(format!("{} relation name {name:?} is too long", spec.label));
    }
    let manifest = Manifest {
        format: spec.format.to_owned(),
        format_version: FORMAT_VERSION,
        schema_version: spiral::SCHEMA_VERSION,
        exported_ms: unix_millis(),
        relations: entries.iter().map(|(name, _)| name.clone()).collect(),
    };
    let manifest = serde_json::to_vec(&manifest)
        .map_err(|error| format!("encoding {} manifest: {error}", spec.label))?;
    let mut bundle = Vec::new();
    bundle.extend_from_slice(spec.magic);
    write_u32(&mut bundle, manifest.len())?;
    bundle.extend_from_slice(&manifest);
    for (name, payload) in entries {
        write_u16(&mut bundle, name.len())?;
        bundle.extend_from_slice(name.as_bytes());
        write_u64(&mut bundle, payload.len())?;
        bundle.extend_from_slice(&payload);
    }
    bundle.extend_from_slice(&0_u16.to_le_bytes());
    if bundle.len() > spec.max_bytes {
        return Err(format!(
            "{} export exceeds the {} MiB portable-file limit",
            spec.label,
            spec.max_bytes / (1024 * 1024),
        ));
    }
    Ok(bundle)
}

pub(crate) fn import(
    connection: &mut RepositoryConnection,
    namespace: &str,
    bundle: &[u8],
) -> Result<ProjectImportSummary, String> {
    let decoded = decode_project_entries(parse_raw_bundle(bundle, PROJECT_SPEC)?)?;
    let fixed_relations = decoded
        .iter()
        .filter_map(|entry| match &entry.relation {
            ProjectRelation::Managed(table) => Some((*table, entry.batches.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let imported_video_episodes = video_episode_ids(&fixed_relations)?;
    let current_caches = current_workflow_registry(connection)?
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
    let workflow_plan = workflow_restore_plan(
        &current_caches,
        incoming_registry.then_some(&incoming_caches),
    );

    let (mut summary, transaction_warning) =
        run_project_import_transaction(connection, "project import", |connection| {
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
                let relation = sql::relation(namespace, table);
                if table_exists(connection, &relation)? {
                    execute_update_sql(connection, &format!("DROP TABLE {relation}")).map_err(
                        |error| format!("clearing project relation {relation}: {error}"),
                    )?;
                }
            }
            for relation in &workflow_plan.retire_caches {
                if table_exists(connection, relation)? {
                    execute_update_sql(connection, &format!("DROP TABLE {relation}"))
                        .map_err(|error| format!("clearing Workflow cache {relation}: {error}"))?;
                }
            }
            if workflow_plan.clear_registry
                && table_exists(connection, WORKFLOW_FEATURE_REGISTRY_RELATION)?
            {
                execute_update_sql(
                    connection,
                    &format!("DROP TABLE {WORKFLOW_FEATURE_REGISTRY_RELATION}"),
                )
                .map_err(|error| format!("clearing Workflow feature registry: {error}"))?;
            }

            let mut summary = ProjectImportSummary::default();
            for entry in decoded {
                let relation = entry.relation.target(namespace);
                summary.relations += 1;
                let batches = entry
                    .batches
                    .into_iter()
                    .map(|batch| {
                        if entry.relation == ProjectRelation::Managed(Table::VideoAssets) {
                            prepare_video_asset_import(batch, namespace)
                        } else {
                            Ok(batch)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                summary.rows =
                    summary.rows.saturating_add(
                        batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64,
                    );
                replace_batches(connection, &relation, batches)?;
            }
            Ok(summary)
        })?;
    let can_reconcile = transaction_warning.is_none();
    if let Some(warning) = transaction_warning {
        summary.warnings.push(warning);
    }
    // ADBC ingest can create a relation inside the import transaction, but
    // Spiral's SQL binder cannot resolve that new relation until the commit is
    // published. Keep the repository's exclusive checkout while reconciling
    // the derived MP4 resources immediately after the durable rows commit.
    if can_reconcile {
        if let Err(error) =
            reconcile_project_video_dataset(connection, namespace, &imported_video_episodes)
        {
            summary.warnings.push(format!(
                "project rows committed, but the derived video dataset needs repair: {error}"
            ));
        }
    } else {
        summary.warnings.push(
            "project rows committed, but derived video reconciliation was skipped because the transaction connection could not be reused"
                .to_owned(),
        );
    }
    Ok(summary)
}

pub(crate) fn import_episodes(
    connection: &mut RepositoryConnection,
    namespace: &str,
    dataset_id: &str,
    bundle: &[u8],
) -> Result<EpisodeImportSummary, String> {
    validate_dataset_id(dataset_id)?;
    let mut decoded = decode_entries(parse_bundle(bundle, EPISODES_SPEC)?, EPISODES_SPEC)?;
    sort_relations(&mut decoded, &EPISODE_TABLES);
    let episode_ids = validate_episode_relations(&decoded)?;
    let video_episode_ids = video_episode_ids(&decoded)?;
    let existing_video_episodes = stored_video_episode_ids(connection, namespace)?;
    reject_existing_episodes(connection, namespace, &episode_ids)?;
    ensure_dataset_namespace(connection, namespace)?;
    if !video_episode_ids.is_empty() {
        ensure_video_assets_blob_relation(connection, namespace)?;
    }
    if !existing_video_episodes.is_empty() && !video_dataset_exists(connection, namespace)? {
        retarget_stored_video_media_relations(connection, namespace)?;
    }

    connection
        .set_autocommit(false)
        .map_err(|error| format!("starting episode import transaction: {error}"))?;
    let outcome = (|| {
        let datasets = sql::relation(namespace, Table::Datasets);
        let dataset_exists = table_exists(connection, &datasets)?
            && !run_query(connection, &sql::dataset_exists(&datasets, dataset_id), 1)?
                .rows
                .is_empty();
        let mut summary = EpisodeImportSummary {
            dataset_id: dataset_id.to_owned(),
            episodes: episode_ids.len() as u64,
            rows: 0,
            first_episode: episode_ids.first().cloned(),
            warnings: Vec::new(),
        };
        for (table, batches) in decoded {
            if table == Table::Datasets && dataset_exists {
                continue;
            }
            let relation = sql::relation(namespace, table);
            let batches = batches
                .into_iter()
                .map(|batch| match table {
                    Table::Datasets | Table::Episodes => {
                        replace_utf8_column(batch, column::DATASET_ID, dataset_id)
                    }
                    Table::VideoAssets => prepare_video_asset_import(batch, namespace),
                    _ => Ok(batch),
                })
                .collect::<Result<Vec<_>, _>>()?;
            summary.rows = summary
                .rows
                .saturating_add(batches.iter().map(RecordBatch::num_rows).sum::<usize>() as u64);
            ingest_batches(connection, &relation, batches)?;
        }
        Ok(summary)
    })();
    let (mut summary, transaction_warning) =
        finish_transaction(connection, outcome, "episode import")?;
    let can_reconcile = transaction_warning.is_none();
    if let Some(warning) = transaction_warning {
        summary.warnings.push(warning);
    }
    if can_reconcile {
        if !video_episode_ids.is_empty()
            && let Err(error) = reconcile_appended_video_dataset(connection, namespace)
        {
            summary.warnings.push(format!(
                "episode rows committed, but the derived video dataset needs repair: {error}"
            ));
        }
    } else if !video_episode_ids.is_empty() {
        summary.warnings.push(
            "episode rows committed, but derived video reconciliation was skipped because the transaction connection could not be reused"
                .to_owned(),
        );
    }
    Ok(summary)
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

#[derive(Default)]
struct ArrowDecodeBudget {
    claimed_decoded_bytes: usize,
    decoded_memory_bytes: usize,
    rows: usize,
}

fn decode_arrow_payload(
    relation: &str,
    payload: Vec<u8>,
    label: &str,
    budget: &mut ArrowDecodeBudget,
) -> Result<Vec<RecordBatch>, String> {
    decode_arrow_payload_with_limits(relation, payload, label, budget, ARROW_DECODE_LIMITS)
}

fn decode_arrow_payload_with_row_limit(
    relation: &str,
    payload: Vec<u8>,
    label: &str,
    budget: &mut ArrowDecodeBudget,
    relation_rows: usize,
) -> Result<Vec<RecordBatch>, String> {
    decode_arrow_payload_with_limits(
        relation,
        payload,
        label,
        budget,
        ArrowDecodeLimits {
            relation_rows,
            ..ARROW_DECODE_LIMITS
        },
    )
}

fn decode_arrow_payload_with_limits(
    relation: &str,
    payload: Vec<u8>,
    label: &str,
    budget: &mut ArrowDecodeBudget,
    limits: ArrowDecodeLimits,
) -> Result<Vec<RecordBatch>, String> {
    preflight_arrow_payload(relation, &payload, label, budget, limits)?;
    let reader = StreamReader::try_new(Cursor::new(payload), None)
        .map_err(|error| format!("opening Arrow stream for {relation}: {error}"))?;
    let schema = reader.schema();
    let mut batches = Vec::new();
    for batch in reader {
        let batch =
            batch.map_err(|error| format!("reading Arrow stream for {relation}: {error}"))?;
        let batch_memory = batch
            .get_array_memory_size()
            .checked_add(std::mem::size_of::<RecordBatch>().saturating_mul(2))
            .ok_or_else(|| {
                format!("{label} Arrow memory size overflows while reading relation {relation}")
            })?;
        budget.decoded_memory_bytes = budget
            .decoded_memory_bytes
            .checked_add(batch_memory)
            .filter(|bytes| *bytes <= limits.decoded_bytes)
            .ok_or_else(|| {
                format!(
                    "{label} Arrow data exceeds the {} MiB decoded-memory limit while reading relation {relation}",
                    limits.decoded_bytes / (1024 * 1024),
                )
            })?;
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

fn preflight_arrow_payload(
    relation: &str,
    payload: &[u8],
    label: &str,
    budget: &mut ArrowDecodeBudget,
    limits: ArrowDecodeLimits,
) -> Result<(), String> {
    let mut position = 0_usize;
    let mut claimed_decoded_bytes = 0_usize;
    let mut rows = 0_usize;
    while position < payload.len() {
        let first = read_arrow_u32(payload, &mut position, relation)?;
        let metadata_len = if first == u32::MAX {
            read_arrow_u32(payload, &mut position, relation)?
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
        claimed_decoded_bytes = claimed_decoded_bytes
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
                validate_arrow_schema_bound(relation, schema, label, limits.schema_fields)?;
            }
            arrow_ipc::MessageHeader::RecordBatch => {
                let batch = message.header_as_record_batch().ok_or_else(|| {
                    format!("{label} Arrow record metadata for {relation} is invalid")
                })?;
                preflight_arrow_record_batch(
                    relation,
                    batch,
                    body,
                    label,
                    limits,
                    &mut claimed_decoded_bytes,
                    &mut rows,
                )?;
            }
            arrow_ipc::MessageHeader::DictionaryBatch => {
                let dictionary = message.header_as_dictionary_batch().ok_or_else(|| {
                    format!("{label} Arrow dictionary metadata for {relation} is invalid")
                })?;
                let batch = dictionary.data().ok_or_else(|| {
                    format!("{label} Arrow dictionary for {relation} has no record metadata")
                })?;
                preflight_arrow_record_batch(
                    relation,
                    batch,
                    body,
                    label,
                    limits,
                    &mut claimed_decoded_bytes,
                    &mut rows,
                )?;
            }
            _ => {}
        }
        position = body_end;
    }

    if rows > limits.relation_rows {
        return Err(format!(
            "{label} Arrow relation {relation} exceeds the {}-row relation limit",
            limits.relation_rows,
        ));
    }
    budget.claimed_decoded_bytes = budget
        .claimed_decoded_bytes
        .checked_add(claimed_decoded_bytes)
        .filter(|bytes| *bytes <= limits.decoded_bytes)
        .ok_or_else(|| {
            format!(
                "{label} Arrow data exceeds the {} MiB decoded-buffer limit at relation {relation}",
                limits.decoded_bytes / (1024 * 1024),
            )
        })?;
    budget.rows = budget
        .rows
        .checked_add(rows)
        .filter(|rows| *rows <= limits.decoded_rows)
        .ok_or_else(|| {
            format!(
                "{label} Arrow data exceeds the {}-row decoded limit at relation {relation}",
                limits.decoded_rows,
            )
        })?;
    Ok(())
}

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

fn validate_arrow_schema_bound(
    relation: &str,
    schema: arrow_ipc::Schema<'_>,
    label: &str,
    max_fields: usize,
) -> Result<(), String> {
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
                format!("{label} Arrow schema for {relation} exceeds the {max_fields}-field limit")
            })?;
        if let Some(children) = field.children() {
            pending.extend(children.iter());
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn preflight_arrow_record_batch(
    relation: &str,
    batch: arrow_ipc::RecordBatch<'_>,
    body: &[u8],
    label: &str,
    limits: ArrowDecodeLimits,
    claimed_decoded_bytes: &mut usize,
    rows: &mut usize,
) -> Result<(), String> {
    let batch_rows = usize::try_from(batch.length()).map_err(|_| {
        format!("{label} Arrow record batch for {relation} has an invalid row count")
    })?;
    if batch_rows > limits.record_batch_rows {
        return Err(format!(
            "{label} Arrow record batch for {relation} exceeds the {}-row batch limit",
            limits.record_batch_rows,
        ));
    }
    *rows = rows.checked_add(batch_rows).ok_or_else(|| {
        format!("{label} Arrow row count overflows while reading relation {relation}")
    })?;

    let compressed = batch.compression().is_some();
    for buffer in batch.buffers().into_iter().flatten() {
        let offset = usize::try_from(buffer.offset())
            .map_err(|_| format!("{label} Arrow buffer for {relation} has an invalid offset"))?;
        let length = usize::try_from(buffer.length())
            .map_err(|_| format!("{label} Arrow buffer for {relation} has an invalid length"))?;
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
        *claimed_decoded_bytes = claimed_decoded_bytes
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

fn decode_entries(
    entries: Vec<(Table, Vec<u8>)>,
    spec: BundleSpec,
) -> Result<Vec<(Table, Vec<RecordBatch>)>, String> {
    let mut budget = ArrowDecodeBudget::default();
    decode_entries_with_budget(entries, spec, &mut budget)
}

fn decode_entries_with_budget(
    entries: Vec<(Table, Vec<u8>)>,
    spec: BundleSpec,
    budget: &mut ArrowDecodeBudget,
) -> Result<Vec<(Table, Vec<RecordBatch>)>, String> {
    let mut decoded = Vec::new();
    for (table, payload) in entries {
        let batches = decode_arrow_payload(table.name(), payload, spec.label, budget)?;
        let schema = batches[0].schema();
        validate_managed_relation_schema(table, schema.as_ref(), spec.label)?;
        decoded.push((table, batches));
    }
    Ok(decoded)
}

fn validate_managed_relation_schema(
    table: Table,
    schema: &Schema,
    label: &str,
) -> Result<(), String> {
    let expected = managed_relation_schema(table);
    if !schema.metadata().is_empty() {
        return Err(format!(
            "{label} relation {table} has unsupported schema metadata"
        ));
    }
    if schema.fields().len() != expected.fields().len() {
        return Err(format!(
            "{label} relation {table} has {} columns; expected {}",
            schema.fields().len(),
            expected.fields().len(),
        ));
    }
    for (actual, expected) in schema.fields().iter().zip(expected.fields()) {
        if !managed_field_matches(actual, expected) {
            return Err(format!(
                "{label} relation {table} field {:?} has type {} (nullable {}); expected semantic type {} (nullable {})",
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

fn managed_field_matches(actual: &Field, expected: &Field) -> bool {
    actual.name() == expected.name()
        && actual.is_nullable() == expected.is_nullable()
        && actual.metadata() == expected.metadata()
        && managed_data_type_matches(actual.data_type(), expected.data_type())
}

fn managed_data_type_matches(actual: &DataType, expected: &DataType) -> bool {
    match expected {
        DataType::Utf8 => matches!(
            actual,
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
        ),
        DataType::Binary | DataType::LargeBinary => matches!(
            actual,
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView
        ),
        DataType::List(expected_child) => {
            let DataType::List(actual_child) = actual else {
                return false;
            };
            managed_field_matches(actual_child, expected_child)
        }
        _ => actual == expected,
    }
}

fn managed_relation_schema(table: Table) -> Schema {
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

fn decode_project_entries(
    entries: Vec<(String, Vec<u8>)>,
) -> Result<Vec<DecodedProjectRelation>, String> {
    let mut budget = ArrowDecodeBudget::default();
    let mut payloads = entries.into_iter().collect::<BTreeMap<_, _>>();
    let registry_batches = payloads
        .remove(WORKFLOW_FEATURE_REGISTRY_RELATION)
        .map(|payload| {
            decode_arrow_payload_with_row_limit(
                WORKFLOW_FEATURE_REGISTRY_RELATION,
                payload,
                PROJECT_SPEC.label,
                &mut budget,
                MAX_FEATURE_REGISTRY_ROWS,
            )
        })
        .transpose()?;
    let registry = registry_batches
        .as_deref()
        .map(validate_workflow_registry)
        .transpose()?
        .unwrap_or_default();
    let declared_caches = registry.cache_relations();
    let dynamic_names = payloads
        .keys()
        .filter(|name| table_named_in(name, &Table::ALL).is_none())
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
                decode_entries_with_budget(vec![(table, payload)], PROJECT_SPEC, &mut budget)?;
            if table == Table::WorkflowRuns {
                validate_workflow_agent_bundle_rows(&relation[0].1)?;
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
        let batches = decode_arrow_payload_with_row_limit(
            &relation,
            payload,
            PROJECT_SPEC.label,
            &mut budget,
            MAX_FEATURE_CACHE_ROWS,
        )?;
        validate_workflow_cache(&relation, &batches, &registry)?;
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

fn validate_workflow_agent_bundle_rows(batches: &[RecordBatch]) -> Result<(), String> {
    let mut keys = BTreeSet::new();
    for batch in batches {
        for row in 0..batch.num_rows() {
            let run_id = batch
                .column_by_name("run_id")
                .and_then(|column| workflow_string_value(column.as_ref(), row))
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
                .and_then(|column| workflow_string_value(column.as_ref(), row))
                .ok_or_else(|| {
                    "project Assistant history has an invalid snapshot_json".to_owned()
                })?;
            validate_workflow_agent_json(snapshot)?;
        }
    }
    Ok(())
}

fn validate_workflow_agent_json(encoded: &str) -> Result<(), String> {
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
            format!("project Assistant history does not match the current document shape: {error}")
        })?;
    if canonical.conversations.iter().any(|conversation| {
        !bundle_provider_history_is_structurally_valid(
            &conversation.provider_messages,
            conversation.session.phase.active(),
        )
    }) {
        return Err(
            "project Assistant provider history has an invalid role or tool exchange".to_owned(),
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
        let id = bounded_json_string(conversation.get("conversation_id"), 128, "conversation id")?;
        if !ids.insert(id) {
            return Err(
                "project Assistant history has duplicate conversation identities".to_owned(),
            );
        }
        let title = bounded_json_string(
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
            .ok_or_else(|| "project Assistant conversation is missing its session".to_owned())?;
        if session
            .get("workflow_id")
            .and_then(serde_json::Value::as_str)
            != Some(DEFAULT_WORKFLOW_ID)
        {
            return Err("project Assistant session belongs to an unsupported Workflow".to_owned());
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
        validate_agent_messages(session.get("messages"))?;
        validate_provider_messages(conversation.get("provider_messages"))?;
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
                "project Assistant history has invalid or duplicate message identities".to_owned(),
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

fn bundle_provider_history_is_structurally_valid(
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
            if expected.len() != message.tool_calls.len() || expected.iter().any(|id| id.is_empty())
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

fn validate_workflow_registry(batches: &[RecordBatch]) -> Result<WorkflowBundleRegistry, String> {
    validate_workflow_registry_with_limits(
        batches,
        MAX_FEATURE_REGISTRY_ROWS,
        MAX_FEATURE_CACHE_COLUMNS,
    )
}

fn validate_workflow_registry_with_limits(
    batches: &[RecordBatch],
    max_rows: usize,
    max_features_per_cache: usize,
) -> Result<WorkflowBundleRegistry, String> {
    let mut registry = WorkflowBundleRegistry::default();
    let mut features_per_cache = BTreeMap::<String, usize>::new();
    let mut cache_sources = BTreeMap::<String, String>::new();
    for batch in batches {
        validate_workflow_registry_schema(batch.schema().as_ref())?;
        for row in 0..batch.num_rows() {
            if registry.entries.len() >= max_rows {
                return Err(format!(
                    "Workflow feature registry exceeds the bounded limit of {max_rows} definitions"
                ));
            }
            let source_relation = required_workflow_text(batch, "source_relation", row)?;
            let cache_relation = required_workflow_text(batch, "cache_relation", row)?;
            let feature_name = required_workflow_text(batch, "feature_name", row)?;
            let definition = WorkflowFeatureDefinition {
                name: feature_name.clone(),
                description: required_workflow_text(batch, "description", row)?,
                concept: required_workflow_text(batch, "concept", row)?,
                applicability: WorkflowFeatureApplicability {
                    source_relation: source_relation.clone(),
                    media_field: required_workflow_text(batch, "media_field", row)?,
                    stream_ids: workflow_registry_stream_ids(batch, row)?,
                },
            };
            validate_qualified_relation(&source_relation).map_err(|error| {
                format!("Workflow feature registry has unsafe source relation: {error}")
            })?;
            validate_qualified_relation(&cache_relation).map_err(|error| {
                format!("Workflow feature registry has unsafe cache relation: {error}")
            })?;
            let expected = derived_feature_cache_relation(&source_relation);
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
            validate_workflow_definition(&definition)?;
            let producer_model = required_workflow_text(batch, "producer_model_id", row)?;
            let producer_repository = required_workflow_text(batch, "producer_repository", row)?;
            let artifact_revision = required_workflow_text(batch, "artifact_revision", row)?;
            let value_semantics = required_workflow_text(batch, "value_semantics", row)?;
            let definition_version = required_workflow_u32(batch, "definition_version", row)?;
            let created_at_ms = required_workflow_u64(batch, "created_at_ms", row)?;
            let updated_at_ms = required_workflow_u64(batch, "updated_at_ms", row)?;
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

fn validate_workflow_registry_schema(schema: &Schema) -> Result<(), String> {
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
                    || !workflow_storage_type_matches(actual.data_type(), &data_type)
            })
    {
        return Err(
            "Workflow feature registry has an incompatible canonical 14-field schema".to_owned(),
        );
    }
    Ok(())
}

fn validate_workflow_definition(definition: &WorkflowFeatureDefinition) -> Result<(), String> {
    let streams = definition
        .applicability
        .stream_ids
        .iter()
        .collect::<BTreeSet<_>>();
    if !is_workflow_feature_name(&definition.name)
        || definition.description.trim().is_empty()
        || definition.description.chars().count() > MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH
        || definition.concept.trim().is_empty()
        || definition.concept.chars().count() > MAX_SIGLIP_CONCEPT_LENGTH
        || !is_identifier(&definition.applicability.media_field)
        || definition.applicability.stream_ids.len() > MAX_WORKFLOW_FEATURE_STREAMS
        || streams.len() != definition.applicability.stream_ids.len()
        || streams
            .iter()
            .any(|stream| stream.trim().is_empty() || stream.len() > 128)
    {
        return Err("Workflow feature registry contains an invalid feature definition".to_owned());
    }
    Ok(())
}

fn workflow_registry_stream_ids(batch: &RecordBatch, row: usize) -> Result<Vec<String>, String> {
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
            workflow_string_value(values.as_ref(), index)
                .map(str::to_owned)
                .ok_or_else(|| {
                    "Workflow feature registry contains a null or invalid stream ID".to_owned()
                })
        })
        .collect()
}

fn required_workflow_text(batch: &RecordBatch, name: &str, row: usize) -> Result<String, String> {
    batch
        .column_by_name(name)
        .and_then(|column| workflow_string_value(column.as_ref(), row))
        .map(str::to_owned)
        .ok_or_else(|| format!("Workflow feature registry field {name:?} is null or invalid"))
}

fn required_workflow_u32(batch: &RecordBatch, name: &str, row: usize) -> Result<u32, String> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
        .filter(|values| !values.is_null(row))
        .map(|values| values.value(row))
        .ok_or_else(|| format!("Workflow feature registry field {name:?} is null or invalid"))
}

fn required_workflow_u64(batch: &RecordBatch, name: &str, row: usize) -> Result<u64, String> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
        .filter(|values| !values.is_null(row))
        .map(|values| values.value(row))
        .ok_or_else(|| format!("Workflow feature registry field {name:?} is null or invalid"))
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WorkflowCacheKey {
    episode_id: String,
    stream_id: String,
    segment_index: u32,
    frame_index: u64,
    episode_time_ns: u64,
}

fn validate_workflow_cache(
    relation: &str,
    batches: &[RecordBatch],
    registry: &WorkflowBundleRegistry,
) -> Result<(), String> {
    validate_workflow_cache_with_limits(
        relation,
        batches,
        registry,
        MAX_FEATURE_CACHE_ROWS,
        MAX_FEATURE_CACHE_COLUMNS,
    )
}

fn validate_workflow_cache_with_limits(
    relation: &str,
    batches: &[RecordBatch],
    registry: &WorkflowBundleRegistry,
    max_rows: usize,
    max_features: usize,
) -> Result<(), String> {
    let registered = registry.features_for_cache(relation);
    let schema = batches
        .first()
        .map(RecordBatch::schema)
        .ok_or_else(|| format!("Workflow feature cache {relation:?} has no Arrow schema"))?;
    validate_workflow_cache_schema(relation, schema.as_ref(), &registered, max_features)?;
    let mut keys = BTreeSet::new();
    for batch in batches {
        if batch.schema() != schema {
            return Err(format!(
                "Workflow feature cache {relation:?} changes schema between batches"
            ));
        }
        for row in 0..batch.num_rows() {
            if keys.len() >= max_rows {
                return Err(format!(
                    "Workflow feature cache exceeds the bounded limit of {max_rows} rows"
                ));
            }
            let key = workflow_cache_key(batch, row)?;
            if !keys.insert(key) {
                return Err(
                    "Workflow feature cache contains duplicate stable frame identities".to_owned(),
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

fn validate_workflow_cache_schema(
    relation: &str,
    schema: &Schema,
    registered: &BTreeMap<String, WorkflowFeatureDefinition>,
    max_features: usize,
) -> Result<(), String> {
    if schema.fields().len() < CACHE_KEY_FIELDS.len() {
        return Err(format!(
            "Workflow feature cache {relation:?} is missing stable identity fields"
        ));
    }
    for (index, (name, expected)) in CACHE_KEY_FIELDS.iter().enumerate() {
        let field = schema.field(index);
        if field.name() != name
            || (field.is_nullable() && !workflow_cache_key_declaration_may_be_nullable(name))
            || !workflow_storage_type_matches(field.data_type(), expected)
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
    if feature_fields.len() > max_features {
        return Err(format!(
            "Workflow feature cache exceeds {max_features} feature columns"
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

fn workflow_cache_key(batch: &RecordBatch, row: usize) -> Result<WorkflowCacheKey, String> {
    let text = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|column| workflow_string_value(column.as_ref(), row))
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

fn workflow_string_value(array: &dyn arrow_array::Array, row: usize) -> Option<&str> {
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

fn workflow_storage_type_matches(actual: &DataType, expected: &DataType) -> bool {
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
                    && workflow_storage_type_matches(actual.data_type(), expected.data_type())
            }
            _ => false,
        }
}

fn workflow_restore_plan(
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

trait ProjectImportTransaction {
    fn begin_project_import(&mut self) -> Result<(), String>;
    fn commit_project_import(&mut self) -> Result<(), String>;
    fn rollback_project_import(&mut self) -> Result<(), String>;
    fn restore_project_autocommit(&mut self) -> Result<(), String>;
    fn discard_project_connection(&mut self);
}

impl ProjectImportTransaction for RepositoryConnection {
    fn begin_project_import(&mut self) -> Result<(), String> {
        self.set_autocommit(false)
    }

    fn commit_project_import(&mut self) -> Result<(), String> {
        commit_catalog_transaction(self)
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

fn run_project_import_transaction<C, T>(
    connection: &mut C,
    operation: &str,
    mutation: impl FnOnce(&mut C) -> Result<T, String>,
) -> Result<(T, Option<String>), String>
where
    C: ProjectImportTransaction,
{
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

fn finish_transaction<T>(
    connection: &mut RepositoryConnection,
    outcome: Result<T, String>,
    operation: &str,
) -> Result<(T, Option<String>), String> {
    let final_result = match outcome {
        Ok(value) => commit_catalog_transaction(connection)
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

fn sort_relations(decoded: &mut [(Table, Vec<RecordBatch>)], order: &[Table]) {
    decoded.sort_by_key(|(table, _)| {
        order
            .iter()
            .position(|candidate| candidate == table)
            .unwrap_or(usize::MAX)
    });
}

fn validate_episode_relations(
    decoded: &[(Table, Vec<RecordBatch>)],
) -> Result<Vec<String>, String> {
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
            source_datasets.extend(utf8_values(batch, column::DATASET_ID)?);
        }
    }
    let source_dataset = source_datasets
        .pop()
        .ok_or_else(|| "episode bundle source dataset has no dataset ID".to_owned())?;
    validate_dataset_id(&source_dataset)?;
    let mut episode_ids = BTreeSet::new();
    for (table, batches) in decoded {
        if *table == Table::Episodes {
            for batch in batches {
                let ids = utf8_values(batch, column::EPISODE_ID)?;
                let datasets = utf8_values(batch, column::DATASET_ID)?;
                if datasets.iter().any(|dataset| dataset != &source_dataset) {
                    return Err(format!(
                        "episode bundle contains an episode outside source dataset {source_dataset:?}"
                    ));
                }
                for episode_id in ids {
                    validate_episode_id(&episode_id)?;
                    if !episode_ids.insert(episode_id.clone()) {
                        return Err(format!("episode bundle repeats episode ID {episode_id:?}"));
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
                for episode_id in utf8_values(batch, column::EPISODE_ID)? {
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

fn video_episode_ids(decoded: &[(Table, Vec<RecordBatch>)]) -> Result<BTreeSet<String>, String> {
    let mut episode_ids = BTreeSet::new();
    for (_, batches) in decoded
        .iter()
        .filter(|(table, _)| *table == Table::VideoAssets)
    {
        for batch in batches {
            for episode_id in utf8_values(batch, column::EPISODE_ID)? {
                validate_episode_id(&episode_id)?;
                episode_ids.insert(episode_id);
            }
        }
    }
    Ok(episode_ids)
}

fn reconcile_project_video_dataset(
    connection: &mut RepositoryConnection,
    namespace: &str,
    imported: &BTreeSet<String>,
) -> Result<(), String> {
    let dataset_exists = video_dataset_exists(connection, namespace)?;
    if imported.is_empty() {
        drop_video_dataset(connection, namespace)?;
    } else {
        rebuild_video_dataset(connection, namespace, dataset_exists)?;
    }
    Ok(())
}

fn reconcile_appended_video_dataset(
    connection: &mut RepositoryConnection,
    namespace: &str,
) -> Result<(), String> {
    let dataset_exists = video_dataset_exists(connection, namespace)?;
    rebuild_video_dataset(connection, namespace, dataset_exists)?;
    Ok(())
}

fn utf8_values(batch: &RecordBatch, name: &str) -> Result<Vec<String>, String> {
    let index = batch
        .schema()
        .index_of(name)
        .map_err(|error| format!("episode bundle is missing column {name}: {error}"))?;
    let values = batch.column(index);
    if let Some(values) = values.as_any().downcast_ref::<StringArray>() {
        return collect_utf8(values.iter(), name);
    }
    if let Some(values) = values.as_any().downcast_ref::<LargeStringArray>() {
        return collect_utf8(values.iter(), name);
    }
    if let Some(values) = values.as_any().downcast_ref::<StringViewArray>() {
        return collect_utf8(values.iter(), name);
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

fn replace_utf8_column(batch: RecordBatch, name: &str, value: &str) -> Result<RecordBatch, String> {
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
    let media_relation = sql::video_dataset_output(namespace);
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

fn prepare_video_asset_import(batch: RecordBatch, namespace: &str) -> Result<RecordBatch, String> {
    retarget_video_media_relations(normalize_video_asset_batch(batch)?, namespace)
}

fn reject_existing_episodes(
    connection: &mut RepositoryConnection,
    namespace: &str,
    episode_ids: &[String],
) -> Result<(), String> {
    let episodes = sql::relation(namespace, Table::Episodes);
    if !table_exists(connection, &episodes)? {
        return Ok(());
    }
    for chunk in episode_ids.chunks(100) {
        let candidates = chunk
            .iter()
            .map(|episode_id| sql::string_literal(episode_id))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT {episode_id} FROM {episodes} WHERE {episode_id} IN ({candidates})",
            episode_id = column::EPISODE_ID,
        );
        let existing = run_query(connection, &query, chunk.len())?;
        if let Some(value) = existing
            .rows
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
    if !spiral::is_dataset_id(dataset_id) {
        return Err("dataset ID must contain 1–96 letters, numbers, '.', '_' or '-'".to_owned());
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

#[cfg(test)]
fn parse(bundle: &[u8]) -> Result<Vec<(Table, Vec<u8>)>, String> {
    parse_bundle(bundle, PROJECT_SPEC)
}

fn parse_bundle(bundle: &[u8], spec: BundleSpec) -> Result<Vec<(Table, Vec<u8>)>, String> {
    parse_raw_bundle(bundle, spec)?
        .into_iter()
        .map(|(name, payload)| {
            table_named_in(&name, spec.tables)
                .map(|table| (table, payload))
                .ok_or_else(|| format!("{} contains unknown relation {name:?}", spec.label))
        })
        .collect()
}

fn parse_raw_bundle(bundle: &[u8], spec: BundleSpec) -> Result<Vec<(String, Vec<u8>)>, String> {
    if bundle.len() > spec.max_bytes {
        return Err(format!(
            "{} file exceeds the {} MiB import limit",
            spec.label,
            spec.max_bytes / (1024 * 1024),
        ));
    }
    let mut source = Cursor::new(bundle);
    let mut magic = [0_u8; 8];
    source
        .read_exact(&mut magic)
        .map_err(|_| format!("{} file is truncated", spec.label))?;
    if &magic != spec.magic {
        return Err(format!("file is not a Spiral Robotics {}", spec.label));
    }
    let manifest_len = read_u32(&mut source)? as usize;
    if manifest_len == 0 || manifest_len > MAX_MANIFEST_BYTES {
        return Err(format!("{} manifest has an invalid size", spec.label));
    }
    let mut manifest = vec![0_u8; manifest_len];
    source
        .read_exact(&mut manifest)
        .map_err(|_| format!("{} manifest is truncated", spec.label))?;
    let manifest: Manifest = serde_json::from_slice(&manifest)
        .map_err(|error| format!("{} manifest is invalid: {error}", spec.label))?;
    if manifest.format != spec.format || manifest.format_version != FORMAT_VERSION {
        return Err(format!(
            "{} file uses an unsupported format version",
            spec.label
        ));
    }
    if manifest.schema_version != spiral::SCHEMA_VERSION {
        return Err(format!(
            "{} schema {} cannot be imported by schema {}",
            spec.label,
            manifest.schema_version,
            spiral::SCHEMA_VERSION
        ));
    }

    let relation_count = manifest.relations.len();
    let mut expected = manifest.relations.into_iter().collect::<BTreeSet<_>>();
    if expected.len() != relation_count {
        return Err(format!(
            "{} manifest contains duplicate relations",
            spec.label
        ));
    }
    if spec.format != PROJECT_FORMAT
        && let Some(name) = expected
            .iter()
            .find(|name| table_named_in(name, spec.tables).is_none())
    {
        return Err(format!(
            "{} manifest contains unknown relation {name:?}",
            spec.label
        ));
    }
    let mut seen = BTreeSet::new();
    let mut entries = BTreeMap::new();
    loop {
        let name_len = read_u16(&mut source)? as usize;
        if name_len == 0 {
            break;
        }
        if name_len > MAX_ENTRY_NAME_BYTES {
            return Err(format!("{} relation name is too long", spec.label));
        }
        let mut name = vec![0_u8; name_len];
        source
            .read_exact(&mut name)
            .map_err(|_| format!("{} relation name is truncated", spec.label))?;
        let name = String::from_utf8(name)
            .map_err(|_| format!("{} relation name is not UTF-8", spec.label))?;
        if !expected.remove(&name) || !seen.insert(name.clone()) {
            return Err(format!(
                "{} contains unexpected or duplicate relation {name:?}",
                spec.label
            ));
        }
        let payload_len = read_u64(&mut source)?;
        let payload_len = usize::try_from(payload_len).map_err(|_| {
            format!(
                "{} relation {name:?} is too large for this machine",
                spec.label
            )
        })?;
        let remaining = bundle.len().saturating_sub(source.position() as usize);
        if payload_len > remaining {
            return Err(format!("{} relation {name:?} is truncated", spec.label));
        }
        let mut payload = vec![0_u8; payload_len];
        source
            .read_exact(&mut payload)
            .map_err(|_| format!("{} relation {name:?} is truncated", spec.label))?;
        entries.insert(name, payload);
    }
    if !expected.is_empty() {
        return Err(format!(
            "{} is missing relations: {}",
            spec.label,
            expected.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if source.position() != bundle.len() as u64 {
        return Err(format!("{} file has trailing bytes", spec.label));
    }
    Ok(entries.into_iter().collect())
}

fn table_named_in(name: &str, tables: &[Table]) -> Option<Table> {
    tables.iter().copied().find(|table| table.name() == name)
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
fn write_u16(target: &mut Vec<u8>, value: usize) -> Result<(), String> {
    let value = u16::try_from(value).map_err(|_| "project entry name is too long".to_owned())?;
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
fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write as _;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use arrow_array::builder::{ListBuilder, StringBuilder};
    use arrow_array::{
        ArrayRef, BinaryArray, Float32Array, Float64Array, Int64Array, LargeBinaryArray, NullArray,
        StringArray, UInt32Array, UInt64Array,
    };
    use arrow_ipc::writer::IpcWriteOptions;
    use arrow_schema::{DataType, Field, Schema};

    use super::*;
    use crate::backend::recording::episodes::{EpisodeCompletion, EpisodeStart, EpisodeStore};
    use crate::backend::recording::spiral::{dataset_batch, episode_completion_batch};
    use crate::backend::storage::manager::{
        StorageManager, ensure_dataset_namespace, execute_update_sql, ingest_batch, run_query,
        table_exists,
    };
    use crate::backend::storage::repository::SpiralRepository;
    use crate::backend::storage::settings::{
        BackendStateStore, environment_spiral_endpoint, environment_spiral_spfs_endpoint,
    };

    static SPIRAL_E2E: Mutex<()> = Mutex::new(());

    fn envelope(manifest: Manifest, entries: &[(&str, &[u8])]) -> Vec<u8> {
        envelope_for(PROJECT_MAGIC, manifest, entries)
    }

    fn envelope_for(magic: &[u8; 8], manifest: Manifest, entries: &[(&str, &[u8])]) -> Vec<u8> {
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut bundle = Vec::new();
        bundle.extend_from_slice(magic);
        write_u32(&mut bundle, manifest.len()).unwrap();
        bundle.extend_from_slice(&manifest);
        for (name, payload) in entries {
            write_u16(&mut bundle, name.len()).unwrap();
            bundle.extend_from_slice(name.as_bytes());
            write_u64(&mut bundle, payload.len()).unwrap();
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

        let error = decode_arrow_payload_with_limits(
            "compressed_bomb",
            payload,
            "project",
            &mut ArrowDecodeBudget::default(),
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
        decode_arrow_payload_with_limits(
            "first_relation",
            payload.clone(),
            "project",
            &mut budget,
            limits,
        )
        .unwrap();

        let error = decode_arrow_payload_with_limits(
            "second_relation",
            payload,
            "project",
            &mut budget,
            limits,
        )
        .unwrap_err();

        assert!(error.contains("decoded-buffer limit"), "{error}");
    }

    #[test]
    fn arrow_decode_rejects_oversized_batch_row_metadata_before_array_decode() {
        let batch = record_batch(
            vec![Field::new("value", DataType::Null, true)],
            vec![Arc::new(NullArray::new(1_001))],
        );
        let error = decode_arrow_payload_with_limits(
            "too_many_rows",
            arrow_payload(&batch),
            "project",
            &mut ArrowDecodeBudget::default(),
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
        let error = decode_arrow_payload_with_limits(
            "too_many_relation_rows",
            arrow_payload_batches(&[batch.clone(), batch]),
            "project",
            &mut ArrowDecodeBudget::default(),
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

        let error = decode_entries(vec![(Table::Datasets, payload)], PROJECT_SPEC).unwrap_err();

        assert!(
            error.contains("1000000-row") && error.contains("limit"),
            "{error}"
        );
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

        let error = decode_entries(vec![(Table::Datasets, arrow_payload(&batch))], PROJECT_SPEC)
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
        let error = decode_arrow_payload_with_limits(
            "too_many_columns",
            arrow_payload(&batch),
            "project",
            &mut ArrowDecodeBudget::default(),
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
        validate_workflow_agent_json(&valid.to_string()).unwrap();

        let mut schema_less = valid.clone();
        schema_less
            .as_object_mut()
            .unwrap()
            .remove("schema_version");
        assert!(
            validate_workflow_agent_json(&schema_less.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut unknown_selection = valid.clone();
        unknown_selection["active_conversation_id"] = serde_json::json!("missing");
        assert!(
            validate_workflow_agent_json(&unknown_selection.to_string())
                .unwrap_err()
                .contains("active conversation")
        );

        let mut duplicate = valid.clone();
        duplicate["conversations"] =
            serde_json::json!([agent_conversation("same"), agent_conversation("same")]);
        duplicate["active_conversation_id"] = serde_json::json!("same");
        assert!(
            validate_workflow_agent_json(&duplicate.to_string())
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
            validate_workflow_agent_json(&over_limit.to_string())
                .unwrap_err()
                .contains("1–20")
        );

        let mut missing_timestamp = valid.clone();
        missing_timestamp["conversations"][0]
            .as_object_mut()
            .unwrap()
            .remove("created_ms");
        assert!(
            validate_workflow_agent_json(&missing_timestamp.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut unknown_nested = valid.clone();
        unknown_nested["conversations"][0]["session"]["messages"][0]["future_field"] =
            serde_json::json!(true);
        assert!(
            validate_workflow_agent_json(&unknown_nested.to_string())
                .unwrap_err()
                .contains("current document shape")
        );

        let mut unknown_top_level = valid;
        unknown_top_level["future_field"] = serde_json::json!(true);
        assert!(
            validate_workflow_agent_json(&unknown_top_level.to_string())
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
            validate_workflow_agent_json(&foreign_workflow.to_string())
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
            let error = validate_workflow_agent_json(&injected.to_string()).unwrap_err();
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
            validate_workflow_agent_json(&legacy.to_string())
                .unwrap_err()
                .contains("current document shape")
        );
        assert!(
            validate_workflow_agent_json(&serde_json::json!({"future": []}).to_string())
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
        let error = match decode_project_entries(vec![(
            Table::WorkflowRuns.name().to_owned(),
            arrow_payload(&workflow_runs),
        )]) {
            Ok(_) => panic!("invalid Assistant history should be rejected before restore"),
            Err(error) => error,
        };
        assert!(error.contains("active conversation"));
    }

    fn workflow_definition(source: &str, name: &str) -> WorkflowFeatureDefinition {
        WorkflowFeatureDefinition {
            name: name.to_owned(),
            description: format!("Detect {name}"),
            concept: name.trim_end_matches("_feature").replace('_', " "),
            applicability: WorkflowFeatureApplicability {
                source_relation: source.to_owned(),
                media_field: "image".to_owned(),
                stream_ids: vec!["camera_front".to_owned()],
            },
        }
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
            for stream in &definition.applicability.stream_ids {
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
                text(|definition| definition.applicability.source_relation.clone()),
                text(|definition| {
                    derived_feature_cache_relation(&definition.applicability.source_relation)
                }),
                text(|definition| definition.name.clone()),
                text(|definition| definition.description.clone()),
                text(|definition| definition.concept.clone()),
                text(|definition| definition.applicability.media_field.clone()),
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
                .map(|definition| Field::new(&definition.name, DataType::Float32, true)),
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
        assert_eq!(parse(&bundle).unwrap(), Vec::new());
        assert!(
            decode_project_entries(parse_raw_bundle(&bundle, PROJECT_SPEC).unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn manifest_and_payload_relations_must_match_exactly() {
        let name = Table::Datasets.name().to_owned();
        let missing = envelope(manifest(vec![name.clone()]), &[]);
        assert!(parse(&missing).unwrap_err().contains("missing relations"));

        let unexpected = envelope(manifest(Vec::new()), &[(name.as_str(), b"arrow")]);
        assert!(
            parse(&unexpected)
                .unwrap_err()
                .contains("unexpected or duplicate")
        );
    }

    #[test]
    fn duplicate_manifest_relations_are_rejected() {
        let name = Table::Datasets.name().to_owned();
        let bundle = envelope(manifest(vec![name.clone(), name]), &[]);
        assert!(parse(&bundle).unwrap_err().contains("duplicate relations"));
    }

    #[test]
    fn project_and_episode_bundle_formats_are_not_interchangeable() {
        let project = envelope(manifest(Vec::new()), &[]);
        assert!(
            parse_bundle(&project, EPISODES_SPEC)
                .unwrap_err()
                .contains("not a Spiral Robotics episode bundle")
        );

        let episodes = envelope_for(
            EPISODES_MAGIC,
            manifest_for(EPISODES_FORMAT, Vec::new()),
            &[],
        );
        assert!(
            parse(&episodes)
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
            parse_bundle(&bundle, EPISODES_SPEC)
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
            validate_episode_relations(&duplicate)
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
            validate_episode_relations(&dangling)
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
            validate_episode_relations(&decoded)
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

        let retargeted = retarget_video_media_relations(batch, "destination").unwrap();

        assert_eq!(
            utf8_values(&retargeted, column::MEDIA_RELATION).unwrap(),
            vec![sql::video_dataset_output("destination"); 2]
        );
        assert_eq!(
            video_episode_ids(&[(Table::VideoAssets, vec![retargeted])]).unwrap(),
            BTreeSet::from(["episode-b".to_owned(), "episode_a".to_owned()])
        );
    }

    #[test]
    fn forged_payload_length_is_rejected_before_allocation() {
        let name = Table::Datasets.name().to_owned();
        let mut bundle = envelope(manifest(vec![name.clone()]), &[(name.as_str(), b"")]);
        let length_offset = bundle.len() - 2 - 8;
        bundle[length_offset..length_offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        let error = parse(&bundle).unwrap_err();
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
        let cache = derived_feature_cache_relation(source);
        let definition = workflow_definition(source, "holding_feature");
        let registry = canonical_registry_batch(std::slice::from_ref(&definition));
        let cache_batch = canonical_cache_batch(std::slice::from_ref(&definition), &[1]);
        let valid = workflow_project_bundle(&registry, &[(cache.as_str(), cache_batch.clone())]);
        let decoded = decode_project_entries(parse_raw_bundle(&valid, PROJECT_SPEC).unwrap())
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
            decode_project_entries(parse_raw_bundle(&missing, PROJECT_SPEC).unwrap())
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
            decode_project_entries(parse_raw_bundle(&undeclared, PROJECT_SPEC).unwrap())
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
            decode_project_entries(parse_raw_bundle(&bundle, PROJECT_SPEC).unwrap())
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
            decode_project_entries(parse_raw_bundle(&bundle, PROJECT_SPEC).unwrap())
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
            decode_project_entries(parse_raw_bundle(&bundle, PROJECT_SPEC).unwrap())
                .unwrap_err()
                .contains("incompatible producer provenance")
        );

        let duplicate = canonical_registry_batch(&[definition.clone(), definition.clone()]);
        assert!(
            validate_workflow_registry(&[duplicate])
                .unwrap_err()
                .contains("duplicate source/name")
        );
        let second = workflow_definition("media.frames", "still_feature");
        let bounded = canonical_registry_batch(&[definition, second]);
        assert!(
            validate_workflow_registry_with_limits(&[bounded], 1, MAX_FEATURE_CACHE_COLUMNS)
                .unwrap_err()
                .contains("bounded limit of 1")
        );
    }

    #[test]
    fn project_cache_uses_atomic_registry_provenance_after_spiral_drops_field_metadata() {
        let definition = workflow_definition("media.frames", "holding_feature");
        let registry_batch = canonical_registry_batch(std::slice::from_ref(&definition));
        let registry = validate_workflow_registry(&[registry_batch.clone()]).unwrap();
        let relation = derived_feature_cache_relation("media.frames");

        let missing_feature = canonical_cache_batch(&[], &[1]);
        let bundle = workflow_project_bundle(
            &registry_batch,
            &[(relation.as_str(), missing_feature.clone())],
        );
        assert!(
            decode_project_entries(parse_raw_bundle(&bundle, PROJECT_SPEC).unwrap())
                .unwrap_err()
                .contains("do not exactly match")
        );

        let canonical = canonical_cache_batch(std::slice::from_ref(&definition), &[1]);
        assert!(canonical.schema().field(5).metadata().is_empty());
        validate_workflow_cache(&relation, std::slice::from_ref(&canonical), &registry)
            .expect("metadata-free cache fields use the validated atomic registry provenance");

        let invalid_score =
            replace_batch_column(&canonical, 5, Arc::new(Float32Array::from(vec![Some(1.5)])));
        assert!(
            validate_workflow_cache(&relation, &[invalid_score], &registry)
                .unwrap_err()
                .contains("sigmoid probability semantics")
        );
    }

    #[test]
    fn project_registry_accepts_pinned_spiral_nested_utf8view_normalization_only() {
        let expected = DataType::List(Arc::new(Field::new("item", DataType::Utf8, false)));
        let normalized = DataType::List(Arc::new(Field::new("item", DataType::Utf8View, false)));
        assert!(workflow_storage_type_matches(&normalized, &expected));
        assert!(!workflow_storage_type_matches(
            &DataType::List(Arc::new(Field::new("item", DataType::BinaryView, false,))),
            &expected
        ));
        assert!(!workflow_storage_type_matches(
            &DataType::List(Arc::new(Field::new("item", DataType::Utf8View, true,))),
            &expected
        ));
    }

    #[test]
    fn project_cache_rejects_duplicate_keys_and_bounded_overflow() {
        let first = workflow_definition("media.frames", "holding_feature");
        let second = workflow_definition("media.frames", "still_feature");
        let registry_batch = canonical_registry_batch(&[first.clone(), second.clone()]);
        let registry = validate_workflow_registry(&[registry_batch]).unwrap();
        let relation = derived_feature_cache_relation("media.frames");
        let duplicate = canonical_cache_batch(&[first.clone(), second.clone()], &[1, 1]);
        assert!(
            validate_workflow_cache(&relation, &[duplicate], &registry)
                .unwrap_err()
                .contains("duplicate stable frame identities")
        );

        let rows = canonical_cache_batch(&[first.clone(), second.clone()], &[1, 2]);
        assert!(
            validate_workflow_cache_with_limits(
                &relation,
                &[rows.clone()],
                &registry,
                1,
                MAX_FEATURE_CACHE_COLUMNS,
            )
            .unwrap_err()
            .contains("bounded limit of 1 rows")
        );
        assert!(
            validate_workflow_cache_with_limits(
                &relation,
                &[rows],
                &registry,
                MAX_FEATURE_CACHE_ROWS,
                1,
            )
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
            workflow_restore_plan(&current, Some(&incoming)),
            WorkflowRestorePlan {
                retire_caches: vec!["workflow.features_a".to_owned()],
                clear_registry: false,
            }
        );
        assert_eq!(
            workflow_restore_plan(&current, None),
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
        let result = run_project_import_transaction(&mut success, "project import", |connection| {
            connection.events.push("mutation");
            Ok(7)
        })
        .unwrap();
        assert_eq!(result, (7, None));
        assert_eq!(success.events, ["begin", "mutation", "commit", "restore"]);
        assert!(!success.discarded);

        let mut failure = MockProjectTransaction::default();
        let error =
            run_project_import_transaction::<_, ()>(&mut failure, "project import", |connection| {
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
        let error =
            run_project_import_transaction(&mut begin, "project import", |_| Ok(())).unwrap_err();
        assert!(error.contains("begin failed") && error.contains("discarded"));
        assert_eq!(begin.events, ["begin", "discard"]);

        let mut rollback = MockProjectTransaction {
            rollback_error: Some("rollback failed"),
            ..Default::default()
        };
        let error =
            run_project_import_transaction::<_, ()>(&mut rollback, "project import", |_| {
                Err("mutation failed".to_owned())
            })
            .unwrap_err();
        assert!(error.contains("mutation failed") && error.contains("rollback failed"));
        assert_eq!(rollback.events, ["begin", "rollback", "discard"]);

        let mut uncertain_commit = MockProjectTransaction {
            commit_error: Some("commit failed"),
            ..Default::default()
        };
        let error =
            run_project_import_transaction(&mut uncertain_commit, "project import", |_| Ok(()))
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
        let error =
            run_project_import_transaction(&mut commit, "project import", |_| Ok(())).unwrap_err();
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
        let error = run_project_import_transaction::<_, ()>(
            &mut rollback_restore,
            "project import",
            |_| Err("mutation failed".to_owned()),
        )
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
        let (value, warning) =
            run_project_import_transaction(&mut transaction, "project import", |_| Ok(9)).unwrap();
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
    /// cargo test -p spiral-robotics-studio-backend \
    ///   backend::storage::bundle::tests::spiral_export_import_round_trip_overwrites_existing_data \
    ///   -- --ignored --exact
    /// ```
    #[test]
    #[ignore = "requires a real SpiralDB server, SpFS, and disposable project access"]
    fn spiral_export_import_round_trip_overwrites_existing_data() {
        let _serial = SPIRAL_E2E
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository = Arc::new(
            SpiralRepository::new(
                configured_spiral_endpoint(),
                configured_spiral_spfs_endpoint(),
                Some(project),
                stored_credentials(),
                namespace,
            )
            .expect("the Spiral E2E repository must be valid"),
        );
        let _cleanup = TestNamespace::new(repository.clone());
        let storage = StorageManager::new(repository.clone());

        repository
            .run_exclusive_blocking(|connection| {
                ensure_dataset_namespace(connection, repository.namespace())?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::StudioSettings),
                    settings_batch("exported-setting", r#"{"source":"export"}"#, 10),
                )?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::TrainingCheckpoints),
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

        repository
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, repository.namespace())?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::StudioSettings),
                    settings_batch("replacement-setting", r#"{"source":"replacement"}"#, 20),
                )?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::RolloutRuns),
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
        assert_eq!(summary.relations, 2);
        assert_eq!(summary.rows, 2);

        let restored = storage
            .export_project_bundle()
            .expect("re-exporting the imported Spiral project must succeed");
        assert_eq!(relation_payloads(&restored), exported_payloads);
        repository
            .run_blocking(|connection| {
                assert!(!table_exists(
                    connection,
                    &sql::relation(repository.namespace(), Table::RolloutRuns)
                )?);
                Ok(())
            })
            .expect("checking overwrite semantics must succeed");
    }

    /// Run with the same real-Spiral environment as the project transfer E2E.
    #[test]
    #[ignore = "requires a real SpiralDB server, SpFS, and disposable project access"]
    fn spiral_episode_export_import_round_trip_merges_without_overwrite() {
        let _serial = SPIRAL_E2E
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_episode_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository = Arc::new(
            SpiralRepository::new(
                configured_spiral_endpoint(),
                configured_spiral_spfs_endpoint(),
                Some(project),
                stored_credentials(),
                namespace,
            )
            .expect("the Spiral episode E2E repository must be valid"),
        );
        let _cleanup = TestNamespace::new(repository.clone());
        let storage = StorageManager::new(repository.clone());

        repository
            .run_exclusive_blocking(|connection| {
                ensure_dataset_namespace(connection, repository.namespace())?;
                seed_episode(
                    connection,
                    repository.namespace(),
                    "source",
                    "episode_exported",
                    10,
                )
            })
            .expect("seeding the source episode must succeed");

        let exported = storage
            .export_episode_bundle("source")
            .expect("exporting source episodes must succeed");
        let exported_relations = parse_bundle(&exported, EPISODES_SPEC)
            .expect("the exported episode bundle must parse")
            .into_iter()
            .map(|(table, _)| table)
            .collect::<Vec<_>>();
        assert_eq!(exported_relations, vec![Table::Datasets, Table::Episodes]);

        repository
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, repository.namespace())?;
                seed_episode(
                    connection,
                    repository.namespace(),
                    "target",
                    "episode_existing",
                    20,
                )
            })
            .expect("seeding the target dataset must succeed");

        let summary = storage
            .import_episode_bundle("target", &exported)
            .expect("importing episodes into the target dataset must succeed");
        assert_eq!(summary.dataset_id, "target");
        assert_eq!(summary.episodes, 1);
        assert_eq!(summary.first_episode.as_deref(), Some("episode_exported"));

        repository
            .run_blocking(|connection| {
                let episodes = sql::relation(repository.namespace(), Table::Episodes);
                let result = run_query(
                    connection,
                    &format!(
                        "SELECT {episode_id}, {dataset_id} FROM {episodes} ORDER BY {episode_id}",
                        episode_id = column::EPISODE_ID,
                        dataset_id = column::DATASET_ID,
                    ),
                    10,
                )?;
                assert_eq!(result.rows.len(), 2);
                assert!(result.rows.iter().all(|row| {
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
        let _serial = SPIRAL_E2E
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = required_project();
        let namespace = format!(
            "spiral_robotics_video_transfer_e2e_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository = Arc::new(
            SpiralRepository::new(
                configured_spiral_endpoint(),
                configured_spiral_spfs_endpoint(),
                Some(project),
                stored_credentials(),
                namespace,
            )
            .expect("the Spiral video import E2E repository must be valid"),
        );
        let _cleanup = TestNamespace::new(repository.clone());
        let storage = StorageManager::new(repository.clone());
        let episode_id = "episode_video_import";
        let start = EpisodeStart {
            id: episode_id.to_owned(),
            dataset_id: "video-transfer".to_owned(),
            started_ms: 10,
            sample_hz: 2.0,
            source: "transfer-e2e".to_owned(),
            note: None,
            arms: vec![spiral_robotics_contracts::api::EpisodeArm {
                id: "follower".to_owned(),
                robot_type: Some("test".to_owned()),
                joints: vec!["joint".to_owned()],
            }],
            condition: Vec::new(),
        };
        let completion = EpisodeCompletion {
            duration_seconds: 1.0,
            recording_gaps: Vec::new(),
            max_follow_error_degrees: None,
            discarded: false,
            condition: Vec::new(),
        };
        let mut mp4 = test_mp4();
        // A valid top-level `free` box makes the fixture large enough to cross
        // Spiral's managed-Blob outlining threshold without making FFmpeg
        // encode hundreds of thousands of incompressible video bytes.
        let free_box_size = 300_000_u32;
        mp4.extend_from_slice(&free_box_size.to_be_bytes());
        mp4.extend_from_slice(b"free");
        mp4.resize(mp4.len() + free_box_size as usize - 8, 0);
        assert!(mp4.len() > 256 * 1024);

        repository
            .run_exclusive_blocking(|connection| {
                ensure_dataset_namespace(connection, repository.namespace())?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::Datasets),
                    dataset_batch(&start).map_err(|error| error.to_string())?,
                )?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::Episodes),
                    episode_completion_batch(&start, &completion, 0)
                        .map_err(|error| error.to_string())?,
                )?;
                ingest_batch(
                    connection,
                    &sql::relation(repository.namespace(), Table::VideoAssets),
                    video_asset_batch(episode_id, &mp4),
                )?;
                ensure_video_assets_blob_relation(connection, repository.namespace())
            })
            .expect("legacy Binary video rows must migrate to Blob without derived resources");
        let exported = storage
            .export_project_bundle()
            .expect("exporting the video project must succeed");
        repository
            .run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, repository.namespace())
            })
            .expect("clearing the relational fixture before import must succeed");

        let first_import = storage
            .import_project_bundle(&exported)
            .expect("importing a video project must rebuild its managed dataset");
        assert!(
            first_import.warnings.is_empty(),
            "first import warnings: {:?}",
            first_import.warnings
        );
        let second_import = storage
            .import_project_bundle(&exported)
            .expect("re-importing a video project must replace its managed dataset outputs");
        assert!(
            second_import.warnings.is_empty(),
            "second import warnings: {:?}",
            second_import.warnings
        );

        repository
            .run_blocking(|connection| {
                let assets = sql::relation(repository.namespace(), Table::VideoAssets);
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
                let descriptor = run_query(
                    connection,
                    &format!(
                        "SELECT spql.blob.uri({payload}) AS uri FROM {assets}",
                        payload = column::PAYLOAD,
                    ),
                    1,
                )?;
                assert!(
                    descriptor.rows[0]
                        .get("uri")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|uri| !uri.is_empty()),
                    "large MP4 must be outlined into a provider-backed Blob",
                );
                let range = run_binary_query(
                    connection,
                    &sql::video_asset_range(
                        repository.namespace(),
                        episode_id,
                        "camera.main",
                        Some(0),
                        512,
                        1024,
                    ),
                )?;
                assert_eq!(range, mp4[512..1536]);

                let output = sql::video_dataset_output(repository.namespace());
                let rows = run_query(connection, &sql::count_rows(&output), 1)?;
                assert_eq!(
                    rows.rows[0].get("rows").and_then(serde_json::Value::as_u64),
                    Some(1)
                );
                let frames = run_query(
                    connection,
                    &format!(
                        "SELECT COUNT(*) AS count FROM {output} AS asset, \
                         LATERAL spql.video.frames(asset.media, \
                         spql.video.frame_indices(ARRAY[0])) AS frame"
                    ),
                    1,
                )?;
                assert_eq!(
                    frames.rows[0]
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
    /// cargo test -p spiral-robotics-studio-backend \
    ///   backend::storage::bundle::tests::spiral_profile_project_import \
    ///   -- --ignored --exact --nocapture
    /// ```
    #[test]
    #[ignore = "profiles a real project import and creates temporary Spiral resources"]
    fn spiral_profile_project_import() {
        let _serial = SPIRAL_E2E
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bundle_path = std::env::var("SPIRAL_PROJECT_BUNDLE")
            .expect("SPIRAL_PROJECT_BUNDLE must name the project archive to profile");
        let process_start = Instant::now();
        eprintln!(
            "PROFILE\tmeta\tpid={}\tpath={bundle_path}",
            std::process::id()
        );
        let file_start = Instant::now();
        let bytes = fs::read(&bundle_path).expect("the profiling archive must be readable");
        profile_event("studio.file_read", file_start, 0, bytes.len());

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
        let repository = Arc::new(
            SpiralRepository::new(
                configured_spiral_endpoint(),
                configured_spiral_spfs_endpoint(),
                Some(project),
                stored_credentials(),
                namespace,
            )
            .expect("the Spiral profiling repository must be valid"),
        );
        profile_event("studio.repository_config", repository_start, 0, 0);
        let cleanup = TestNamespace::new(repository.clone());

        let namespace_start = Instant::now();
        repository
            .run_exclusive_blocking(|connection| {
                ensure_dataset_namespace(connection, repository.namespace())
            })
            .expect("the profiling namespace must be created");
        profile_event("spiral.setup_namespace", namespace_start, 0, 0);

        let exclusive_start = Instant::now();
        let summary = repository
            .run_exclusive_blocking(|connection| {
                profile_project_import_into(connection, repository.namespace(), &bytes)
            })
            .expect("the profiled project import must succeed");
        profile_event(
            "studio.exclusive_import_total",
            exclusive_start,
            summary.rows as usize,
            bytes.len(),
        );

        let storage = StorageManager::new(repository.clone());
        let preferences_start = Instant::now();
        storage
            .load_studio_preferences()
            .expect("profiled preferences reload must succeed");
        profile_event("studio.reload_preferences", preferences_start, 0, 0);
        let training_start = Instant::now();
        storage
            .load_training_snapshot::<spiral_robotics_contracts::api::TrainingSnapshot>()
            .expect("profiled training reload must succeed");
        profile_event("studio.reload_training", training_start, 0, 0);
        let rollout_start = Instant::now();
        storage
            .load_rollout_snapshot::<spiral_robotics_contracts::api::RolloutSnapshot>()
            .expect("profiled rollout reload must succeed");
        profile_event("studio.reload_rollout", rollout_start, 0, 0);
        let catalog_start = Instant::now();
        let episodes = EpisodeStore::new(repository.clone()).list();
        profile_event(
            "studio.refresh_episode_catalog",
            catalog_start,
            episodes.len(),
            0,
        );

        profile_event(
            "studio.route_equivalent_total",
            process_start,
            summary.rows as usize,
            bytes.len(),
        );
        eprintln!(
            "PROFILE\tmeta\trelations={}\trows={}\twarnings={}",
            summary.relations,
            summary.rows,
            summary.warnings.len()
        );
        let cleanup_start = Instant::now();
        drop(cleanup);
        profile_event("spiral.cleanup", cleanup_start, 0, 0);
    }

    /// Measure hosted Arbor's fixed create, append, and replacement costs
    /// separately from project parsing, video reconciliation, and Studio state
    /// reloads.
    ///
    /// ```text
    /// SPIRAL_PROJECT_BUNDLE=/absolute/path/to/project.spiral-project \
    /// cargo test -p spiral-robotics-studio-backend \
    ///   backend::storage::bundle::tests::spiral_profile_write_fixed_cost \
    ///   -- --ignored --exact --nocapture
    /// ```
    #[test]
    #[ignore = "profiles real hosted Arbor replacements in temporary Spiral resources"]
    fn spiral_profile_write_fixed_cost() {
        let _serial = SPIRAL_E2E
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bundle_path = std::env::var("SPIRAL_PROJECT_BUNDLE")
            .expect("SPIRAL_PROJECT_BUNDLE must name the project archive to profile");
        let bytes = fs::read(&bundle_path).expect("the profiling archive must be readable");
        let mut decoded = decode_entries(
            parse(&bytes).expect("the profiling archive must parse"),
            PROJECT_SPEC,
        )
        .expect("the profiling archive must decode");
        sort_relations(&mut decoded, &Table::ALL);

        let project = required_project();
        let namespace = format!(
            "spiral_robotics_replace_profile_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        );
        let repository = Arc::new(
            SpiralRepository::new(
                configured_spiral_endpoint(),
                configured_spiral_spfs_endpoint(),
                Some(project),
                stored_credentials(),
                namespace,
            )
            .expect("the Spiral profiling repository must be valid"),
        );
        let cleanup = TestNamespace::new(repository.clone());

        repository
            .run_exclusive_blocking(|connection| {
                ensure_dataset_namespace(connection, repository.namespace())?;
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
                    let target = format!("{}.{label}", repository.namespace());
                    let ingest_target = format!("{}.{label}_ingest", repository.namespace());
                    let autocommit_target =
                        format!("{}.{label}_autocommit", repository.namespace());

                    profile_autocommit_ingest_operation(
                        connection,
                        label,
                        "ingest_missing_autocommit",
                        &autocommit_target,
                        &batches,
                        rows,
                        encoded_bytes,
                    )?;
                    profile_autocommit_ingest_operation(
                        connection,
                        label,
                        "append_existing_autocommit",
                        &autocommit_target,
                        &batches,
                        rows,
                        encoded_bytes,
                    )?;
                    profile_write_operation(
                        connection,
                        label,
                        "ingest_missing",
                        &ingest_target,
                        &batches,
                        rows,
                        encoded_bytes,
                        false,
                    )?;
                    profile_write_operation(
                        connection,
                        label,
                        "replace_missing",
                        &target,
                        &batches,
                        rows,
                        encoded_bytes,
                        true,
                    )?;
                    profile_write_operation(
                        connection,
                        label,
                        "existing_cold",
                        &target,
                        &batches,
                        rows,
                        encoded_bytes,
                        true,
                    )?;

                    for bind_index in 1..=10 {
                        let bind_start = Instant::now();
                        let exists = connection.table_schema(&target)?.is_some();
                        eprintln!(
                            "PROFILE_SCOPED\tcase={label}\tphase=bind_existing_{bind_index}\tseconds={:.6}\trows={rows}\tbytes={encoded_bytes}\texists={exists}",
                            bind_start.elapsed().as_secs_f64()
                        );
                    }
                    let missing_target = format!("{}.missing", repository.namespace());
                    for bind_index in 1..=5 {
                        let bind_start = Instant::now();
                        let exists = connection.table_schema(&missing_target)?.is_some();
                        eprintln!(
                            "PROFILE_SCOPED\tcase={label}\tphase=bind_missing_{bind_index}\tseconds={:.6}\trows=0\tbytes=0\texists={exists}",
                            bind_start.elapsed().as_secs_f64()
                        );
                    }
                    profile_write_operation(
                        connection,
                        label,
                        "existing_after_bind",
                        &target,
                        &batches,
                        rows,
                        encoded_bytes,
                        true,
                    )?;
                }
                Ok(())
            })
            .expect("scoped Spiral replacement profiling must succeed");

        let cleanup_start = Instant::now();
        drop(cleanup);
        profile_event("spiral.cleanup", cleanup_start, 0, 0);
    }

    fn profile_autocommit_ingest_operation(
        connection: &mut RepositoryConnection,
        label: &str,
        phase: &str,
        target: &str,
        batches: &[RecordBatch],
        rows: usize,
        bytes: usize,
    ) -> Result<(), String> {
        let write_start = Instant::now();
        ingest_batches(connection, target, batches.to_vec())?;
        let write_seconds = write_start.elapsed().as_secs_f64();
        eprintln!(
            "PROFILE_SCOPED\tcase={label}\tphase={phase}\tseconds={write_seconds:.6}\trows={rows}\tbytes={bytes}"
        );
        Ok(())
    }

    fn profile_write_operation(
        connection: &mut RepositoryConnection,
        label: &str,
        phase: &str,
        target: &str,
        batches: &[RecordBatch],
        rows: usize,
        bytes: usize,
        replace: bool,
    ) -> Result<(), String> {
        let begin_start = Instant::now();
        connection.set_autocommit(false)?;
        let begin_seconds = begin_start.elapsed().as_secs_f64();

        let write_start = Instant::now();
        if replace {
            replace_batches(connection, target, batches.to_vec())?;
        } else {
            ingest_batches(connection, target, batches.to_vec())?;
        }
        let write_seconds = write_start.elapsed().as_secs_f64();

        let commit_start = Instant::now();
        commit_catalog_transaction(connection)?;
        let commit_seconds = commit_start.elapsed().as_secs_f64();
        connection.set_autocommit(true)?;

        eprintln!(
            "PROFILE_SCOPED\tcase={label}\tphase={phase}\tseconds={write_seconds:.6}\tbegin_seconds={begin_seconds:.6}\tcommit_seconds={commit_seconds:.6}\trows={rows}\tbytes={bytes}"
        );
        Ok(())
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
        connection: &mut RepositoryConnection,
        namespace: &str,
        bundle: &[u8],
    ) -> Result<ProjectImportSummary, String> {
        let parse_start = Instant::now();
        let entries = parse(bundle)?;
        profile_event("studio.bundle_parse", parse_start, 0, bundle.len());

        let mut decoded = Vec::new();
        for (table, payload) in entries {
            let payload_bytes = payload.len();
            let decode_start = Instant::now();
            let mut relation = decode_entries(vec![(table, payload)], PROJECT_SPEC)?;
            let rows = relation
                .iter()
                .flat_map(|(_, batches)| batches)
                .map(RecordBatch::num_rows)
                .sum();
            profile_event(
                &format!("studio.arrow_decode.{}", table.name()),
                decode_start,
                rows,
                payload_bytes,
            );
            decoded.append(&mut relation);
        }
        sort_relations(&mut decoded, &Table::ALL);
        if let Some(limit) = profile_usize("SPIRAL_PROFILE_VIDEO_LIMIT") {
            limit_relation_rows(&mut decoded, Table::VideoAssets, limit);
        }

        let video_ids_start = Instant::now();
        let imported_video_episodes = video_episode_ids(&decoded)?;
        profile_event(
            "studio.collect_video_episode_ids",
            video_ids_start,
            imported_video_episodes.len(),
            0,
        );
        let transaction_start = Instant::now();
        connection
            .set_autocommit(false)
            .map_err(|error| format!("starting profiled project import transaction: {error}"))?;
        profile_event("spiral.transaction_begin", transaction_start, 0, 0);

        let outcome = (|| {
            for table in Table::ALL.into_iter().rev() {
                if decoded.iter().any(|(incoming, _)| *incoming == table) {
                    continue;
                }
                let relation = sql::relation(namespace, table);
                let exists_start = Instant::now();
                let exists = table_exists(connection, &relation)?;
                profile_event(
                    &format!("spiral.table_exists.{}", table.name()),
                    exists_start,
                    0,
                    0,
                );
                if exists {
                    let drop_start = Instant::now();
                    execute_update_sql(connection, &format!("DROP TABLE {relation}"))?;
                    profile_event(
                        &format!("spiral.drop_relation.{}", table.name()),
                        drop_start,
                        0,
                        0,
                    );
                }
            }

            let video_chunk_rows = profile_usize("SPIRAL_PROFILE_VIDEO_CHUNK_ROWS");
            let mut summary = ProjectImportSummary::default();
            for (table, batches) in decoded {
                let relation = sql::relation(namespace, table);
                summary.relations += 1;
                let relation_ingest_start = Instant::now();
                let mut relation_rows = 0usize;
                let mut relation_bytes = 0usize;
                let mut stream_batches = Vec::new();
                for batch in batches {
                    let transform_start = Instant::now();
                    let batch = if table == Table::VideoAssets {
                        prepare_video_asset_import(batch, namespace)?
                    } else {
                        batch
                    };
                    profile_event(
                        &format!("studio.transform.{}", table.name()),
                        transform_start,
                        batch.num_rows(),
                        batch.get_array_memory_size(),
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
                        summary.rows = summary.rows.saturating_add(len as u64);
                    }
                }
                let stream_ingest_start = Instant::now();
                replace_batches(connection, &relation, stream_batches)?;
                profile_event(
                    &format!("spiral.replace.{}.stream", table.name()),
                    stream_ingest_start,
                    relation_rows,
                    relation_bytes,
                );
                profile_event(
                    &format!("spiral.replace_total.{}", table.name()),
                    relation_ingest_start,
                    relation_rows,
                    0,
                );
            }
            Ok(summary)
        })();

        let mut summary = match outcome {
            Ok(summary) => {
                let commit_start = Instant::now();
                commit_catalog_transaction(connection)
                    .map_err(|error| format!("committing profiled project import: {error}"))?;
                profile_event(
                    "spiral.transaction_commit",
                    commit_start,
                    summary.rows as usize,
                    0,
                );
                summary
            }
            Err(error) => {
                let rollback_start = Instant::now();
                let rollback = connection.rollback();
                profile_event("spiral.transaction_rollback", rollback_start, 0, 0);
                return Err(match rollback {
                    Ok(()) => error,
                    Err(rollback) => format!("{error}; rolling back also failed: {rollback}"),
                });
            }
        };
        let autocommit_start = Instant::now();
        if let Err(error) = connection.set_autocommit(true) {
            connection.discard();
            summary.warnings.push(format!(
                "profiled project import committed, but its Spiral connection was discarded: {error}"
            ));
        }
        profile_event(
            "spiral.transaction_restore_autocommit",
            autocommit_start,
            0,
            0,
        );

        if !profile_bool("SPIRAL_PROFILE_SKIP_VIDEO_RECONCILE") {
            let reconcile_total_start = Instant::now();
            if imported_video_episodes.is_empty() {
                drop_video_dataset(connection, namespace)?;
            } else {
                let _ = profile_rebuild_video_dataset(connection, namespace)?;
            }
            profile_event(
                "spiral.reconcile_video_total",
                reconcile_total_start,
                imported_video_episodes.len(),
                0,
            );
        }
        Ok(summary)
    }

    fn profile_rebuild_video_dataset(
        connection: &mut RepositoryConnection,
        namespace: &str,
    ) -> Result<bool, String> {
        let exists_start = Instant::now();
        let dataset_exists = video_dataset_exists(connection, namespace)?;
        profile_event("spiral.reconcile_video_exists", exists_start, 1, 0);

        let sources = sql::video_sources_relation(namespace);
        let assets = sql::relation(namespace, Table::VideoAssets);
        let dataset = sql::video_dataset_relation(namespace);
        let view_start = Instant::now();
        execute_update_sql(connection, &sql::create_video_sources(&sources, &assets))
            .map_err(|error| format!("creating Spiral video source view: {error}"))?;
        profile_event("spiral.reconcile_video_view", view_start, 1, 0);
        if !dataset_exists {
            let create_start = Instant::now();
            execute_update_sql(connection, &sql::create_video_dataset(&dataset, &sources))
                .map_err(|error| format!("creating Spiral MP4 dataset: {error}"))?;
            profile_event("spiral.reconcile_video_create", create_start, 1, 0);
        }
        let refresh_start = Instant::now();
        execute_update_sql(connection, &sql::refresh_dataset(&dataset))
            .map_err(|error| format!("refreshing Spiral MP4 dataset: {error}"))?;
        profile_event("spiral.reconcile_video_refresh", refresh_start, 1, 0);
        Ok(dataset_exists)
    }

    fn profile_event(stage: &str, start: Instant, rows: usize, bytes: usize) {
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

    fn stored_credentials() -> Option<crate::backend::storage::settings::SpiralCredentials> {
        BackendStateStore::open_default().ok()?.credentials()
    }

    fn configured_spiral_endpoint() -> String {
        BackendStateStore::open_default()
            .ok()
            .and_then(|store| store.spiral_endpoint())
            .unwrap_or_else(environment_spiral_endpoint)
    }

    fn configured_spiral_spfs_endpoint() -> String {
        BackendStateStore::open_default()
            .ok()
            .and_then(|store| store.spiral_spfs_endpoint())
            .unwrap_or_else(environment_spiral_spfs_endpoint)
    }

    fn relation_payloads(bundle: &[u8]) -> BTreeMap<String, Vec<u8>> {
        parse_raw_bundle(bundle, PROJECT_SPEC)
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

    fn seed_episode(
        connection: &mut RepositoryConnection,
        namespace: &str,
        dataset_id: &str,
        episode_id: &str,
        started_ms: i64,
    ) -> Result<(), String> {
        let start = EpisodeStart {
            id: episode_id.to_owned(),
            dataset_id: dataset_id.to_owned(),
            started_ms,
            sample_hz: 60.0,
            source: "transfer-e2e".to_owned(),
            note: None,
            arms: Vec::new(),
            condition: Vec::new(),
        };
        let completion = EpisodeCompletion {
            duration_seconds: 0.0,
            recording_gaps: Vec::new(),
            max_follow_error_degrees: None,
            discarded: false,
            condition: Vec::new(),
        };
        ingest_batch(
            connection,
            &sql::relation(namespace, Table::Datasets),
            dataset_batch(&start).map_err(|error| error.to_string())?,
        )?;
        ingest_batch(
            connection,
            &sql::relation(namespace, Table::Episodes),
            episode_completion_batch(&start, &completion, 0).map_err(|error| error.to_string())?,
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

    fn drop_studio_relations(
        connection: &mut RepositoryConnection,
        namespace: &str,
    ) -> Result<(), String> {
        drop_video_dataset(connection, namespace)?;
        for table in Table::ALL.into_iter().rev() {
            let relation = sql::relation(namespace, table);
            if table_exists(connection, &relation)? {
                execute_update_sql(connection, &format!("DROP TABLE {relation}"))?;
            }
        }
        Ok(())
    }

    struct TestNamespace {
        repository: Arc<SpiralRepository>,
    }

    impl TestNamespace {
        fn new(repository: Arc<SpiralRepository>) -> Self {
            Self { repository }
        }
    }

    impl Drop for TestNamespace {
        fn drop(&mut self) {
            let namespace = self.repository.namespace().to_owned();
            let _ = self.repository.run_exclusive_blocking(|connection| {
                drop_studio_relations(connection, &namespace)?;
                if table_exists(connection, &namespace)? {
                    execute_update_sql(connection, &format!("DROP TABLE {namespace}"))?;
                }
                Ok(())
            });
        }
    }
}
