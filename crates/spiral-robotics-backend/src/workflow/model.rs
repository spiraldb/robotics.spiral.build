//! Blocking SigLIP worker for relation-first Workflow Model nodes.
//!
//! The async manager owns run admission and snapshots. This module owns the
//! pinned artifacts, device, bounded Arrow/image pipeline, and derived Spiral
//! relation. All identifiers arrive prevalidated and server-derived.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{
    Array as _, ArrayRef, Float32Array, Int64Array, LargeListArray, LargeStringArray, ListArray,
    RecordBatch, StringArray, StringViewArray, StructArray, UInt8Array, UInt32Array, UInt64Array,
};
use arrow_schema::extension::EXTENSION_TYPE_NAME_KEY;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use arrow_select::take::take;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::{clip, siglip};
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Repo, RepoType};
use image::imageops::FilterType;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use spiral_robotics_api::workflow::{
    MAX_SIGLIP_CONCEPT_LENGTH, MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH,
    MAX_WORKFLOW_FEATURE_STREAMS, SIGLIP_BASE_MODEL_ID, SIGLIP_BASE_REPOSITORY,
    SIGLIP_BASE_REVISION, WORKFLOW_FEATURE_REGISTRY_RELATION as FEATURE_REGISTRY_RELATION,
    WorkflowFeatureDefinition, WorkflowModelExecution, WorkflowModelIdentity, WorkflowModelMetrics,
    WorkflowModelRunMetadata, WorkflowNodeProgress, WorkflowQuarantineSummary,
    derived_feature_cache_relation,
};
use spiral_robotics_core::query::{ReadOnlyQuery, ReadOnlyQueryMode, UnquotedSqlIdentifier};
use spiral_robotics_core::workflow::QualifiedRelation;
use tokenizers::Tokenizer;
use tokenizers::utils::padding::{PaddingDirection, PaddingParams, PaddingStrategy};
use tokenizers::utils::truncation::TruncationParams;

use spiral_robotics_storage::ProjectStore;

const IMAGE_EXTENSION_NAME: &str = "spql.image.Image";
const INPUT_PREFETCH_CHUNKS: usize = 2;
const MAX_QUARANTINE_EXAMPLES: usize = 20;
const DEFAULT_MODEL_INPUT_BUDGET_BYTES: usize = 8 * 1024 * 1024;
const MIN_MODEL_CHUNK_ROWS: usize = 1;
const MAX_MODEL_CHUNK_ROWS: usize = 16;
const CACHE_KEY_FIELDS: [(&str, DataType); 5] = [
    ("episode_id", DataType::Utf8),
    ("stream_id", DataType::Utf8),
    ("segment_index", DataType::UInt32),
    ("frame_index", DataType::UInt64),
    ("episode_time_ns", DataType::UInt64),
];
const FEATURE_VALUE_SEMANTICS: &str = "sigmoid_probability_0_1";
const MODEL_RUN_CANCELLED: &str = "Workflow Model run was cancelled.";
const MAX_FEATURE_REGISTRY_ROWS: usize = 4_096;
const MAX_FEATURE_CACHE_ROWS: usize = 1_000_000;
const MAX_FEATURE_CACHE_COLUMNS: usize = 256;

fn cache_key_declaration_may_be_nullable(name: &str) -> bool {
    matches!(name, "frame_index" | "episode_time_ns")
}

pub(crate) struct ModelRunRequest {
    candidate_sql: String,
    feature_cache_relation: String,
    features: Vec<WorkflowFeatureDefinition>,
}

pub(crate) struct ModelRunResult {
    rows: usize,
    requested_feature_cells: u64,
    computed_feature_cells: u64,
    reused_feature_cells: u64,
    quarantine: WorkflowQuarantineSummary,
    metadata: WorkflowModelRunMetadata,
    warnings: Vec<String>,
}

impl ModelRunRequest {
    pub(crate) fn new(
        candidate_sql: impl Into<String>,
        feature_cache_relation: impl Into<String>,
        features: Vec<WorkflowFeatureDefinition>,
    ) -> Self {
        Self {
            candidate_sql: candidate_sql.into(),
            feature_cache_relation: feature_cache_relation.into(),
            features,
        }
    }
}

impl ModelRunResult {
    pub(crate) const fn rows(&self) -> usize {
        self.rows
    }
    pub(crate) const fn requested_feature_cells(&self) -> u64 {
        self.requested_feature_cells
    }
    pub(crate) const fn computed_feature_cells(&self) -> u64 {
        self.computed_feature_cells
    }
    pub(crate) const fn reused_feature_cells(&self) -> u64 {
        self.reused_feature_cells
    }
    pub(crate) const fn quarantine(&self) -> &WorkflowQuarantineSummary {
        &self.quarantine
    }
    pub(crate) const fn metadata(&self) -> &WorkflowModelRunMetadata {
        &self.metadata
    }
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ModelRunFailure {
    Cancelled,
    Failed(String),
}

impl From<String> for ModelRunFailure {
    fn from(error: String) -> Self {
        if error == MODEL_RUN_CANCELLED {
            Self::Cancelled
        } else {
            Self::Failed(error)
        }
    }
}

pub(crate) enum ModelRunEvent {
    Loading(String),
    Validating(String),
    Materializing(String),
    Running {
        detail: String,
        progress: WorkflowNodeProgress,
    },
}

pub(crate) struct ModelRunContext {
    project_store: Arc<ProjectStore>,
    request: ModelRunRequest,
    cancel: Arc<AtomicBool>,
    publication_gate: Arc<Mutex<()>>,
}

impl ModelRunContext {
    pub(crate) fn new(
        project_store: Arc<ProjectStore>,
        request: ModelRunRequest,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        Self {
            project_store,
            request,
            cancel,
            publication_gate: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn with_publication_gate(mut self, gate: Arc<Mutex<()>>) -> Self {
        self.publication_gate = gate;
        self
    }
}

pub(crate) fn run_model(
    context: ModelRunContext,
    mut publish: impl FnMut(ModelRunEvent),
) -> Result<ModelRunResult, ModelRunFailure> {
    let ModelRunContext {
        project_store,
        request,
        cancel,
        publication_gate,
    } = context;
    ReadOnlyQuery::new(&request.candidate_sql, ReadOnlyQueryMode::Training)
        .authorize()
        .map_err(|error| {
            error
                .to_string()
                .replace("Training SQL", "Workflow candidate SQL")
        })?;
    publish(ModelRunEvent::Validating(
        "Inspecting candidate keys and feature cache".to_owned(),
    ));
    let scan = CandidateScanRequest::new(&request.candidate_sql, &request.features, &cancel)
        .scan(&project_store)?;
    let mut registry = load_feature_registry(&project_store)?;
    let registry_changed = registry.register(FeatureRegistration::new(
        &request.feature_cache_relation,
        &request.features,
        current_time_millis(),
    ))?;
    let mut cache = load_feature_cache(&project_store, &request.feature_cache_relation, &registry)?;
    let (requested_indices, cache_schema_changed) = cache.merge_requested(&request.features)?;
    ensure_merged_cache_row_bound(&cache, &scan.keys, MAX_FEATURE_CACHE_ROWS)?;
    for key in &scan.keys {
        cache.ensure_key(key.clone());
    }
    let missing_cells = scan
        .keys
        .iter()
        .map(|key| {
            cache.missing_count(
                key,
                RequestedFeatures::new(&requested_indices, &request.features),
            )
        })
        .sum::<usize>();
    let requested_cells = scan
        .keys
        .iter()
        .map(|key| {
            request
                .features
                .iter()
                .filter(|definition| feature_applies(definition, key))
                .count()
        })
        .sum::<usize>();
    ensure_not_cancelled(&cancel)?;

    if missing_cells == 0 && !registry_changed && !cache_schema_changed {
        return Ok(FeatureCacheHit::new(request, scan)
            .with_counts(cache.rows.len(), requested_cells)
            .into_result());
    }

    if missing_cells == 0 {
        publish(ModelRunEvent::Materializing(
            "Publishing feature definitions and cache schema".to_owned(),
        ));
        FeaturePublication::new(&request.feature_cache_relation, &cache, &registry).publish(
            &project_store,
            PublicationGuard::new(&cancel, &publication_gate),
        )?;
        return Ok(ModelRunResult {
            rows: cache.rows.len(),
            requested_feature_cells: requested_cells as u64,
            computed_feature_cells: 0,
            reused_feature_cells: requested_cells as u64,
            quarantine: WorkflowQuarantineSummary::default(),
            metadata: WorkflowModelRunMetadata::new(
                WorkflowModelIdentity::new(
                    SIGLIP_BASE_MODEL_ID,
                    SIGLIP_BASE_REVISION,
                    request.feature_cache_relation,
                )
                .with_fingerprint(scan.fingerprint),
                WorkflowModelExecution::new(
                    request.features,
                    false,
                    "not loaded (empty applicable cohort)",
                ),
                WorkflowModelMetrics::new(scan.keys.len() as u64, cache.rows.len() as u64, 0),
            ),
            warnings: Vec::new(),
        });
    }

    publish(ModelRunEvent::Loading(format!(
        "Loading pinned {} artifacts",
        SIGLIP_BASE_MODEL_ID
    )));
    let artifacts = SiglipArtifacts::resolve()?;
    ensure_not_cancelled(&cancel)?;
    let concepts = request
        .features
        .iter()
        .map(WorkflowFeatureDefinition::concept)
        .collect::<Vec<_>>();
    let (engine, fallback_warning) = SiglipEngine::load_preferred(&artifacts, &concepts)?;
    ensure_not_cancelled(&cancel)?;
    let device_name = engine.device_name.clone();
    let chunk_rows = model_chunk_rows(engine.processor.image_size);
    publish(ModelRunEvent::Running {
        detail: format!("Backfilling {missing_cells} feature cells on {device_name}"),
        progress: WorkflowNodeProgress::new(0, missing_cells as u64, "feature cells"),
    });

    let (sender, receiver) = mpsc::sync_channel(INPUT_PREFETCH_CHUNKS);
    let producer_store = project_store.clone();
    let producer_sql = request.candidate_sql.clone();
    let producer_cancel = cancel.clone();
    let producer = thread::Builder::new()
        .name("workflow-model-candidates".to_owned())
        .spawn(move || {
            CandidateBatchProducer::new(&producer_sql, chunk_rows, &producer_cancel)
                .with_sender(sender)
                .produce(&producer_store)
        })
        .map_err(|error| format!("starting the Workflow candidate prefetch worker: {error}"))?;

    let mut seen = BTreeSet::new();
    let mut completed_cells = 0_u64;
    let mut rows_needing_features = 0_u64;
    let mut decode_ms = 0_u64;
    let mut inference_ms = 0_u64;
    let mut quarantine = WorkflowQuarantineSummary::default();
    let mut quarantined_keys = BTreeSet::new();
    let mut pipeline_error = None;
    for item in &receiver {
        if cancelled(&cancel) {
            pipeline_error = Some(MODEL_RUN_CANCELLED.to_owned());
            break;
        }
        let batch = match item {
            Ok(batch) => batch,
            Err(error) => {
                pipeline_error = Some(error);
                break;
            }
        };
        let model = BackfillModel::new(&engine, &requested_indices, &request.features);
        let cache_state = BackfillCache::new(&mut cache, &scan.keys, &mut seen);
        let progress = BackfillProgress::new(&mut completed_cells, &mut rows_needing_features)
            .with_timings(&mut decode_ms, &mut inference_ms);
        let quarantine_state = BackfillQuarantine::new(&mut quarantine, &mut quarantined_keys);
        if let Err(error) = CandidateBackfill::new(model, &batch, cache_state)
            .with_progress(progress)
            .with_quarantine(quarantine_state)
            .run()
        {
            pipeline_error = Some(error);
            break;
        }
        publish(ModelRunEvent::Running {
            detail: format!("Backfilled {completed_cells} of {missing_cells} feature cells"),
            progress: WorkflowNodeProgress::new(
                completed_cells.min(missing_cells as u64),
                missing_cells as u64,
                "feature cells",
            ),
        });
    }
    drop(receiver);
    let producer_result = producer
        .join()
        .map_err(|_| "Workflow candidate prefetch worker panicked.".to_owned())?;
    if pipeline_error.is_none() {
        pipeline_error = producer_result.err();
    }
    if cancelled(&cancel) {
        pipeline_error = Some(MODEL_RUN_CANCELLED.to_owned());
    }
    if pipeline_error.is_none() && seen != scan.keys {
        pipeline_error = Some(
            "Workflow candidate cohort changed between validation and feature backfill. Run again."
                .to_owned(),
        );
    }
    if quarantine.count().saturating_mul(2) > rows_needing_features {
        pipeline_error = Some(format!(
            "{} of {rows_needing_features} candidate rows needing features could not be decoded; more than half were quarantined.",
            quarantine.count()
        ));
    }
    if let Some(error) = pipeline_error {
        return Err(error.into());
    }
    ensure_not_cancelled(&cancel)?;
    publish(ModelRunEvent::Materializing(
        "Atomically replacing the complete feature cache".to_owned(),
    ));
    FeaturePublication::new(&request.feature_cache_relation, &cache, &registry).publish(
        &project_store,
        PublicationGuard::new(&cancel, &publication_gate),
    )?;
    project_store.invalidate_idle_connections();

    let mut warnings = Vec::new();
    if let Some(warning) = fallback_warning {
        warnings.push(warning);
    }
    if quarantine.count() > 0 {
        warnings.push(format!(
            "{} candidate rows were quarantined; their requested feature cells remain null.",
            quarantine.count()
        ));
    }
    let remaining_cells = scan
        .keys
        .iter()
        .map(|key| {
            cache.missing_count(
                key,
                RequestedFeatures::new(&requested_indices, &request.features),
            )
        })
        .sum::<usize>();
    Ok(ModelRunResult {
        rows: cache.rows.len(),
        requested_feature_cells: requested_cells as u64,
        computed_feature_cells: missing_cells.saturating_sub(remaining_cells) as u64,
        reused_feature_cells: requested_cells.saturating_sub(missing_cells) as u64,
        quarantine,
        metadata: WorkflowModelRunMetadata::new(
            WorkflowModelIdentity::new(
                SIGLIP_BASE_MODEL_ID,
                SIGLIP_BASE_REVISION,
                request.feature_cache_relation,
            )
            .with_fingerprint(scan.fingerprint),
            WorkflowModelExecution::new(request.features, false, device_name),
            WorkflowModelMetrics::new(scan.keys.len() as u64, cache.rows.len() as u64, decode_ms)
                .with_inference_ms(inference_ms),
        ),
        warnings,
    })
}

struct FeatureCacheHit {
    request: ModelRunRequest,
    scan: CandidateScan,
    cache_rows: usize,
    requested_cells: usize,
}

impl FeatureCacheHit {
    fn new(request: ModelRunRequest, scan: CandidateScan) -> Self {
        Self {
            request,
            scan,
            cache_rows: 0,
            requested_cells: 0,
        }
    }

    fn with_counts(mut self, cache_rows: usize, requested_cells: usize) -> Self {
        self.cache_rows = cache_rows;
        self.requested_cells = requested_cells;
        self
    }

    fn into_result(self) -> ModelRunResult {
        ModelRunResult {
            rows: self.cache_rows,
            requested_feature_cells: self.requested_cells as u64,
            computed_feature_cells: 0,
            reused_feature_cells: self.requested_cells as u64,
            quarantine: WorkflowQuarantineSummary::default(),
            metadata: WorkflowModelRunMetadata::new(
                WorkflowModelIdentity::new(
                    SIGLIP_BASE_MODEL_ID,
                    SIGLIP_BASE_REVISION,
                    self.request.feature_cache_relation,
                )
                .with_fingerprint(self.scan.fingerprint),
                WorkflowModelExecution::new(
                    self.request.features,
                    true,
                    "not loaded (feature cache hit)",
                ),
                WorkflowModelMetrics::new(self.scan.keys.len() as u64, self.cache_rows as u64, 0),
            ),
            warnings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FrameKey {
    episode_id: String,
    stream_id: String,
    segment_index: u32,
    frame_index: u64,
    episode_time_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheFeature {
    name: String,
    definition: WorkflowFeatureDefinition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FeatureRegistryEntry {
    definition: WorkflowFeatureDefinition,
    cache_relation: String,
    producer: FeatureProducer,
    timestamps: FeatureRegistryTimestamps,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FeatureProducer {
    model_id: String,
    repository: String,
    artifact_revision: String,
    value_semantics: String,
    definition_version: u32,
}

impl FeatureProducer {
    fn siglip() -> Self {
        Self {
            model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
            repository: SIGLIP_BASE_REPOSITORY.to_owned(),
            artifact_revision: SIGLIP_BASE_REVISION.to_owned(),
            value_semantics: FEATURE_VALUE_SEMANTICS.to_owned(),
            definition_version: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FeatureRegistryTimestamps {
    created_at_ms: u64,
    updated_at_ms: u64,
}

impl FeatureRegistryTimestamps {
    fn new(created_at_ms: u64, updated_at_ms: u64) -> Self {
        Self {
            created_at_ms,
            updated_at_ms,
        }
    }
}

impl FeatureRegistryEntry {
    fn new(
        definition: WorkflowFeatureDefinition,
        cache_relation: impl Into<String>,
        now_ms: u64,
    ) -> Self {
        Self {
            definition,
            cache_relation: cache_relation.into(),
            producer: FeatureProducer::siglip(),
            timestamps: FeatureRegistryTimestamps::new(now_ms, now_ms),
        }
    }
}

#[derive(Clone, Default)]
struct FeatureRegistry {
    entries: BTreeMap<(String, String), FeatureRegistryEntry>,
}

impl FeatureRegistry {
    fn register(&mut self, registration: FeatureRegistration<'_>) -> Result<bool, String> {
        let FeatureRegistration {
            cache_relation,
            requested,
            now_ms,
        } = registration;
        let mut changed = false;
        for definition in requested {
            let key = (
                definition.applicability().source_relation().to_owned(),
                definition.name().to_owned(),
            );
            if let Some(existing) = self.entries.get(&key) {
                if existing.definition != *definition
                    || existing.cache_relation != cache_relation
                    || existing.producer.model_id != SIGLIP_BASE_MODEL_ID
                    || existing.producer.repository != SIGLIP_BASE_REPOSITORY
                    || existing.producer.artifact_revision != SIGLIP_BASE_REVISION
                    || existing.producer.value_semantics != FEATURE_VALUE_SEMANTICS
                {
                    return Err(format!(
                        "Feature {:?} already exists for source {:?} with an incompatible definition. Reuse its exact registry definition or choose a new descriptive feature name.",
                        definition.name(),
                        definition.applicability().source_relation()
                    ));
                }
                continue;
            }
            if self.entries.len() >= MAX_FEATURE_REGISTRY_ROWS {
                return Err(format!(
                    "Workflow feature registry exceeds the bounded limit of {MAX_FEATURE_REGISTRY_ROWS} definitions."
                ));
            }
            let entry = FeatureRegistryEntry::new(definition.clone(), cache_relation, now_ms);
            validate_feature_registry_entry(&entry)?;
            self.entries.insert(key, entry);
            changed = true;
        }
        Ok(changed)
    }

    fn cache_features(&self, cache_relation: &str) -> Vec<CacheFeature> {
        self.entries
            .values()
            .filter(|entry| entry.cache_relation == cache_relation)
            .map(|entry| CacheFeature {
                name: entry.definition.name().to_owned(),
                definition: entry.definition.clone(),
            })
            .collect()
    }
}

struct FeatureRegistration<'a> {
    cache_relation: &'a str,
    requested: &'a [WorkflowFeatureDefinition],
    now_ms: u64,
}

impl<'a> FeatureRegistration<'a> {
    fn new(
        cache_relation: &'a str,
        requested: &'a [WorkflowFeatureDefinition],
        now_ms: u64,
    ) -> Self {
        Self {
            cache_relation,
            requested,
            now_ms,
        }
    }
}

#[derive(Default)]
struct FeatureCache {
    features: Vec<CacheFeature>,
    rows: BTreeMap<FrameKey, Vec<Option<f32>>>,
}

impl FeatureCache {
    fn merge_requested(
        &mut self,
        requested: &[WorkflowFeatureDefinition],
    ) -> Result<(Vec<usize>, bool), String> {
        let additional = requested
            .iter()
            .filter(|definition| {
                !self
                    .features
                    .iter()
                    .any(|feature| feature.name == definition.name())
            })
            .count();
        if self.features.len().saturating_add(additional) > MAX_FEATURE_CACHE_COLUMNS {
            return Err(format!(
                "Workflow feature cache supports at most {MAX_FEATURE_CACHE_COLUMNS} feature columns."
            ));
        }
        let mut indices = Vec::with_capacity(requested.len());
        let mut changed = false;
        for mapping in requested {
            if let Some((index, existing)) = self
                .features
                .iter()
                .enumerate()
                .find(|(_, feature)| feature.name == mapping.name())
            {
                if existing.definition != *mapping {
                    return Err(format!(
                        "Feature cache field {:?} has an incompatible registered definition.",
                        mapping.name()
                    ));
                }
                indices.push(index);
                continue;
            }
            let index = self.features.len();
            self.features.push(CacheFeature {
                name: mapping.name().to_owned(),
                definition: mapping.clone(),
            });
            for values in self.rows.values_mut() {
                values.push(None);
            }
            indices.push(index);
            changed = true;
        }
        Ok((indices, changed))
    }

    fn ensure_key(&mut self, key: FrameKey) {
        let feature_count = self.features.len();
        self.rows
            .entry(key)
            .or_insert_with(|| vec![None; feature_count]);
    }

    fn missing_indices(
        &self,
        key: &FrameKey,
        requested: RequestedFeatures<'_>,
    ) -> Vec<(usize, usize)> {
        let values = self.rows.get(key).expect("candidate key was inserted");
        requested
            .indices
            .iter()
            .copied()
            .enumerate()
            .filter(|(request_index, cache_index)| {
                feature_applies(&requested.definitions[*request_index], key)
                    && values[*cache_index].is_none()
            })
            .collect()
    }

    fn missing_count(&self, key: &FrameKey, requested: RequestedFeatures<'_>) -> usize {
        self.missing_indices(key, requested).len()
    }
}

#[derive(Clone, Copy)]
struct RequestedFeatures<'a> {
    indices: &'a [usize],
    definitions: &'a [WorkflowFeatureDefinition],
}

impl<'a> RequestedFeatures<'a> {
    fn new(indices: &'a [usize], definitions: &'a [WorkflowFeatureDefinition]) -> Self {
        Self {
            indices,
            definitions,
        }
    }
}

fn ensure_merged_cache_row_bound(
    cache: &FeatureCache,
    candidates: &BTreeSet<FrameKey>,
    limit: usize,
) -> Result<(), String> {
    let additional = candidates
        .iter()
        .filter(|key| !cache.rows.contains_key(*key))
        .count();
    if cache.rows.len().saturating_add(additional) > limit {
        return Err(format!(
            "Workflow feature cache would exceed the bounded limit of {limit} rows after merging this candidate cohort. Narrow the candidate Query."
        ));
    }
    Ok(())
}

fn feature_applies(definition: &WorkflowFeatureDefinition, key: &FrameKey) -> bool {
    definition.applicability().stream_ids().is_empty()
        || definition
            .applicability()
            .stream_ids()
            .iter()
            .any(|stream| stream == &key.stream_id)
}

struct CandidateScan {
    keys: BTreeSet<FrameKey>,
    fingerprint: String,
}

struct CandidateScanRequest<'a> {
    sql: &'a str,
    features: &'a [WorkflowFeatureDefinition],
    cancel: &'a AtomicBool,
}

impl<'a> CandidateScanRequest<'a> {
    fn new(
        sql: &'a str,
        features: &'a [WorkflowFeatureDefinition],
        cancel: &'a AtomicBool,
    ) -> Self {
        Self {
            sql,
            features,
            cancel,
        }
    }

    fn scan(self, project_store: &ProjectStore) -> Result<CandidateScan, String> {
        let sql = self.sql.to_owned();
        let media_fields = self
            .features
            .iter()
            .map(|feature| feature.applicability().media_field().to_owned())
            .collect::<BTreeSet<_>>();
        let mut keys = project_store.run_blocking(|connection| {
        let mut reader = connection.execute_query(&sql)?;
        validate_candidate_schema(reader.schema().as_ref(), &media_fields)?;
        let mut keys = BTreeSet::new();
        for batch in &mut reader {
            ensure_not_cancelled(self.cancel)?;
            let batch = batch.map_err(|error| format!("reading Workflow candidates: {error}"))?;
            for row in 0..batch.num_rows() {
                let key = frame_key(&batch, row)?;
                if !keys.insert(key) {
                    return Err(
                        "Workflow candidate SQL returned a duplicate stable frame identity. Make the cohort unique before Model enrichment."
                            .to_owned(),
                    );
                }
                if keys.len() > MAX_FEATURE_CACHE_ROWS {
                    return Err(format!(
                        "Workflow candidate cohort exceeds the bounded limit of {MAX_FEATURE_CACHE_ROWS} rows. Narrow the SQL cohort."
                    ));
                }
            }
        }
        Ok(keys)
    })?;
        let mut digest = Sha256::new();
        digest.update(b"spiral-workflow-feature-cache\0v1");
        for definition in self.features {
            fingerprint_field(&mut digest, b"feature_name", definition.name().as_bytes());
            fingerprint_field(&mut digest, b"concept", definition.concept().as_bytes());
            fingerprint_field(
                &mut digest,
                b"definition",
                feature_definition_fingerprint(definition).as_bytes(),
            );
        }
        for key in &keys {
            fingerprint_frame_key(&mut digest, key);
        }
        Ok(CandidateScan {
            keys: std::mem::take(&mut keys),
            fingerprint: digest_hex(digest.finalize().as_slice()),
        })
    }
}

fn validate_candidate_schema(
    schema: &Schema,
    media_fields: &BTreeSet<String>,
) -> Result<(), String> {
    for (name, expected) in &CACHE_KEY_FIELDS {
        let field = schema.field_with_name(name).map_err(|_| {
            format!("Workflow candidate SQL must project stable identity field {name:?}.")
        })?;
        // Spiral conservatively marks values projected from table-generating
        // functions as nullable even after an explicit non-null filter. Accept
        // that declaration here; `frame_key` still rejects every null value
        // before the identity can enter the cache or its fingerprint.
        if !storage_type_matches(field.data_type(), expected) {
            return Err(format!(
                "Workflow candidate field {name:?} must use {expected:?} storage."
            ));
        }
    }
    for media_field in media_fields {
        let media = schema
            .field_with_name(media_field)
            .map_err(|_| format!("Workflow candidate SQL has no image field {media_field:?}."))?;
        if media
            .metadata()
            .get(EXTENSION_TYPE_NAME_KEY)
            .map(String::as_str)
            != Some(IMAGE_EXTENSION_NAME)
        {
            return Err(format!(
                "Workflow candidate field {media_field:?} must be a decoded spql.image.Image. Author frame selection and decode in the upstream Query."
            ));
        }
    }
    Ok(())
}

fn frame_key(batch: &RecordBatch, row: usize) -> Result<FrameKey, String> {
    let text = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|column| string_value(column.as_ref(), row))
            .map(str::to_owned)
            .ok_or_else(|| format!("Workflow candidate field {name:?} is null or not UTF-8."))
    };
    let integer = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|column| u64_value(column.as_ref(), row))
            .ok_or_else(|| {
                format!("Workflow candidate field {name:?} is null or not an unsigned integer.")
            })
    };
    Ok(FrameKey {
        episode_id: text("episode_id")?,
        stream_id: text("stream_id")?,
        segment_index: u32::try_from(integer("segment_index")?)
            .map_err(|_| "Workflow candidate segment_index overflowed UInt32.".to_owned())?,
        frame_index: integer("frame_index")?,
        episode_time_ns: integer("episode_time_ns")?,
    })
}

fn fingerprint_frame_key(digest: &mut Sha256, key: &FrameKey) {
    fingerprint_field(digest, b"episode_id", key.episode_id.as_bytes());
    fingerprint_field(digest, b"stream_id", key.stream_id.as_bytes());
    fingerprint_field(digest, b"segment_index", &key.segment_index.to_le_bytes());
    fingerprint_field(digest, b"frame_index", &key.frame_index.to_le_bytes());
    fingerprint_field(
        digest,
        b"episode_time_ns",
        &key.episode_time_ns.to_le_bytes(),
    );
}

fn feature_definition_fingerprint(definition: &WorkflowFeatureDefinition) -> String {
    let mut digest = Sha256::new();
    digest.update(b"spiral-workflow-feature-definition\0v1");
    fingerprint_field(&mut digest, b"name", definition.name().as_bytes());
    fingerprint_field(
        &mut digest,
        b"description",
        definition.description().as_bytes(),
    );
    fingerprint_field(&mut digest, b"concept", definition.concept().as_bytes());
    fingerprint_field(
        &mut digest,
        b"source_relation",
        definition.applicability().source_relation().as_bytes(),
    );
    fingerprint_field(
        &mut digest,
        b"media_field",
        definition.applicability().media_field().as_bytes(),
    );
    for stream in definition.applicability().stream_ids() {
        fingerprint_field(&mut digest, b"stream_id", stream.as_bytes());
    }
    digest_hex(digest.finalize().as_slice())
}

fn load_feature_registry(project_store: &ProjectStore) -> Result<FeatureRegistry, String> {
    project_store.run_blocking(|connection| {
        let Some(schema) = connection.table_schema(FEATURE_REGISTRY_RELATION)? else {
            return Ok(FeatureRegistry::default());
        };
        validate_feature_registry_schema(&schema)?;
        let mut reader =
            connection.execute_query(format!("SELECT * FROM {FEATURE_REGISTRY_RELATION}"))?;
        let mut registry = FeatureRegistry::default();
        for batch in &mut reader {
            let batch =
                batch.map_err(|error| format!("reading Workflow feature registry: {error}"))?;
            for row in 0..batch.num_rows() {
                let required_text = |name: &str| {
                    batch
                        .column_by_name(name)
                        .and_then(|column| string_value(column.as_ref(), row))
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            format!("Workflow feature registry field {name:?} is null or invalid.")
                        })
                };
                let source_relation = required_text("source_relation")?;
                let feature_name = required_text("feature_name")?;
                let entry = FeatureRegistryEntry {
                    definition: WorkflowFeatureDefinition::new(
                        feature_name.clone(),
                        required_text("description")?,
                        required_text("concept")?,
                    )
                    .with_applicability(
                        spiral_robotics_api::workflow::WorkflowFeatureApplicability::new(
                            source_relation.clone(),
                            required_text("media_field")?,
                        )
                        .with_stream_ids(registry_stream_ids(&batch, row)?),
                    ),
                    cache_relation: required_text("cache_relation")?,
                    producer: FeatureProducer {
                        model_id: required_text("producer_model_id")?,
                        repository: required_text("producer_repository")?,
                        artifact_revision: required_text("artifact_revision")?,
                        value_semantics: required_text("value_semantics")?,
                        definition_version: u32::try_from(
                            batch
                                .column_by_name("definition_version")
                                .and_then(|column| u64_value(column.as_ref(), row))
                                .ok_or_else(|| {
                                    "Workflow feature registry definition_version is invalid."
                                        .to_owned()
                                })?,
                        )
                        .map_err(|_| {
                            "Workflow feature registry definition_version overflowed UInt32."
                                .to_owned()
                        })?,
                    },
                    timestamps: FeatureRegistryTimestamps::new(
                        batch
                            .column_by_name("created_at_ms")
                            .and_then(|column| u64_value(column.as_ref(), row))
                            .ok_or_else(|| {
                                "Workflow feature registry created_at_ms is invalid.".to_owned()
                            })?,
                        batch
                            .column_by_name("updated_at_ms")
                            .and_then(|column| u64_value(column.as_ref(), row))
                            .ok_or_else(|| {
                                "Workflow feature registry updated_at_ms is invalid.".to_owned()
                            })?,
                    ),
                };
                LoadedFeatureRegistryEntry::new(source_relation, feature_name, entry)
                    .insert_into(&mut registry)?;
            }
        }
        Ok(registry)
    })
}

struct LoadedFeatureRegistryEntry {
    source_relation: String,
    feature_name: String,
    entry: FeatureRegistryEntry,
}

impl LoadedFeatureRegistryEntry {
    fn new(source_relation: String, feature_name: String, entry: FeatureRegistryEntry) -> Self {
        Self {
            source_relation,
            feature_name,
            entry,
        }
    }

    fn insert_into(self, registry: &mut FeatureRegistry) -> Result<(), String> {
        validate_feature_registry_entry(&self.entry)?;
        if registry
            .entries
            .contains_key(&(self.source_relation.clone(), self.feature_name.clone()))
        {
            return Err(
                "Workflow feature registry contains a duplicate source/name definition.".to_owned(),
            );
        }
        if registry.entries.len() >= MAX_FEATURE_REGISTRY_ROWS {
            return Err(format!(
                "Workflow feature registry exceeds the bounded limit of {MAX_FEATURE_REGISTRY_ROWS} definitions."
            ));
        }
        registry
            .entries
            .insert((self.source_relation, self.feature_name), self.entry);
        Ok(())
    }
}

fn validate_feature_registry_entry(entry: &FeatureRegistryEntry) -> Result<(), String> {
    QualifiedRelation::try_from(entry.definition.applicability().source_relation())
        .map_err(|error| format!("Workflow feature registry source relation is unsafe: {error}"))?;
    QualifiedRelation::try_from(entry.cache_relation.as_str())
        .map_err(|error| format!("Workflow feature registry cache relation is unsafe: {error}"))?;
    if entry.cache_relation
        != derived_feature_cache_relation(entry.definition.applicability().source_relation())
    {
        return Err(
            "Workflow feature registry cache relation does not match its server-derived source identity."
                .to_owned(),
        );
    }
    if !entry.definition.has_valid_name()
        || entry.definition.description().trim().is_empty()
        || entry.definition.description().chars().count() > MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH
        || entry.definition.concept().trim().is_empty()
        || entry.definition.concept().chars().count() > MAX_SIGLIP_CONCEPT_LENGTH
        || entry.definition.applicability().stream_ids().len() > MAX_WORKFLOW_FEATURE_STREAMS
        || UnquotedSqlIdentifier::try_from(entry.definition.applicability().media_field()).is_err()
    {
        return Err("Workflow feature registry contains an invalid feature definition.".to_owned());
    }
    let streams = entry
        .definition
        .applicability()
        .stream_ids()
        .iter()
        .collect::<BTreeSet<_>>();
    if streams.len() != entry.definition.applicability().stream_ids().len()
        || streams
            .iter()
            .any(|stream| stream.trim().is_empty() || stream.len() > 128)
    {
        return Err(
            "Workflow feature registry contains invalid applicability stream IDs.".to_owned(),
        );
    }
    if entry.producer.model_id != SIGLIP_BASE_MODEL_ID
        || entry.producer.repository != SIGLIP_BASE_REPOSITORY
        || entry.producer.artifact_revision != SIGLIP_BASE_REVISION
        || entry.producer.value_semantics != FEATURE_VALUE_SEMANTICS
        || entry.producer.definition_version != 1
        || entry.timestamps.updated_at_ms < entry.timestamps.created_at_ms
    {
        return Err(
            "Workflow feature registry contains incompatible producer provenance.".to_owned(),
        );
    }
    Ok(())
}

fn feature_registry_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
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
    ]))
}

fn validate_feature_registry_schema(schema: &Schema) -> Result<(), String> {
    let expected = feature_registry_schema();
    if schema.fields().len() != expected.fields().len()
        || schema
            .fields()
            .iter()
            .zip(expected.fields())
            .any(|(actual, expected)| {
                actual.name() != expected.name()
                    || actual.is_nullable() != expected.is_nullable()
                    || !storage_type_matches(actual.data_type(), expected.data_type())
            })
    {
        return Err(format!(
            "Workflow feature registry has an incompatible schema. Studio will not replace it. Expected {expected:?}; actual {schema:?}."
        ));
    }
    Ok(())
}

fn registry_stream_ids(batch: &RecordBatch, row: usize) -> Result<Vec<String>, String> {
    let values = batch
        .column_by_name("stream_ids")
        .and_then(|column| column.as_any().downcast_ref::<ListArray>())
        .ok_or_else(|| "Workflow feature registry stream_ids is not a List<Utf8>.".to_owned())?;
    if values.is_null(row) {
        return Err("Workflow feature registry stream_ids is null.".to_owned());
    }
    let values = values.value(row);
    (0..values.len())
        .map(|index| {
            string_value(values.as_ref(), index)
                .map(str::to_owned)
                .ok_or_else(|| {
                    "Workflow feature registry contains a null or invalid stream ID.".to_owned()
                })
        })
        .collect()
}

fn feature_registry_batch(registry: &FeatureRegistry) -> Result<RecordBatch, String> {
    let entries = registry.entries.values().collect::<Vec<_>>();
    let text = |value: fn(&FeatureRegistryEntry) -> &str| {
        Arc::new(StringArray::from_iter_values(
            entries.iter().map(|entry| value(entry)),
        )) as ArrayRef
    };
    let mut streams = ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Utf8,
        false,
    )));
    for entry in &entries {
        for stream in entry.definition.applicability().stream_ids() {
            streams.values().append_value(stream);
        }
        streams.append(true);
    }
    RecordBatch::try_new(
        feature_registry_schema(),
        vec![
            text(|entry| entry.definition.applicability().source_relation()),
            text(|entry| &entry.cache_relation),
            text(|entry| entry.definition.name()),
            text(|entry| entry.definition.description()),
            text(|entry| entry.definition.concept()),
            text(|entry| entry.definition.applicability().media_field()),
            Arc::new(streams.finish()) as ArrayRef,
            text(|entry| &entry.producer.model_id),
            text(|entry| &entry.producer.repository),
            text(|entry| &entry.producer.artifact_revision),
            text(|entry| &entry.producer.value_semantics),
            Arc::new(UInt32Array::from_iter_values(
                entries
                    .iter()
                    .map(|entry| entry.producer.definition_version),
            )) as ArrayRef,
            Arc::new(UInt64Array::from_iter_values(
                entries.iter().map(|entry| entry.timestamps.created_at_ms),
            )) as ArrayRef,
            Arc::new(UInt64Array::from_iter_values(
                entries.iter().map(|entry| entry.timestamps.updated_at_ms),
            )) as ArrayRef,
        ],
    )
    .map_err(|error| format!("building Workflow feature registry: {error}"))
}

fn load_feature_cache(
    project_store: &ProjectStore,
    relation: &str,
    registry: &FeatureRegistry,
) -> Result<FeatureCache, String> {
    let relation = relation.to_owned();
    let registered = registry.cache_features(&relation);
    project_store.run_blocking(move |connection| {
        let Some(schema) = connection.table_schema(&relation)? else {
            return Ok(FeatureCache::default());
        };
        let features = validate_cache_schema(&schema, &registered)?;
        let mut reader = connection.execute_query(format!("SELECT * FROM {relation}"))?;
        let mut rows = BTreeMap::new();
        for batch in &mut reader {
            let batch = batch.map_err(|error| format!("reading Workflow feature cache: {error}"))?;
            for row in 0..batch.num_rows() {
                let key = frame_key(&batch, row)?;
                let values = features
                    .iter()
                    .map(|feature| {
                        let column = batch.column_by_name(&feature.name).ok_or_else(|| {
                            format!("Workflow feature cache lost field {:?}.", feature.name)
                        })?;
                        if column.is_null(row) {
                            Ok(None)
                        } else {
                            column
                                .as_any()
                                .downcast_ref::<Float32Array>()
                                .map(|values| Some(values.value(row)))
                                .ok_or_else(|| {
                                    format!(
                                        "Workflow feature cache field {:?} is not Float32.",
                                        feature.name
                                    )
                                })
                        }
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                if rows.insert(key, values).is_some() {
                    return Err(
                        "Workflow feature cache contains duplicate stable frame identities. Delete the owned cache and run again."
                            .to_owned(),
                    );
                }
                if rows.len() > MAX_FEATURE_CACHE_ROWS {
                    return Err(format!(
                        "Workflow feature cache exceeds the bounded limit of {MAX_FEATURE_CACHE_ROWS} rows."
                    ));
                }
            }
        }
        Ok(FeatureCache { features, rows })
    })
}

fn validate_cache_schema(
    schema: &Schema,
    registered: &[CacheFeature],
) -> Result<Vec<CacheFeature>, String> {
    if schema.fields().len() < CACHE_KEY_FIELDS.len() {
        return Err("Workflow feature cache is missing stable identity fields.".to_owned());
    }
    for (index, (name, expected)) in CACHE_KEY_FIELDS.iter().enumerate() {
        let field = schema.field(index);
        if field.name() != name
            || (field.is_nullable() && !cache_key_declaration_may_be_nullable(name))
            || !storage_type_matches(field.data_type(), expected)
        {
            return Err(format!(
                "Workflow feature cache key field {index} must use compatible {name} {expected:?} storage."
            ));
        }
    }
    if schema.fields().len() - CACHE_KEY_FIELDS.len() > MAX_FEATURE_CACHE_COLUMNS {
        return Err(format!(
            "Workflow feature cache exceeds {MAX_FEATURE_CACHE_COLUMNS} feature columns."
        ));
    }
    schema
        .fields()
        .iter()
        .skip(CACHE_KEY_FIELDS.len())
        .map(|field| {
            if field.data_type() != &DataType::Float32 || !field.is_nullable() {
                return Err(format!(
                    "Workflow feature cache field {:?} must be nullable Float32.",
                    field.name()
                ));
            }
            let registered = registered
                .iter()
                .find(|feature| feature.name == field.name().as_str())
                .ok_or_else(|| {
                    format!(
                        "Workflow feature cache field {:?} has no registry definition.",
                        field.name()
                    )
                })?;
            Ok(CacheFeature {
                name: field.name().clone(),
                definition: registered.definition.clone(),
            })
        })
        .collect()
}

struct CandidateBatchProducer<'a> {
    sql: &'a str,
    chunk_rows: usize,
    cancel: &'a AtomicBool,
    sender: Option<SyncSender<Result<RecordBatch, String>>>,
}

impl<'a> CandidateBatchProducer<'a> {
    fn new(sql: &'a str, chunk_rows: usize, cancel: &'a AtomicBool) -> Self {
        Self {
            sql,
            chunk_rows,
            cancel,
            sender: None,
        }
    }

    fn with_sender(mut self, sender: SyncSender<Result<RecordBatch, String>>) -> Self {
        self.sender = Some(sender);
        self
    }

    fn produce(self, project_store: &ProjectStore) -> Result<(), String> {
        let sql = self.sql.to_owned();
        let sender = self.sender.expect("candidate batch sender is configured");
        project_store.run_blocking(move |connection| {
            let mut reader = connection.execute_query(sql)?;
            for batch in &mut reader {
                if cancelled(self.cancel) {
                    return Ok(());
                }
                let batch =
                    batch.map_err(|error| format!("reading Workflow candidates: {error}"))?;
                let mut offset = 0;
                while offset < batch.num_rows() {
                    let rows = self.chunk_rows.min(batch.num_rows() - offset);
                    let compact = compact_batch_rows(&batch, offset, rows)?;
                    if sender.send(Ok(compact)).is_err() {
                        return Ok(());
                    }
                    offset += rows;
                }
            }
            Ok(())
        })
    }
}

struct BackfillModel<'a> {
    engine: &'a SiglipEngine,
    requested: RequestedFeatures<'a>,
}

impl<'a> BackfillModel<'a> {
    fn new(
        engine: &'a SiglipEngine,
        requested_indices: &'a [usize],
        definitions: &'a [WorkflowFeatureDefinition],
    ) -> Self {
        Self {
            engine,
            requested: RequestedFeatures::new(requested_indices, definitions),
        }
    }
}

struct BackfillCache<'a> {
    cache: &'a mut FeatureCache,
    expected: &'a BTreeSet<FrameKey>,
    seen: &'a mut BTreeSet<FrameKey>,
}

impl<'a> BackfillCache<'a> {
    fn new(
        cache: &'a mut FeatureCache,
        expected: &'a BTreeSet<FrameKey>,
        seen: &'a mut BTreeSet<FrameKey>,
    ) -> Self {
        Self {
            cache,
            expected,
            seen,
        }
    }
}

struct BackfillProgress<'a> {
    completed_cells: &'a mut u64,
    rows_needing_features: &'a mut u64,
    decode_ms: Option<&'a mut u64>,
    inference_ms: Option<&'a mut u64>,
}

impl<'a> BackfillProgress<'a> {
    fn new(completed_cells: &'a mut u64, rows_needing_features: &'a mut u64) -> Self {
        Self {
            completed_cells,
            rows_needing_features,
            decode_ms: None,
            inference_ms: None,
        }
    }

    fn with_timings(mut self, decode_ms: &'a mut u64, inference_ms: &'a mut u64) -> Self {
        self.decode_ms = Some(decode_ms);
        self.inference_ms = Some(inference_ms);
        self
    }
}

struct BackfillQuarantine<'a> {
    summary: &'a mut WorkflowQuarantineSummary,
    keys: &'a mut BTreeSet<FrameKey>,
}

impl<'a> BackfillQuarantine<'a> {
    fn new(summary: &'a mut WorkflowQuarantineSummary, keys: &'a mut BTreeSet<FrameKey>) -> Self {
        Self { summary, keys }
    }
}

struct CandidateBackfill<'a> {
    model: BackfillModel<'a>,
    batch: &'a RecordBatch,
    cache: BackfillCache<'a>,
    progress: Option<BackfillProgress<'a>>,
    quarantine: Option<BackfillQuarantine<'a>>,
}

impl<'a> CandidateBackfill<'a> {
    fn new(model: BackfillModel<'a>, batch: &'a RecordBatch, cache: BackfillCache<'a>) -> Self {
        Self {
            model,
            batch,
            cache,
            progress: None,
            quarantine: None,
        }
    }

    fn with_progress(mut self, progress: BackfillProgress<'a>) -> Self {
        self.progress = Some(progress);
        self
    }

    fn with_quarantine(mut self, quarantine: BackfillQuarantine<'a>) -> Self {
        self.quarantine = Some(quarantine);
        self
    }

    fn run(self) -> Result<(), String> {
        let Self {
            model: BackfillModel { engine, requested },
            batch,
            cache:
                BackfillCache {
                    cache,
                    expected,
                    seen,
                },
            progress,
            quarantine,
        } = self;
        let RequestedFeatures {
            indices: requested_indices,
            definitions,
        } = requested;
        let BackfillProgress {
            completed_cells,
            rows_needing_features,
            decode_ms,
            inference_ms,
        } = progress.expect("backfill progress is configured");
        let decode_ms = decode_ms.expect("backfill decode timing is configured");
        let inference_ms = inference_ms.expect("backfill inference timing is configured");
        let BackfillQuarantine {
            summary: quarantine,
            keys: quarantined_keys,
        } = quarantine.expect("backfill quarantine is configured");
        let media_fields = definitions
            .iter()
            .map(|definition| definition.applicability().media_field().to_owned())
            .collect::<BTreeSet<_>>();
        validate_candidate_schema(batch.schema().as_ref(), &media_fields)?;
        type MissingFeatureGroups = BTreeMap<(String, Vec<(usize, usize)>), Vec<(usize, FrameKey)>>;
        let mut groups = MissingFeatureGroups::new();
        for row in 0..batch.num_rows() {
            let key = frame_key(batch, row)?;
            if !expected.contains(&key) || !seen.insert(key.clone()) {
                return Err(
                "Workflow candidate cohort changed between validation and feature backfill. Run again."
                    .to_owned(),
            );
            }
            let missing =
                cache.missing_indices(&key, RequestedFeatures::new(requested_indices, definitions));
            if !missing.is_empty() {
                *rows_needing_features = rows_needing_features.saturating_add(1);
                let mut by_media = BTreeMap::<String, Vec<(usize, usize)>>::new();
                for pair @ (request_index, _) in missing {
                    by_media
                        .entry(
                            definitions[request_index]
                                .applicability()
                                .media_field()
                                .to_owned(),
                        )
                        .or_default()
                        .push(pair);
                }
                for (media_field, pairs) in by_media {
                    groups
                        .entry((media_field, pairs))
                        .or_default()
                        .push((row, key.clone()));
                }
            }
        }
        for ((media_field, feature_indices), rows) in groups {
            let media = batch
                .column_by_name(&media_field)
                .and_then(|column| column.as_any().downcast_ref::<StructArray>())
                .ok_or_else(|| {
                    format!(
                        "Workflow candidate image field {media_field:?} did not use struct storage."
                    )
                })?;
            let decode_started = Instant::now();
            let mut pixels = Vec::new();
            let mut valid_keys = Vec::new();
            for (row, key) in rows {
                match decode_image_row(media, row, &engine.processor) {
                    Ok(image) => {
                        pixels.extend(image);
                        valid_keys.push(key);
                    }
                    Err(error) => {
                        if quarantined_keys.insert(key.clone()) {
                            quarantine.set_count(quarantine.count().saturating_add(1));
                            if quarantine.examples().len() < MAX_QUARANTINE_EXAMPLES {
                                quarantine.examples_mut().push(format!(
                                    "{}/{}/{}: {error}",
                                    key.episode_id, key.stream_id, key.frame_index
                                ));
                            }
                        }
                        *completed_cells =
                            completed_cells.saturating_add(feature_indices.len() as u64);
                    }
                }
            }
            *decode_ms = decode_ms.saturating_add(elapsed_millis(decode_started));
            if valid_keys.is_empty() {
                continue;
            }
            let inference_started = Instant::now();
            let engine_indices = feature_indices
                .iter()
                .map(|(request_index, _)| *request_index)
                .collect::<Vec<_>>();
            let scores = engine.score_selected(SelectedImageBatch::new(
                &pixels,
                valid_keys.len(),
                &engine_indices,
            ))?;
            *inference_ms = inference_ms.saturating_add(elapsed_millis(inference_started));
            for (key, values) in valid_keys.into_iter().zip(scores) {
                let row = cache
                    .rows
                    .get_mut(&key)
                    .expect("candidate key was inserted");
                for (value, (_, cache_index)) in values.into_iter().zip(&feature_indices) {
                    row[*cache_index] = Some(value);
                }
                *completed_cells = completed_cells.saturating_add(feature_indices.len() as u64);
            }
        }
        Ok(())
    }
}

struct FeaturePublication<'a> {
    relation: &'a str,
    cache: &'a FeatureCache,
    registry: &'a FeatureRegistry,
}

impl<'a> FeaturePublication<'a> {
    fn new(relation: &'a str, cache: &'a FeatureCache, registry: &'a FeatureRegistry) -> Self {
        Self {
            relation,
            cache,
            registry,
        }
    }

    fn replace(self, project_store: &ProjectStore) -> Result<(), String> {
        let relation = self.relation.to_owned();
        let batches = feature_cache_batches(self.cache)?;
        let registry_batch = feature_registry_batch(self.registry)?;
        project_store.run_exclusive_blocking(move |connection| {
            let namespace = relation
                .split_once('.')
                .map(|(namespace, _)| namespace)
                .ok_or_else(|| "Workflow feature cache relation was not qualified.".to_owned())?;
            connection.ensure_dataset_namespace(namespace)?;
            run_feature_publication_transaction(connection, |connection| {
                connection.replace_batches(&relation, batches)?;
                connection.replace_batches(FEATURE_REGISTRY_RELATION, vec![registry_batch])
            })
        })
    }

    fn publish(
        self,
        project_store: &ProjectStore,
        guard: PublicationGuard<'_>,
    ) -> Result<(), String> {
        let PublicationGuard {
            cancel,
            publication_gate,
        } = guard;
        // Cancellation takes this same gate before setting the token. Whichever
        // side acquires it first owns the publication boundary: cancellation that
        // wins prevents the transaction, while a completed transaction remains a
        // truthful predecessor of any later cancellation request.
        let _publication = publication_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_not_cancelled(cancel)?;
        self.replace(project_store)
    }
}

struct PublicationGuard<'a> {
    cancel: &'a AtomicBool,
    publication_gate: &'a Mutex<()>,
}

impl<'a> PublicationGuard<'a> {
    fn new(cancel: &'a AtomicBool, publication_gate: &'a Mutex<()>) -> Self {
        Self {
            cancel,
            publication_gate,
        }
    }
}

trait FeaturePublicationTransaction {
    fn begin(&mut self) -> Result<(), String>;
    fn commit_publication(&mut self) -> Result<(), String>;
    fn rollback_publication(&mut self) -> Result<(), String>;
    fn restore_autocommit(&mut self) -> Result<(), String>;
    fn discard_publication_connection(&mut self);
}

impl FeaturePublicationTransaction for spiral_robotics_storage::StorageConnection {
    fn begin(&mut self) -> Result<(), String> {
        self.set_autocommit(false)
    }

    fn commit_publication(&mut self) -> Result<(), String> {
        self.commit()
    }

    fn rollback_publication(&mut self) -> Result<(), String> {
        self.rollback()
    }

    fn restore_autocommit(&mut self) -> Result<(), String> {
        self.set_autocommit(true)
    }

    fn discard_publication_connection(&mut self) {
        self.discard();
    }
}

fn run_feature_publication_transaction<C>(
    connection: &mut C,
    publication: impl FnOnce(&mut C) -> Result<(), String>,
) -> Result<(), String>
where
    C: FeaturePublicationTransaction,
{
    connection.begin()?;
    let mutation = publication(connection).and_then(|()| connection.commit_publication());
    let rollback = mutation.is_err().then(|| connection.rollback_publication());
    if rollback.as_ref().is_some_and(Result::is_err) {
        connection.discard_publication_connection();
    }
    let restore = connection.restore_autocommit();
    if restore.is_err() {
        connection.discard_publication_connection();
    }
    match (mutation, rollback, restore) {
        (Ok(()), None, Ok(())) => Ok(()),
        (Err(error), Some(rollback), Ok(())) => Err(feature_publication_failure(error, rollback)),
        (Ok(()), None, Err(_)) => Ok(()),
        (Err(error), Some(rollback), Err(restore)) => Err(format!(
            "{}; restoring ADBC autocommit after feature publication also failed: {restore}",
            feature_publication_failure(error, rollback)
        )),
        (Ok(()), Some(_), _) | (Err(_), None, _) => {
            unreachable!("rollback state follows feature publication outcome")
        }
    }
}

fn feature_publication_failure(error: String, rollback: Result<(), String>) -> String {
    match rollback {
        Ok(()) => error,
        Err(rollback) => {
            format!("{error}; rolling back feature publication also failed: {rollback}")
        }
    }
}

fn feature_cache_schema(features: &[CacheFeature]) -> SchemaRef {
    let mut fields = CACHE_KEY_FIELDS
        .iter()
        .map(|(name, data_type)| {
            Field::new(
                *name,
                data_type.clone(),
                cache_key_declaration_may_be_nullable(name),
            )
        })
        .collect::<Vec<_>>();
    // Spiral/Vortex intentionally does not preserve arbitrary Arrow field
    // metadata through its logical DType. The atomically published feature
    // registry is therefore the durable authority for definition and producer
    // provenance; cache fields carry only their safe registered names/types.
    fields.extend(
        features
            .iter()
            .map(|feature| Field::new(&feature.name, DataType::Float32, true)),
    );
    Arc::new(Schema::new(fields))
}

fn feature_cache_batches(cache: &FeatureCache) -> Result<Vec<RecordBatch>, String> {
    let schema = feature_cache_schema(&cache.features);
    if cache.rows.is_empty() {
        return Ok(vec![RecordBatch::new_empty(schema)]);
    }
    cache
        .rows
        .iter()
        .collect::<Vec<_>>()
        .chunks(10_000)
        .map(|rows| {
            let mut columns = vec![
                Arc::new(StringArray::from_iter_values(
                    rows.iter().map(|(key, _)| key.episode_id.as_str()),
                )) as ArrayRef,
                Arc::new(StringArray::from_iter_values(
                    rows.iter().map(|(key, _)| key.stream_id.as_str()),
                )) as ArrayRef,
                Arc::new(UInt32Array::from_iter_values(
                    rows.iter().map(|(key, _)| key.segment_index),
                )) as ArrayRef,
                Arc::new(UInt64Array::from_iter_values(
                    rows.iter().map(|(key, _)| key.frame_index),
                )) as ArrayRef,
                Arc::new(UInt64Array::from_iter_values(
                    rows.iter().map(|(key, _)| key.episode_time_ns),
                )) as ArrayRef,
            ];
            columns.extend((0..cache.features.len()).map(|index| {
                Arc::new(Float32Array::from(
                    rows.iter()
                        .map(|(_, values)| values[index])
                        .collect::<Vec<_>>(),
                )) as ArrayRef
            }));
            RecordBatch::try_new(schema.clone(), columns)
                .map_err(|error| format!("building Workflow feature cache: {error}"))
        })
        .collect()
}

fn fingerprint_field(digest: &mut Sha256, tag: &[u8], value: &[u8]) {
    digest.update((tag.len() as u64).to_le_bytes());
    digest.update(tag);
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

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

fn u64_value(array: &dyn arrow_array::Array, row: usize) -> Option<u64> {
    if array.is_null(row) {
        return None;
    }
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        Some(values.value(row))
    } else if let Some(values) = array.as_any().downcast_ref::<UInt32Array>() {
        Some(u64::from(values.value(row)))
    } else {
        array
            .as_any()
            .downcast_ref::<Int64Array>()
            .and_then(|values| u64::try_from(values.value(row)).ok())
    }
}

fn storage_type_matches(actual: &DataType, expected: &DataType) -> bool {
    actual == expected
        || matches!(
            (actual, expected),
            (
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View,
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
            ) | (
                DataType::Binary | DataType::LargeBinary | DataType::BinaryView,
                DataType::Binary | DataType::LargeBinary | DataType::BinaryView
            )
        )
        || match (actual, expected) {
            // Spiral's pinned Vortex Arrow export canonicalizes nested Utf8
            // values to Utf8View while preserving the List container and child
            // field identity/nullability. This is storage-equivalent to the
            // canonical List<Utf8> Studio writes; no other nested type drift is
            // accepted.
            (DataType::List(actual), DataType::List(expected)) => {
                actual.name() == expected.name()
                    && actual.is_nullable() == expected.is_nullable()
                    && actual.metadata() == expected.metadata()
                    && storage_type_matches(actual.data_type(), expected.data_type())
            }
            _ => false,
        }
}

fn compact_batch_rows(
    batch: &RecordBatch,
    offset: usize,
    rows: usize,
) -> Result<RecordBatch, String> {
    let end = offset
        .checked_add(rows)
        .filter(|end| *end <= batch.num_rows())
        .ok_or_else(|| "Workflow Model chunk exceeded its source batch.".to_owned())?;
    let indices = UInt64Array::from_iter_values((offset..end).map(|index| index as u64));
    let columns = batch
        .columns()
        .iter()
        .map(|column| {
            take(column.as_ref(), &indices, None)
                .map_err(|error| format!("compacting Workflow Model input: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    RecordBatch::try_new(batch.schema(), columns)
        .map_err(|error| format!("building compact Workflow Model input: {error}"))
}

fn decode_image_row(
    images: &StructArray,
    row: usize,
    processor: &SiglipImageProcessor,
) -> Result<Vec<f32>, String> {
    if images.is_null(row) {
        return Err("image is null".to_owned());
    }
    let widths = images
        .column_by_name("width")
        .and_then(|array| array.as_any().downcast_ref::<UInt32Array>())
        .ok_or_else(|| "image has no UInt32 width".to_owned())?;
    let heights = images
        .column_by_name("height")
        .and_then(|array| array.as_any().downcast_ref::<UInt32Array>())
        .ok_or_else(|| "image has no UInt32 height".to_owned())?;
    let formats = images
        .column_by_name("pixel_format")
        .and_then(|array| array.as_any().downcast_ref::<UInt8Array>())
        .ok_or_else(|| "image has no UInt8 pixel_format".to_owned())?;
    let data = images
        .column_by_name("data")
        .ok_or_else(|| "image has no pixel data".to_owned())?;
    if widths.is_null(row) || heights.is_null(row) || formats.is_null(row) {
        return Err("image dimensions or pixel format are null".to_owned());
    }
    let width = widths.value(row);
    let height = heights.value(row);
    if width == 0 || height == 0 {
        return Err("image dimensions are zero".to_owned());
    }
    let values = list_u8_value(data.as_ref(), row)
        .ok_or_else(|| "image pixels are not a non-null UInt8 list".to_owned())?;
    let dynamic = match formats.value(row) {
        1 => image::RgbImage::from_raw(width, height, values).map(image::DynamicImage::ImageRgb8),
        2 => image::RgbaImage::from_raw(width, height, values).map(image::DynamicImage::ImageRgba8),
        format => return Err(format!("unsupported image pixel format {format}")),
    }
    .ok_or_else(|| "image pixels do not match their dimensions".to_owned())?;
    let rgb = dynamic.to_rgb8();
    let resized = image::imageops::resize(
        &rgb,
        processor.image_size as u32,
        processor.image_size as u32,
        FilterType::CatmullRom,
    );
    Ok(resized
        .as_raw()
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let channel = index % 3;
            ((*value as f32 * processor.rescale_factor) - processor.image_mean[channel])
                / processor.image_std[channel]
        })
        .collect())
}

fn list_u8_value(array: &dyn arrow_array::Array, row: usize) -> Option<Vec<u8>> {
    if array.is_null(row) {
        return None;
    }
    let values = if let Some(values) = array.as_any().downcast_ref::<ListArray>() {
        values.value(row)
    } else {
        array.as_any().downcast_ref::<LargeListArray>()?.value(row)
    };
    let values = values.as_any().downcast_ref::<UInt8Array>()?;
    (values.null_count() == 0).then(|| values.values().to_vec())
}

struct SiglipArtifacts {
    config: PathBuf,
    preprocessor: PathBuf,
    tokenizer: PathBuf,
    weights: PathBuf,
}

impl SiglipArtifacts {
    fn resolve() -> Result<Self, String> {
        let api = ApiBuilder::new()
            .with_progress(false)
            .build()
            .map_err(|error| format!("opening the pinned Hugging Face model cache: {error}"))?;
        let repository = api.repo(Repo::with_revision(
            SIGLIP_BASE_REPOSITORY.to_owned(),
            RepoType::Model,
            SIGLIP_BASE_REVISION.to_owned(),
        ));
        let get = |name: &str| {
            repository.get(name).map_err(|error| {
                format!(
                    "loading pinned {SIGLIP_BASE_MODEL_ID} artifact {name:?}: {error}. Check network access, then run again."
                )
            })
        };
        Ok(Self {
            config: get("config.json")?,
            preprocessor: get("preprocessor_config.json")?,
            tokenizer: get("tokenizer.json")?,
            weights: get("model.safetensors")?,
        })
    }
}

#[derive(Deserialize)]
struct SiglipPreprocessorFile {
    #[serde(flatten)]
    operations: SiglipPreprocessorOperations,
    #[serde(flatten)]
    normalization: SiglipPreprocessorNormalization,
    #[serde(flatten)]
    resizing: SiglipPreprocessorResizing,
}

#[derive(Deserialize)]
struct SiglipPreprocessorOperations {
    do_normalize: bool,
    do_rescale: bool,
    do_resize: bool,
}

#[derive(Deserialize)]
struct SiglipPreprocessorNormalization {
    image_mean: Vec<f32>,
    image_std: Vec<f32>,
    rescale_factor: f32,
}

#[derive(Deserialize)]
struct SiglipPreprocessorResizing {
    resample: u8,
    size: SiglipPreprocessorSize,
}

#[derive(Deserialize)]
struct SiglipPreprocessorSize {
    height: usize,
    width: usize,
}

struct SiglipImageProcessor {
    image_size: usize,
    image_mean: [f32; 3],
    image_std: [f32; 3],
    rescale_factor: f32,
}

impl SiglipImageProcessor {
    fn load(path: &std::path::Path, model_image_size: usize) -> Result<Self, String> {
        let source: SiglipPreprocessorFile = serde_json::from_slice(
            &std::fs::read(path)
                .map_err(|error| format!("reading SigLIP preprocessor config: {error}"))?,
        )
        .map_err(|error| format!("decoding SigLIP preprocessor config: {error}"))?;
        Self::from_source(source, model_image_size)
    }

    fn from_source(
        source: SiglipPreprocessorFile,
        model_image_size: usize,
    ) -> Result<Self, String> {
        if !source.operations.do_resize
            || !source.operations.do_rescale
            || !source.operations.do_normalize
        {
            return Err(
                "Pinned SigLIP preprocessor must resize, rescale, and normalize images.".to_owned(),
            );
        }
        if source.resizing.resample != 3 {
            return Err(format!(
                "Pinned SigLIP preprocessor uses unsupported resample mode {}; expected PIL bicubic mode 3.",
                source.resizing.resample
            ));
        }
        if source.resizing.size.width != model_image_size
            || source.resizing.size.height != model_image_size
        {
            return Err(format!(
                "Pinned SigLIP processor dimensions {}x{} do not match model input {model_image_size}x{model_image_size}.",
                source.resizing.size.width, source.resizing.size.height
            ));
        }
        let image_mean: [f32; 3] =
            source
                .normalization
                .image_mean
                .try_into()
                .map_err(|values: Vec<f32>| {
                    format!(
                        "Pinned SigLIP preprocessor has {} mean values; expected 3.",
                        values.len()
                    )
                })?;
        let image_std: [f32; 3] =
            source
                .normalization
                .image_std
                .try_into()
                .map_err(|values: Vec<f32>| {
                    format!(
                        "Pinned SigLIP preprocessor has {} standard-deviation values; expected 3.",
                        values.len()
                    )
                })?;
        if !source.normalization.rescale_factor.is_finite()
            || source.normalization.rescale_factor <= 0.0
            || image_mean.iter().any(|value| !value.is_finite())
            || image_std
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err("Pinned SigLIP preprocessor normalization is invalid.".to_owned());
        }
        Ok(Self {
            image_size: model_image_size,
            image_mean,
            image_std,
            rescale_factor: source.normalization.rescale_factor,
        })
    }
}

struct SiglipEngine {
    model: siglip::Model,
    text_features: Tensor,
    logit_scale: Tensor,
    logit_bias: Tensor,
    device: Device,
    device_name: String,
    processor: SiglipImageProcessor,
}

struct SiglipDevice {
    device: Device,
    name: &'static str,
}

impl SiglipDevice {
    fn new(device: Device, name: &'static str) -> Self {
        Self { device, name }
    }
}

struct SelectedImageBatch<'a> {
    pixels: &'a [f32],
    images: usize,
    concept_indices: &'a [usize],
}

impl<'a> SelectedImageBatch<'a> {
    fn new(pixels: &'a [f32], images: usize, concept_indices: &'a [usize]) -> Self {
        Self {
            pixels,
            images,
            concept_indices,
        }
    }
}

impl SiglipEngine {
    fn load_preferred(
        artifacts: &SiglipArtifacts,
        concepts: &[&str],
    ) -> Result<(Self, Option<String>), String> {
        #[cfg(target_os = "macos")]
        {
            match Device::new_metal(0) {
                Ok(device) => {
                    match Self::load(artifacts, concepts, SiglipDevice::new(device, "Metal")) {
                        Ok(engine) => Ok((engine, None)),
                        Err(error) => {
                            let warning = format!(
                                "Metal SigLIP initialization failed; this run used CPU instead: {error}"
                            );
                            Self::load(artifacts, concepts, SiglipDevice::new(Device::Cpu, "CPU"))
                                .map(|engine| (engine, Some(warning)))
                        }
                    }
                }
                Err(error) => {
                    let warning =
                        format!("Metal is unavailable; this run used CPU instead: {error}");
                    Self::load(artifacts, concepts, SiglipDevice::new(Device::Cpu, "CPU"))
                        .map(|engine| (engine, Some(warning)))
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::load(artifacts, concepts, SiglipDevice::new(Device::Cpu, "CPU")).map(|engine| {
                (
                    engine,
                    Some("Metal is unavailable on this platform; this run used CPU.".to_owned()),
                )
            })
        }
    }

    fn load(
        artifacts: &SiglipArtifacts,
        concepts: &[&str],
        target: SiglipDevice,
    ) -> Result<Self, String> {
        let SiglipDevice {
            device,
            name: device_name,
        } = target;
        let config: siglip::Config = serde_json::from_slice(
            &std::fs::read(&artifacts.config)
                .map_err(|error| format!("reading SigLIP config: {error}"))?,
        )
        .map_err(|error| format!("decoding SigLIP config: {error}"))?;
        let processor =
            SiglipImageProcessor::load(&artifacts.preprocessor, config.vision_config.image_size)?;
        let mut tokenizer = Tokenizer::from_file(&artifacts.tokenizer)
            .map_err(|error| format!("loading SigLIP tokenizer: {error}"))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::Fixed(config.text_config.max_position_embeddings),
            direction: PaddingDirection::Right,
            pad_to_multiple_of: None,
            pad_id: config.text_config.pad_token_id,
            pad_type_id: 0,
            pad_token: "</s>".to_owned(),
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: config.text_config.max_position_embeddings,
                ..Default::default()
            }))
            .map_err(|error| format!("configuring SigLIP tokenizer: {error}"))?;
        let prompts = concepts
            .iter()
            .map(|concept| format!("This is a photo of {concept}."))
            .collect::<Vec<_>>();
        let encodings = tokenizer
            .encode_batch(prompts, true)
            .map_err(|error| format!("tokenizing SigLIP concepts: {error}"))?;
        let sequence = config.text_config.max_position_embeddings;
        let token_ids = encodings
            .iter()
            .flat_map(|encoding| encoding.get_ids().iter().copied())
            .collect::<Vec<_>>();
        let input_ids = Tensor::from_vec(token_ids, (concepts.len(), sequence), &device)
            .map_err(candle_error("uploading SigLIP concepts"))?;
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[artifacts.weights.as_path()], DType::F32, &device)
                .map_err(candle_error("mapping SigLIP weights"))?
        };
        let logit_scale = vb
            .get(&[1], "logit_scale")
            .map_err(candle_error("loading SigLIP logit scale"))?
            .exp()
            .map_err(candle_error("calibrating SigLIP logit scale"))?;
        let logit_bias = vb
            .get(&[1], "logit_bias")
            .map_err(candle_error("loading SigLIP logit bias"))?;
        let model =
            siglip::Model::new(&config, vb).map_err(candle_error("loading the SigLIP model"))?;
        let text_features = model
            .get_text_features(&input_ids)
            .and_then(|features| clip::div_l2_norm(&features))
            .map_err(candle_error("encoding SigLIP concepts"))?;
        device
            .synchronize()
            .map_err(candle_error("synchronizing SigLIP concept encoding"))?;
        Ok(Self {
            model,
            text_features,
            logit_scale,
            logit_bias,
            device,
            device_name: device_name.to_owned(),
            processor,
        })
    }

    fn score_selected(&self, batch: SelectedImageBatch<'_>) -> Result<Vec<Vec<f32>>, String> {
        let SelectedImageBatch {
            pixels,
            images,
            concept_indices,
        } = batch;
        let pixel_values = Tensor::from_vec(
            pixels.to_vec(),
            (
                images,
                self.processor.image_size,
                self.processor.image_size,
                3,
            ),
            &self.device,
        )
        .and_then(|tensor| tensor.permute((0, 3, 1, 2)))
        .map_err(candle_error("uploading SigLIP images"))?;
        let image_features = self
            .model
            .get_image_features(&pixel_values)
            .and_then(|features| clip::div_l2_norm(&features))
            .map_err(candle_error("encoding SigLIP images"))?;
        let indices = concept_indices
            .iter()
            .map(|index| {
                u32::try_from(*index).map_err(|_| "SigLIP concept index overflowed u32.".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let indices = Tensor::from_vec(indices, concept_indices.len(), &self.device)
            .map_err(candle_error("uploading selected SigLIP concept indices"))?;
        let selected_text = self
            .text_features
            .index_select(&indices, 0)
            .map_err(candle_error("selecting cached SigLIP concept embeddings"))?;
        let logits = selected_text
            .matmul(
                &image_features
                    .t()
                    .map_err(candle_error("transposing SigLIP image features"))?,
            )
            .and_then(|logits| logits.broadcast_mul(&self.logit_scale))
            .and_then(|logits| logits.broadcast_add(&self.logit_bias))
            .and_then(|logits| logits.t())
            .and_then(|logits| candle_nn::ops::sigmoid(&logits))
            .map_err(candle_error("computing calibrated SigLIP scores"))?;
        self.device
            .synchronize()
            .map_err(candle_error("synchronizing SigLIP inference"))?;
        logits
            .to_vec2::<f32>()
            .map_err(candle_error("reading SigLIP scores"))
    }
}

fn candle_error(context: &'static str) -> impl FnOnce(candle_core::Error) -> String {
    move |error| format!("{context}: {error}")
}

fn digest_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn model_chunk_rows(image_size: usize) -> usize {
    let budget = std::env::var("SPIRAL_WORKFLOW_MODEL_INPUT_MIB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(|mib| mib.checked_mul(1024 * 1024))
        .unwrap_or(DEFAULT_MODEL_INPUT_BUDGET_BYTES);
    chunk_rows_for_budget(image_size, budget)
}

fn chunk_rows_for_budget(image_size: usize, budget_bytes: usize) -> usize {
    let bytes_per_image = image_size
        .saturating_mul(image_size)
        .saturating_mul(3)
        .saturating_mul(std::mem::size_of::<f32>());
    if bytes_per_image == 0 {
        return MIN_MODEL_CHUNK_ROWS;
    }
    (budget_bytes / bytes_per_image).clamp(MIN_MODEL_CHUNK_ROWS, MAX_MODEL_CHUNK_ROWS)
}

fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::SeqCst)
}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancelled(cancel) {
        Err(MODEL_RUN_CANCELLED.to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use arrow_array::{ListArray, StructArray};
    use arrow_schema::Fields;

    use spiral_robotics_storage::{
        BackendStateStore, SpiralConnectionSettings, environment_spiral_endpoint,
        environment_spiral_spfs_endpoint,
    };

    fn image_field() -> Field {
        Field::new(
            "image",
            DataType::Struct(Fields::from(vec![
                Field::new(
                    "data",
                    DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))),
                    false,
                ),
                Field::new("width", DataType::UInt32, false),
                Field::new("height", DataType::UInt32, false),
                Field::new("pixel_format", DataType::UInt8, false),
            ])),
            true,
        )
        .with_metadata(HashMap::from([(
            EXTENSION_TYPE_NAME_KEY.to_owned(),
            IMAGE_EXTENSION_NAME.to_owned(),
        )]))
    }

    fn candidate_schema() -> Schema {
        Schema::new(vec![
            Field::new("episode_id", DataType::Utf8, false),
            Field::new("stream_id", DataType::Utf8, false),
            Field::new("segment_index", DataType::UInt32, false),
            Field::new("frame_index", DataType::UInt64, false),
            Field::new("episode_time_ns", DataType::UInt64, false),
            image_field(),
        ])
    }

    fn key(frame_index: u64) -> FrameKey {
        FrameKey {
            episode_id: "episode".to_owned(),
            stream_id: "camera".to_owned(),
            segment_index: 0,
            frame_index,
            episode_time_ns: frame_index * 1_000,
        }
    }

    fn feature(name: &str, concept: &str, source_relation: &str) -> WorkflowFeatureDefinition {
        WorkflowFeatureDefinition::new(
            name,
            format!("Probability that the frame shows {concept}"),
            concept,
        )
        .with_applicability(
            spiral_robotics_api::workflow::WorkflowFeatureApplicability::new(
                source_relation,
                "image",
            ),
        )
    }

    #[test]
    fn candidate_contract_requires_stable_identity_and_decoded_image() {
        let media_fields = BTreeSet::from(["image".to_owned()]);
        assert!(validate_candidate_schema(&candidate_schema(), &media_fields).is_ok());

        let nullable_identity = Schema::new(
            candidate_schema()
                .fields()
                .iter()
                .map(|field| {
                    if CACHE_KEY_FIELDS
                        .iter()
                        .any(|(name, _)| *name == field.name())
                    {
                        Arc::new(field.as_ref().clone().with_nullable(true))
                    } else {
                        field.clone()
                    }
                })
                .collect::<Vec<_>>(),
        );
        assert!(validate_candidate_schema(&nullable_identity, &media_fields).is_ok());

        let missing_identity = Schema::new(vec![image_field()]);
        assert!(
            validate_candidate_schema(&missing_identity, &media_fields)
                .unwrap_err()
                .contains("episode_id")
        );

        let mut wrong_media = candidate_schema();
        let mut fields = wrong_media
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        fields[5] = Field::new("image", DataType::Utf8, false);
        wrong_media = Schema::new(fields);
        assert!(
            validate_candidate_schema(&wrong_media, &media_fields)
                .unwrap_err()
                .contains("spql.image.Image")
        );

        let mut wrong_identity = candidate_schema();
        let mut fields = wrong_identity
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        fields[3] = Field::new("frame_index", DataType::Int64, true);
        wrong_identity = Schema::new(fields);
        assert!(
            validate_candidate_schema(&wrong_identity, &media_fields)
                .unwrap_err()
                .contains("UInt64 storage")
        );
    }

    #[test]
    fn candidate_nullable_key_declarations_still_require_non_null_row_values() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("episode_id", DataType::Utf8, true),
            Field::new("stream_id", DataType::Utf8, true),
            Field::new("segment_index", DataType::UInt32, true),
            Field::new("frame_index", DataType::UInt64, true),
            Field::new("episode_time_ns", DataType::UInt64, true),
        ]));
        let valid = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![Some("episode")])) as ArrayRef,
                Arc::new(StringArray::from(vec![Some("camera")])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![Some(0)])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![Some(12)])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![Some(400_000_000)])) as ArrayRef,
            ],
        )
        .expect("valid candidate batch");
        assert_eq!(frame_key(&valid, 0).expect("non-null key").frame_index, 12);

        let null_frame_index = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec![Some("episode")])) as ArrayRef,
                Arc::new(StringArray::from(vec![Some("camera")])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![Some(0)])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![None])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![Some(400_000_000)])) as ArrayRef,
            ],
        )
        .expect("nullable candidate batch");
        assert_eq!(
            frame_key(&null_frame_index, 0).unwrap_err(),
            "Workflow candidate field \"frame_index\" is null or not an unsigned integer."
        );
    }

    #[test]
    fn feature_cache_preserves_unrequested_rows_and_backfills_only_missing_cells() {
        let mut cache = FeatureCache {
            features: vec![CacheFeature {
                name: "has_cube_feature".to_owned(),
                definition: feature("has_cube_feature", "cube", "dataset.frames"),
            }],
            rows: BTreeMap::from([
                (key(1), vec![Some(0.8)]),
                (key(2), vec![None]),
                (key(99), vec![Some(0.3)]),
            ]),
        };
        let definitions = vec![
            feature("has_cube_feature", "cube", "dataset.frames"),
            feature("has_gripper_feature", "gripper", "dataset.frames"),
        ];
        let (requested, changed) = cache.merge_requested(&definitions).unwrap();
        assert!(changed);
        assert_eq!(requested, vec![0, 1]);
        assert_eq!(cache.rows[&key(1)], vec![Some(0.8), None]);
        assert_eq!(
            cache.missing_indices(&key(1), RequestedFeatures::new(&requested, &definitions)),
            vec![(1, 1)]
        );
        assert_eq!(
            cache.missing_indices(&key(2), RequestedFeatures::new(&requested, &definitions)),
            vec![(0, 0), (1, 1)]
        );
        assert_eq!(cache.rows[&key(99)], vec![Some(0.3), None]);

        let batches = feature_cache_batches(&cache).unwrap();
        assert_eq!(batches.iter().map(RecordBatch::num_rows).sum::<usize>(), 3);
        assert_eq!(batches[0].schema().fields().len(), 7);
        let score = batches[0]
            .column_by_name("has_gripper_feature")
            .unwrap()
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        assert_eq!(score.null_count(), 3);
    }

    #[test]
    fn feature_cache_at_column_limit_reuses_exact_definition_before_counting_new_fields() {
        let definitions = (0..MAX_FEATURE_CACHE_COLUMNS)
            .map(|index| {
                feature(
                    &format!("is_cached_{index}_feature"),
                    &format!("cached concept {index}"),
                    "dataset.frames",
                )
            })
            .collect::<Vec<_>>();
        let mut cache = FeatureCache::default();
        let (_, changed) = cache.merge_requested(&definitions).unwrap();
        assert!(changed);
        assert_eq!(cache.features.len(), MAX_FEATURE_CACHE_COLUMNS);

        let (indices, changed) = cache
            .merge_requested(std::slice::from_ref(&definitions[127]))
            .expect("an exact registered feature remains reusable at the cache limit");
        assert_eq!(indices, vec![127]);
        assert!(!changed);

        let error = cache
            .merge_requested(&[feature("is_new_feature", "new concept", "dataset.frames")])
            .unwrap_err();
        assert!(error.contains("at most 256 feature columns"));
    }

    #[test]
    fn merged_cache_row_bound_counts_only_new_candidate_keys() {
        let mut cache = FeatureCache::default();
        for frame in [0, 2, 4] {
            cache.ensure_key(key(frame));
        }
        ensure_merged_cache_row_bound(&cache, &BTreeSet::from([key(0), key(2), key(4), key(6)]), 4)
            .expect("overlap plus one new key stays within the final-cache bound");
        let error = ensure_merged_cache_row_bound(&cache, &BTreeSet::from([key(6), key(8)]), 4)
            .unwrap_err();
        assert!(error.contains("after merging this candidate cohort"));
    }

    #[test]
    fn cache_hit_metadata_describes_the_current_cohort_without_prior_run_warnings() {
        let definition = feature("is_raining_feature", "raining", "dataset.frames");
        let result = FeatureCacheHit::new(
            ModelRunRequest::new(
                "SELECT * FROM candidates",
                "workflow.features_dataset",
                vec![definition],
            ),
            CandidateScan {
                keys: BTreeSet::from([key(2), key(6)]),
                fingerprint: "current-cohort".to_owned(),
            },
        )
        .with_counts(9, 2)
        .into_result();
        assert_eq!(result.metadata.device(), "not loaded (feature cache hit)");
        assert_eq!(result.metadata.input_rows(), 2);
        assert_eq!(result.metadata.output_rows(), 9);
        assert_eq!(result.metadata.fingerprint(), "current-cohort");
        assert_eq!(result.quarantine, WorkflowQuarantineSummary::default());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn feature_cache_rejects_unsafe_schema_and_concept_collisions() {
        let unsafe_schema = Schema::new(vec![
            Field::new("episode_id", DataType::Utf8, false),
            Field::new("stream_id", DataType::Utf8, false),
            Field::new("segment_index", DataType::UInt32, false),
            Field::new("frame_index", DataType::UInt64, false),
            Field::new("episode_time_ns", DataType::UInt64, false),
            Field::new("has_cube_feature", DataType::Float32, true),
        ]);
        assert!(validate_cache_schema(&unsafe_schema, &[]).is_err());

        let mut cache = FeatureCache {
            features: vec![CacheFeature {
                name: "has_cube_feature".to_owned(),
                definition: feature("has_cube_feature", "other", "dataset.frames"),
            }],
            rows: BTreeMap::new(),
        };
        let error = cache
            .merge_requested(&[feature("has_cube_feature", "not a cube", "dataset.frames")])
            .unwrap_err();
        assert!(error.contains("incompatible registered definition"));
    }

    #[test]
    fn feature_cache_uses_the_registry_when_spiral_drops_arrow_field_metadata() {
        let definition = feature("is_raining_feature", "raining", "dataset.frames");
        let registered = vec![CacheFeature {
            name: definition.name().to_owned(),
            definition: definition.clone(),
        }];
        let schema = feature_cache_schema(&registered);
        assert!(!schema.field(0).is_nullable());
        assert!(!schema.field(1).is_nullable());
        assert!(!schema.field(2).is_nullable());
        assert!(schema.field(3).is_nullable());
        assert!(schema.field(4).is_nullable());
        let feature_field = schema.field(CACHE_KEY_FIELDS.len());
        assert!(
            feature_field.metadata().is_empty(),
            "arbitrary cache-field metadata is not a durable Spiral contract"
        );
        let loaded = validate_cache_schema(schema.as_ref(), &registered)
            .expect("a metadata-free Spiral cache must use its atomic registry definition");
        assert_eq!(loaded[0].definition, definition);

        let mut incompatible_registry = FeatureRegistryEntry::new(
            loaded[0].definition.clone(),
            derived_feature_cache_relation("dataset.frames"),
            1,
        );
        incompatible_registry.producer.artifact_revision = "different-artifact-revision".to_owned();
        assert!(
            validate_feature_registry_entry(&incompatible_registry)
                .unwrap_err()
                .contains("incompatible producer provenance")
        );
        incompatible_registry.producer.artifact_revision = SIGLIP_BASE_REVISION.to_owned();
        validate_feature_registry_entry(&incompatible_registry)
            .expect("the registry retains exact producer provenance without field metadata");
    }

    #[test]
    fn cross_workflow_cohorts_share_only_the_same_source_cache() {
        let mapping = feature("is_raining_feature", "raining", "dataset_a.frames");
        let mut cache = FeatureCache::default();
        let definitions = vec![mapping];
        let (requested, _) = cache.merge_requested(&definitions).unwrap();
        for frame in [0, 4] {
            cache.ensure_key(key(frame));
            cache.rows.get_mut(&key(frame)).unwrap()[requested[0]] = Some(0.75);
        }

        for frame in [0, 2, 4, 6] {
            cache.ensure_key(key(frame));
        }
        let missing = [0, 2, 4, 6]
            .into_iter()
            .filter(|frame| {
                cache.missing_count(
                    &key(*frame),
                    RequestedFeatures::new(&requested, &definitions),
                ) == 1
            })
            .collect::<Vec<_>>();
        assert_eq!(missing, vec![2, 6]);
        assert_eq!(cache.rows[&key(0)][0], Some(0.75));
        assert_eq!(cache.rows[&key(4)][0], Some(0.75));
        assert_ne!(
            spiral_robotics_api::workflow::derived_feature_cache_relation("dataset_a.frames"),
            spiral_robotics_api::workflow::derived_feature_cache_relation("dataset_b.frames")
        );
    }

    #[test]
    fn feature_registry_reuses_exact_definitions_and_rejects_same_name_conflicts() {
        let definition = feature("is_raining_feature", "raining", "dataset_a.frames");
        let cache_relation =
            spiral_robotics_api::workflow::derived_feature_cache_relation("dataset_a.frames");
        let mut registry = FeatureRegistry::default();
        assert!(
            registry
                .register(FeatureRegistration::new(
                    &cache_relation,
                    std::slice::from_ref(&definition),
                    10,
                ))
                .unwrap()
        );
        assert!(
            !registry
                .register(FeatureRegistration::new(
                    &cache_relation,
                    std::slice::from_ref(&definition),
                    20,
                ))
                .unwrap()
        );
        let mut conflict = definition.clone();
        conflict.set_description("A different public meaning");
        assert!(
            registry
                .register(FeatureRegistration::new(&cache_relation, &[conflict], 30))
                .unwrap_err()
                .contains("incompatible definition")
        );
        let other_source = feature("is_raining_feature", "raining", "dataset_b.frames");
        let other_cache =
            spiral_robotics_api::workflow::derived_feature_cache_relation("dataset_b.frames");
        assert!(
            registry
                .register(FeatureRegistration::new(&other_cache, &[other_source], 40))
                .unwrap()
        );

        let batch = feature_registry_batch(&registry).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema(), feature_registry_schema());
        assert_eq!(
            batch
                .column_by_name("value_semantics")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0),
            FEATURE_VALUE_SEMANTICS
        );
    }

    #[test]
    fn feature_registry_accepts_pinned_spiral_nested_utf8view_normalization_only() {
        let expected = DataType::List(Arc::new(Field::new("item", DataType::Utf8, false)));
        let spiral_round_trip =
            DataType::List(Arc::new(Field::new("item", DataType::Utf8View, false)));
        assert!(storage_type_matches(&spiral_round_trip, &expected));
        assert!(!storage_type_matches(
            &DataType::List(Arc::new(Field::new("item", DataType::BinaryView, false,))),
            &expected
        ));
        assert!(!storage_type_matches(
            &DataType::List(Arc::new(Field::new("item", DataType::Utf8View, true,))),
            &expected
        ));

        let canonical = feature_registry_schema();
        let normalized_fields = canonical
            .fields()
            .iter()
            .map(|field| {
                let data_type = match field.data_type() {
                    DataType::Utf8 => DataType::Utf8View,
                    DataType::List(item) => DataType::List(Arc::new(
                        Field::new("item", DataType::Utf8View, item.is_nullable())
                            .with_metadata(item.metadata().clone()),
                    )),
                    other => other.clone(),
                };
                Field::new(field.name(), data_type, field.is_nullable())
                    .with_metadata(field.metadata().clone())
            })
            .collect::<Vec<_>>();
        validate_feature_registry_schema(&Schema::new(normalized_fields))
            .expect("pinned Spiral Utf8View normalization is storage-equivalent");

        let view = StringViewArray::from(vec!["camera.top"]);
        assert_eq!(string_value(&view, 0), Some("camera.top"));
    }

    #[test]
    fn loaded_feature_registry_rejects_rows_beyond_the_durable_bound() {
        let source = "dataset.frames";
        let cache_relation = derived_feature_cache_relation(source);
        let mut registry = FeatureRegistry::default();
        for index in 0..MAX_FEATURE_REGISTRY_ROWS {
            let definition = feature(
                &format!("is_loaded_{index}_feature"),
                &format!("loaded concept {index}"),
                source,
            );
            registry.entries.insert(
                (source.to_owned(), definition.name().to_owned()),
                FeatureRegistryEntry::new(definition, cache_relation.clone(), 1),
            );
        }
        let definition = feature("is_one_too_many_feature", "overflow", source);
        let feature_name = definition.name().to_owned();
        let error = LoadedFeatureRegistryEntry::new(
            source.to_owned(),
            feature_name,
            FeatureRegistryEntry::new(definition, cache_relation, 2),
        )
        .insert_into(&mut registry)
        .expect_err("a persisted registry above the durable bound must be rejected while loading");
        assert!(error.contains("4096 definitions"));
    }

    #[test]
    fn feature_registry_uses_the_contracts_character_limits_for_unicode_text() {
        let source = "dataset.frames";
        let cache_relation = derived_feature_cache_relation(source);
        let mut definition = feature("is_unicode_feature", "visual", source);
        definition.set_description("é".repeat(MAX_WORKFLOW_FEATURE_DESCRIPTION_LENGTH));
        definition.set_concept("雨".repeat(MAX_SIGLIP_CONCEPT_LENGTH));
        let mut registry = FeatureRegistry::default();
        registry
            .register(FeatureRegistration::new(
                &cache_relation,
                std::slice::from_ref(&definition),
                1,
            ))
            .expect("the registry must share the contract's character-count limits");

        let mut too_long = definition;
        too_long.set_name("is_unicode_too_long_feature");
        too_long.set_description(format!("{}é", too_long.description()));
        assert!(
            registry
                .register(FeatureRegistration::new(&cache_relation, &[too_long], 2))
                .unwrap_err()
                .contains("invalid feature definition")
        );
    }

    #[test]
    fn feature_publication_reports_rollback_failure_without_hiding_the_mutation_error() {
        assert_eq!(
            feature_publication_failure(
                "replacing registry failed".to_owned(),
                Err("rollback failed".to_owned())
            ),
            "replacing registry failed; rolling back feature publication also failed: rollback failed"
        );
        assert_eq!(
            feature_publication_failure("replacing registry failed".to_owned(), Ok(())),
            "replacing registry failed"
        );

        #[derive(Default)]
        struct InjectedTransaction {
            began: bool,
            committed: bool,
            rolled_back: bool,
            restored: bool,
            discarded: bool,
            rollback_error: Option<String>,
        }

        impl FeaturePublicationTransaction for InjectedTransaction {
            fn begin(&mut self) -> Result<(), String> {
                self.began = true;
                Ok(())
            }

            fn commit_publication(&mut self) -> Result<(), String> {
                self.committed = true;
                Ok(())
            }

            fn rollback_publication(&mut self) -> Result<(), String> {
                self.rolled_back = true;
                self.rollback_error.clone().map_or(Ok(()), Err)
            }

            fn restore_autocommit(&mut self) -> Result<(), String> {
                self.restored = true;
                Ok(())
            }

            fn discard_publication_connection(&mut self) {
                self.discarded = true;
            }
        }

        let mut transaction = InjectedTransaction::default();
        let mut replacements = 0;
        let error = run_feature_publication_transaction(&mut transaction, |_| {
            replacements += 1;
            replacements += 1;
            Err("second relation replacement failed".to_owned())
        })
        .unwrap_err();
        assert_eq!(replacements, 2);
        assert!(error.contains("second relation replacement failed"));
        assert!(transaction.began);
        assert!(transaction.rolled_back);
        assert!(transaction.restored);
        assert!(!transaction.committed);

        let mut uncertain = InjectedTransaction {
            rollback_error: Some("rollback failed".to_owned()),
            ..InjectedTransaction::default()
        };
        let error = run_feature_publication_transaction(&mut uncertain, |_| {
            Err("second relation replacement failed".to_owned())
        })
        .unwrap_err();
        assert!(error.contains("rolling back feature publication also failed"));
        assert!(uncertain.discarded);
    }

    #[test]
    fn publication_gate_linearizes_commit_before_or_cancellation_before_publication() {
        let gate = Arc::new(Mutex::new(()));
        let cancel = Arc::new(AtomicBool::new(false));
        let publication = gate.lock().unwrap();
        let cancel_gate = gate.clone();
        let cancel_token = cancel.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let cancelling = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _gate = cancel_gate.lock().unwrap();
            cancel_token.store(true, Ordering::SeqCst);
        });
        started_rx.recv().unwrap();
        assert!(
            !cancel.load(Ordering::SeqCst),
            "a cancellation cannot cross a publication that already owns the gate"
        );
        drop(publication);
        cancelling.join().unwrap();
        assert!(cancel.load(Ordering::SeqCst));

        let cancelled_gate = gate.lock().unwrap();
        assert!(ensure_not_cancelled(&cancel).is_err());
        drop(cancelled_gate);
    }

    #[test]
    fn model_run_failure_keeps_publication_errors_distinct_from_cancellation() {
        assert_eq!(
            ModelRunFailure::from(MODEL_RUN_CANCELLED.to_owned()),
            ModelRunFailure::Cancelled
        );
        let failure = ModelRunFailure::from(
            "replacing the feature registry failed; rolling back also failed".to_owned(),
        );
        assert!(matches!(
            failure,
            ModelRunFailure::Failed(error) if error.contains("rolling back also failed")
        ));
    }

    #[test]
    fn nonapplicable_stream_cells_remain_null_without_being_missing_work() {
        let mut definition = feature("is_gripper_open_feature", "gripper open", "dataset.frames");
        definition.set_stream_ids(vec!["camera.top".to_owned()]);
        let definitions = vec![definition];
        let mut cache = FeatureCache::default();
        let (requested, _) = cache.merge_requested(&definitions).unwrap();
        let applicable = FrameKey {
            stream_id: "camera.top".to_owned(),
            ..key(1)
        };
        let nonapplicable = FrameKey {
            stream_id: "camera.wrist".to_owned(),
            ..key(2)
        };
        cache.ensure_key(applicable.clone());
        cache.ensure_key(nonapplicable.clone());
        assert_eq!(
            cache.missing_count(
                &applicable,
                RequestedFeatures::new(&requested, &definitions),
            ),
            1
        );
        assert_eq!(
            cache.missing_count(
                &nonapplicable,
                RequestedFeatures::new(&requested, &definitions),
            ),
            0
        );
        assert_eq!(cache.rows[&nonapplicable][0], None);
    }

    #[test]
    #[ignore = "requires a disposable real Spiral project"]
    fn feature_cache_and_registry_round_trip_in_one_spiral_transaction() {
        let stored = BackendStateStore::open_default().ok();
        let project = std::env::var("SPIRAL_PROJECT_ID")
            .ok()
            .filter(|project| !project.trim().is_empty())
            .or_else(|| stored.as_ref().and_then(BackendStateStore::project_id))
            .expect("SPIRAL_PROJECT_ID or a disposable project saved in Settings is required");
        let connection = SpiralConnectionSettings::new(
            stored
                .as_ref()
                .and_then(BackendStateStore::spiral_endpoint)
                .unwrap_or_else(environment_spiral_endpoint),
            stored
                .as_ref()
                .and_then(BackendStateStore::spiral_spfs_endpoint)
                .unwrap_or_else(environment_spiral_spfs_endpoint),
        )
        .and_then(|settings| settings.with_project(&project))
        .expect("the Workflow registry connection must be valid")
        .with_credentials(stored.as_ref().and_then(BackendStateStore::credentials));
        let project_store = Arc::new(
            ProjectStore::new(connection, "workflow".to_owned())
                .expect("the Workflow registry project store must be valid"),
        );
        let registry_existed = project_store
            .run_blocking(|connection| connection.table_exists(FEATURE_REGISTRY_RELATION))
            .expect("the Workflow registry catalog check must succeed");
        let previous_registry = load_feature_registry(&project_store)
            .expect("the existing Workflow registry must be readable");
        let source_relation = format!(
            "workflow_registry_test_{}_{}.frames",
            std::process::id(),
            current_time_millis()
        );
        let cache_relation = derived_feature_cache_relation(&source_relation);
        let mut cleanup = RegistryRoundTripCleanup {
            project_store: project_store.clone(),
            cache_relation: cache_relation.clone(),
            previous_registry: previous_registry.clone(),
            registry_existed,
            cleaned: false,
        };
        let definition = feature(
            "is_registry_round_trip_feature",
            "registry round trip",
            &source_relation,
        );
        let expected_registry_after_cleanup = previous_registry.clone();
        let mut registry = previous_registry;
        registry
            .register(FeatureRegistration::new(
                &cache_relation,
                std::slice::from_ref(&definition),
                42,
            ))
            .expect("the test feature must register");
        let mut cache = FeatureCache::default();
        let (indices, _) = cache
            .merge_requested(std::slice::from_ref(&definition))
            .expect("the test cache must admit the feature");
        cache.ensure_key(key(7));
        cache.rows.get_mut(&key(7)).unwrap()[indices[0]] = Some(0.625);

        FeaturePublication::new(&cache_relation, &cache, &registry)
            .replace(&project_store)
            .expect("cache and registry must commit together");
        project_store.invalidate_idle_connections();
        let reloaded_registry =
            load_feature_registry(&project_store).expect("the committed registry must round trip");
        let reloaded_cache =
            load_feature_cache(&project_store, &cache_relation, &reloaded_registry)
                .expect("the committed cache must round trip");
        assert_eq!(reloaded_cache.rows[&key(7)][0], Some(0.625));
        assert_eq!(
            reloaded_registry
                .entries
                .get(&(source_relation, definition.name().to_owned()))
                .unwrap()
                .producer
                .value_semantics,
            FEATURE_VALUE_SEMANTICS
        );
        cleanup
            .cleanup_now()
            .expect("the registry round trip must restore the prior project state");
        assert!(
            !project_store
                .run_blocking(|connection| connection.table_exists(&cache_relation))
                .expect("the cleaned cache catalog check must succeed"),
            "the registry round trip must remove its exact derived cache"
        );
        let restored_registry = load_feature_registry(&project_store)
            .expect("the prior Workflow registry must remain readable after cleanup");
        assert_eq!(
            restored_registry.entries, expected_registry_after_cleanup.entries,
            "the registry round trip must restore the exact prior definitions"
        );
    }

    #[test]
    fn candidate_fingerprint_identity_fields_are_length_delimited() {
        let mut first = Sha256::new();
        fingerprint_frame_key(&mut first, &key(1));
        fingerprint_frame_key(&mut first, &key(23));
        let first = first.finalize();
        let mut second = Sha256::new();
        fingerprint_frame_key(
            &mut second,
            &FrameKey {
                episode_id: "episode".to_owned(),
                stream_id: "camera".to_owned(),
                segment_index: 0,
                frame_index: 12,
                episode_time_ns: 3_000,
            },
        );
        let second = second.finalize();
        assert_ne!(first.as_slice(), second.as_slice());
    }

    #[test]
    fn invalid_pixels_are_quarantinable_without_panicking() {
        let data = Arc::new(ListArray::from_iter_primitive::<
            arrow_array::types::UInt8Type,
            _,
            _,
        >([Some(vec![Some(255), Some(0)])])) as ArrayRef;
        let images = StructArray::new(
            Fields::from(vec![
                Field::new("data", data.data_type().clone(), false),
                Field::new("width", DataType::UInt32, false),
                Field::new("height", DataType::UInt32, false),
                Field::new("pixel_format", DataType::UInt8, false),
            ]),
            vec![
                data,
                Arc::new(UInt32Array::from(vec![1])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![1])) as ArrayRef,
                Arc::new(UInt8Array::from(vec![1])) as ArrayRef,
            ],
            None,
        );
        let processor = SiglipImageProcessor {
            image_size: 224,
            image_mean: [0.5; 3],
            image_std: [0.5; 3],
            rescale_factor: 1.0 / 255.0,
        };
        assert!(decode_image_row(&images, 0, &processor).is_err());
    }

    #[test]
    fn pinned_processor_contract_requires_bicubic_and_exact_normalization() {
        let source: SiglipPreprocessorFile = serde_json::from_value(serde_json::json!({
            "do_normalize": true,
            "do_rescale": true,
            "do_resize": true,
            "image_mean": [0.5, 0.5, 0.5],
            "image_std": [0.5, 0.5, 0.5],
            "resample": 3,
            "rescale_factor": 0.00392156862745098,
            "size": {"height": 224, "width": 224}
        }))
        .unwrap();
        let processor = SiglipImageProcessor::from_source(source, 224).unwrap();
        assert_eq!(processor.image_size, 224);
        assert!((processor.rescale_factor - 1.0 / 255.0).abs() < f32::EPSILON);

        let unsupported: SiglipPreprocessorFile = serde_json::from_value(serde_json::json!({
            "do_normalize": true,
            "do_rescale": true,
            "do_resize": true,
            "image_mean": [0.5, 0.5, 0.5],
            "image_std": [0.5, 0.5, 0.5],
            "resample": 2,
            "rescale_factor": 0.00392156862745098,
            "size": {"height": 224, "width": 224}
        }))
        .unwrap();
        assert!(SiglipImageProcessor::from_source(unsupported, 224).is_err());
    }

    #[test]
    fn compact_chunks_do_not_retain_the_parent_arrow_buffers() {
        let large = "x".repeat(1024 * 1024);
        let values = StringArray::from(vec![large.as_str(), "small"]);
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "value",
                DataType::Utf8,
                false,
            )])),
            vec![Arc::new(values) as ArrayRef],
        )
        .unwrap();
        let parent_bytes = batch.column(0).get_buffer_memory_size();
        let compact = compact_batch_rows(&batch, 1, 1).unwrap();
        assert_eq!(compact.num_rows(), 1);
        assert!(compact.column(0).get_buffer_memory_size() < parent_bytes / 100);
    }

    #[test]
    fn chunk_rows_are_derived_from_budget_and_model_dimensions() {
        let bytes_per_image = 224 * 224 * 3 * std::mem::size_of::<f32>();
        assert_eq!(chunk_rows_for_budget(224, bytes_per_image * 7), 7);
        assert_eq!(chunk_rows_for_budget(224, 1), MIN_MODEL_CHUNK_ROWS);
        assert_eq!(chunk_rows_for_budget(224, usize::MAX), MAX_MODEL_CHUNK_ROWS);
    }

    /// Deterministic real-model golden: one valid black RGB image and one
    /// malformed row exercise scores, progress, and bounded quarantine in the
    /// same materialization. The expected scores were independently checked
    /// with Transformers 4.55.4 `SiglipModel` + `SiglipImageProcessor` at the
    /// exact artifact revision (0.48523283 / 0.00243547). Kept ignored because
    /// the pinned weights are large.
    #[test]
    #[ignore = "requires cached/downloadable SigLIP artifacts and disposable real Spiral project access"]
    fn siglip_image_golden_scores_progress_and_quarantine() {
        let stored = BackendStateStore::open_default().ok();
        let project = std::env::var("SPIRAL_PROJECT_ID")
            .ok()
            .filter(|project| !project.trim().is_empty())
            .or_else(|| stored.as_ref().and_then(BackendStateStore::project_id))
            .expect("SPIRAL_PROJECT_ID or a project saved in Studio Settings is required");
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let namespace = format!("workflow_golden_{}_{}", std::process::id(), suffix);
        let input_relation = format!("{namespace}.images");
        let output_relation = format!("{namespace}.scores");
        let connection = SpiralConnectionSettings::new(
            stored
                .as_ref()
                .and_then(BackendStateStore::spiral_endpoint)
                .unwrap_or_else(environment_spiral_endpoint),
            stored
                .as_ref()
                .and_then(BackendStateStore::spiral_spfs_endpoint)
                .unwrap_or_else(environment_spiral_spfs_endpoint),
        )
        .and_then(|settings| settings.with_project(&project))
        .expect("the Workflow golden connection must be valid")
        .with_credentials(stored.as_ref().and_then(BackendStateStore::credentials));
        let project_store = Arc::new(
            ProjectStore::new(connection, namespace.clone())
                .expect("the Workflow golden project store must be valid"),
        );
        let _cleanup = GoldenRelations {
            project_store: project_store.clone(),
            relations: vec![input_relation.clone(), output_relation.clone()],
            namespace: namespace.clone(),
        };

        let valid_pixels = vec![Some(0_u8); 32 * 32 * 3];
        let invalid_pixels = vec![Some(0_u8), Some(0_u8)];
        let pixels = Arc::new(ListArray::from_iter_primitive::<
            arrow_array::types::UInt8Type,
            _,
            _,
        >([Some(valid_pixels), Some(invalid_pixels)])) as ArrayRef;
        let images = Arc::new(StructArray::new(
            Fields::from(vec![
                Field::new("data", pixels.data_type().clone(), false),
                Field::new("width", DataType::UInt32, false),
                Field::new("height", DataType::UInt32, false),
                Field::new("pixel_format", DataType::UInt8, false),
            ]),
            vec![
                pixels,
                Arc::new(UInt32Array::from(vec![32, 1])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![32, 1])) as ArrayRef,
                Arc::new(UInt8Array::from(vec![1, 1])) as ArrayRef,
            ],
            None,
        )) as ArrayRef;
        let stored_image_field = Field::new("image", images.data_type().clone(), true)
            .with_metadata(HashMap::from([(
                EXTENSION_TYPE_NAME_KEY.to_owned(),
                IMAGE_EXTENSION_NAME.to_owned(),
            )]));
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("episode_id", DataType::Utf8, false),
                Field::new("stream_id", DataType::Utf8, false),
                Field::new("segment_index", DataType::UInt32, false),
                Field::new("frame_index", DataType::UInt64, false),
                Field::new("episode_time_ns", DataType::UInt64, false),
                stored_image_field,
            ])),
            vec![
                Arc::new(StringArray::from(vec!["episode", "episode"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["camera", "camera"])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![0, 0])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![0, 1])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![0, 1_000_000_000])) as ArrayRef,
                images,
            ],
        )
        .expect("the golden image fixture must match its schema");
        let seed_relation = input_relation.clone();
        let seed_namespace = namespace.clone();
        project_store
            .run_exclusive_blocking(move |connection| {
                connection.ensure_dataset_namespace(&seed_namespace)?;
                connection.ingest_batch(&seed_relation, batch)
            })
            .expect("seeding the golden image relation must succeed");
        project_store.invalidate_idle_connections();

        let features = vec![
            feature("is_black_square_feature", "black square", &input_relation),
            feature("is_white_square_feature", "white square", &input_relation),
        ];
        let mut events = Vec::new();
        let result = run_model(
            ModelRunContext::new(
                project_store.clone(),
                ModelRunRequest::new(
                    format!("SELECT * FROM {input_relation}"),
                    output_relation.clone(),
                    features,
                ),
                Arc::new(AtomicBool::new(false)),
            )
            .with_publication_gate(Arc::new(Mutex::new(()))),
            |event| events.push(event),
        )
        .expect("the golden SigLIP run must complete with a warning");
        assert_eq!(result.metadata.input_rows(), 2);
        assert_eq!(result.metadata.output_rows(), 2);
        assert_eq!(result.quarantine.count(), 1);
        assert_eq!(result.quarantine.examples().len(), 1);
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("quarantined"))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ModelRunEvent::Running { progress, .. }
                if progress.completed() == 2
                    && progress.total() == 4
                    && progress.unit() == "feature cells"
        )));

        let scored_relation = output_relation.clone();
        let scored = project_store
            .run_blocking(move |connection| {
                connection.run_query(
                    &format!(
                        "SELECT is_black_square_feature, is_white_square_feature FROM {scored_relation} WHERE frame_index = 0"
                    ),
                    1,
                )
            })
            .expect("the golden score relation must be queryable");
        assert_eq!(scored.rows().len(), 1);
        let black = scored.rows()[0]
            .get("is_black_square_feature")
            .and_then(serde_json::Value::as_f64)
            .expect("black score must be numeric");
        let white = scored.rows()[0]
            .get("is_white_square_feature")
            .and_then(serde_json::Value::as_f64)
            .expect("white score must be numeric");
        assert!((0.0..=1.0).contains(&black));
        assert!((0.0..=1.0).contains(&white));
        assert!(
            (black - 0.485_230_92).abs() < 0.001,
            "black golden score drifted: {black}"
        );
        assert!(
            (white - 0.002_435_43).abs() < 0.001,
            "white golden score drifted: {white}"
        );
        eprintln!(
            "SigLIP image golden: device={}, black={black:.8}, white={white:.8}, decode_ms={}, inference_ms={}",
            result.metadata.device(),
            result.metadata.decode_ms(),
            result.metadata.inference_ms(),
        );
    }

    struct RegistryRoundTripCleanup {
        project_store: Arc<ProjectStore>,
        cache_relation: String,
        previous_registry: FeatureRegistry,
        registry_existed: bool,
        cleaned: bool,
    }

    impl RegistryRoundTripCleanup {
        fn cleanup_now(&mut self) -> Result<(), String> {
            if self.cleaned {
                return Ok(());
            }
            let cache_relation = self.cache_relation.clone();
            let registry_existed = self.registry_existed;
            let registry_batch = feature_registry_batch(&self.previous_registry)?;
            self.project_store.run_exclusive_blocking(move |connection| {
                connection.set_autocommit(false)?;
                let mutation = (|| {
                    connection.execute_update_sql(
                        &format!("DROP TABLE IF EXISTS {cache_relation}"),
                    )?;
                    if registry_existed {
                        connection.replace_batches(
                            FEATURE_REGISTRY_RELATION,
                            vec![registry_batch],
                        )?;
                    } else {
                        connection.execute_update_sql(
                            &format!("DROP TABLE IF EXISTS {FEATURE_REGISTRY_RELATION}"),
                        )?;
                    }
                    connection.commit()
                })();
                if mutation.is_err() {
                    let _ = connection.rollback();
                }
                let restore = connection.set_autocommit(true);
                match (mutation, restore) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), Ok(())) => Err(error),
                    (Ok(()), Err(_)) => {
                        connection.discard();
                        Ok(())
                    }
                    (Err(error), Err(restore)) => Err(format!(
                        "{error}; restoring ADBC autocommit after registry cleanup also failed: {restore}"
                    )),
                }
            })?;
            self.project_store.invalidate_idle_connections();
            self.cleaned = true;
            Ok(())
        }
    }

    impl Drop for RegistryRoundTripCleanup {
        fn drop(&mut self) {
            if let Err(error) = self.cleanup_now() {
                eprintln!("Workflow registry round-trip cleanup failed: {error}");
            }
        }
    }

    struct GoldenRelations {
        project_store: Arc<ProjectStore>,
        relations: Vec<String>,
        namespace: String,
    }

    impl Drop for GoldenRelations {
        fn drop(&mut self) {
            let relations = self.relations.clone();
            let namespace = self.namespace.clone();
            let _ = self
                .project_store
                .run_exclusive_blocking(move |connection| {
                    for relation in relations {
                        if connection.table_exists(&relation)? {
                            connection.execute_update_sql(&format!("DROP TABLE {relation}"))?;
                        }
                    }
                    if connection.table_exists(&namespace)? {
                        connection.execute_update_sql(&format!("DROP TABLE {namespace}"))?;
                    }
                    Ok(())
                });
        }
    }
}
