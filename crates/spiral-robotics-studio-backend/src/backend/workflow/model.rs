//! Blocking SigLIP worker for relation-first Workflow Model nodes.
//!
//! The async manager owns run admission and snapshots. This module owns the
//! pinned artifacts, device, bounded Arrow/image pipeline, and derived Spiral
//! relation. All identifiers arrive prevalidated and server-derived.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::Instant;

use arrow_array::{
    Array as _, ArrayRef, BinaryArray, BinaryViewArray, Float32Array, Int64Array, LargeBinaryArray,
    LargeListArray, LargeStringArray, ListArray, RecordBatch, StringArray, StringViewArray,
    StructArray, UInt8Array, UInt32Array, UInt64Array,
};
use arrow_ipc::writer::StreamWriter;
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
use spiral_robotics_contracts::workflow::{
    SIGLIP_BASE_MODEL_ID, SIGLIP_BASE_REPOSITORY, SIGLIP_BASE_REVISION, WorkflowConceptColumn,
    WorkflowModelRunMetadata, WorkflowNodeProgress, WorkflowQuarantineSummary,
};
use tokenizers::Tokenizer;
use tokenizers::utils::padding::{PaddingDirection, PaddingParams, PaddingStrategy};
use tokenizers::utils::truncation::TruncationParams;

use crate::backend::storage::manager::{
    ensure_dataset_namespace, ingest_batch, replace_batches, run_query,
};
use crate::backend::storage::repository::{RepositoryConnection, SpiralRepository};
use crate::backend::storage::sql::string_literal;
use spiral_robotics_core::workflow::validate_qualified_relation;

const IMAGE_EXTENSION_NAME: &str = "spql.image.Image";
const BLOB_EXTENSION_NAME: &str = "spql.io.Blob";
const INPUT_PREFETCH_CHUNKS: usize = 2;
const MAX_QUARANTINE_EXAMPLES: usize = 20;
const MAX_VIDEO_SEGMENTS: usize = 10_000;
const DEFAULT_MODEL_INPUT_BUDGET_BYTES: usize = 8 * 1024 * 1024;
const FINGERPRINT_READ_BYTES: u64 = 8 * 1024 * 1024;
const MIN_MODEL_CHUNK_ROWS: usize = 1;
const MAX_MODEL_CHUNK_ROWS: usize = 16;
const DECODED_IMAGE_COLUMN: &str = "__workflow_image";
pub(crate) const MODEL_CLEANUP_FAILURE_PREFIX: &str =
    "Workflow Model could not clear its partial derived relation";

pub(crate) struct ModelRunRequest {
    pub input_relation: String,
    pub media_column: String,
    pub output_relation: String,
    pub concept_columns: Vec<WorkflowConceptColumn>,
    pub previous: Option<WorkflowModelRunMetadata>,
    pub previous_quarantine: WorkflowQuarantineSummary,
}

pub(crate) struct ModelRunResult {
    pub rows: usize,
    pub quarantine: WorkflowQuarantineSummary,
    pub metadata: WorkflowModelRunMetadata,
    pub warnings: Vec<String>,
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

#[derive(Clone)]
enum InputPlan {
    Images,
    VideoManifest { segments: Vec<VideoSegment> },
}

impl InputPlan {
    fn media_column<'a>(&self, configured: &'a str) -> &'a str {
        match self {
            Self::Images => configured,
            Self::VideoManifest { .. } => DECODED_IMAGE_COLUMN,
        }
    }
}

#[derive(Clone)]
struct VideoSegment {
    episode_id: String,
    stream_id: String,
    segment_index: u32,
    timeline_offset_ns: u64,
    fps: u32,
    frames: u64,
    media_relation: String,
    byte_size: Option<u64>,
    content_sha256: Option<String>,
}

struct PrefetchedBatch {
    batch: RecordBatch,
    engine_decode_ms: u64,
}

enum PrefetchedInput {
    Batch(PrefetchedBatch),
    Quarantine {
        rows: u64,
        reason: String,
        engine_decode_ms: u64,
    },
}

pub(crate) fn run_model(
    repository: Arc<SpiralRepository>,
    request: ModelRunRequest,
    cancel: Arc<AtomicBool>,
    mut publish: impl FnMut(ModelRunEvent),
) -> Result<ModelRunResult, String> {
    publish(ModelRunEvent::Validating(
        "Fingerprinting the upstream image relation".to_owned(),
    ));
    let scan = scan_input(&repository, &request, &cancel)?;
    let scan_warnings = scan.warnings.clone();
    if cancelled(&cancel) {
        return Err("Workflow Model run was cancelled.".to_owned());
    }

    if let Some(previous) = request.previous.as_ref()
        && scan.reusable
        && previous.fingerprint == scan.fingerprint
        && output_relation_matches(
            &repository,
            &request.output_relation,
            &scan.output_schema,
            previous.output_rows,
        )?
    {
        let mut metadata = previous.clone();
        metadata.reused = true;
        metadata.input_rows = scan.rows;
        return Ok(ModelRunResult {
            rows: usize::try_from(metadata.output_rows).unwrap_or(usize::MAX),
            quarantine: request.previous_quarantine,
            metadata,
            warnings: scan_warnings,
        });
    }

    publish(ModelRunEvent::Materializing(
        "Replacing the derived relation schema".to_owned(),
    ));
    replace_output_with_empty(
        &repository,
        &request.output_relation,
        scan.output_schema.clone(),
    )?;
    if scan.rows == 0 {
        repository.invalidate_idle_connections();
        return Ok(ModelRunResult {
            rows: 0,
            quarantine: WorkflowQuarantineSummary::default(),
            metadata: WorkflowModelRunMetadata {
                model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
                artifact_revision: SIGLIP_BASE_REVISION.to_owned(),
                output_relation: request.output_relation,
                concept_columns: request.concept_columns,
                fingerprint: scan.fingerprint,
                reused: false,
                device: "not loaded (empty input)".to_owned(),
                input_rows: 0,
                output_rows: 0,
                decode_ms: 0,
                inference_ms: 0,
            },
            warnings: scan_warnings,
        });
    }

    publish(ModelRunEvent::Loading(format!(
        "Loading pinned {} artifacts",
        SIGLIP_BASE_MODEL_ID
    )));
    let artifacts = SiglipArtifacts::resolve()?;
    ensure_not_cancelled(&cancel)?;
    let concepts = request
        .concept_columns
        .iter()
        .map(|mapping| mapping.concept.as_str())
        .collect::<Vec<_>>();
    let (engine, fallback_warning) = SiglipEngine::load_preferred(&artifacts, &concepts)?;
    ensure_not_cancelled(&cancel)?;
    let device_name = engine.device_name.clone();
    let chunk_rows = model_chunk_rows(engine.processor.image_size);

    publish(ModelRunEvent::Running {
        detail: format!("Scoring images on {device_name}"),
        progress: WorkflowNodeProgress {
            completed: 0,
            total: scan.rows,
            unit: "rows".to_owned(),
        },
    });
    ensure_not_cancelled(&cancel)?;

    let (sender, receiver) = mpsc::sync_channel(INPUT_PREFETCH_CHUNKS);
    let producer_repository = repository.clone();
    let producer_relation = request.input_relation.clone();
    let producer_plan = scan.plan.clone();
    let producer_cancel = cancel.clone();
    let producer = thread::Builder::new()
        .name("workflow-model-input".to_owned())
        .spawn(move || match producer_plan {
            InputPlan::Images => produce_image_batches(
                &producer_repository,
                &producer_relation,
                chunk_rows,
                &producer_cancel,
                sender,
            ),
            InputPlan::VideoManifest { segments } => produce_video_batches(
                &producer_repository,
                &segments,
                chunk_rows,
                &producer_cancel,
                sender,
            ),
        })
        .map_err(|error| format!("starting the Workflow image prefetch worker: {error}"))?;

    let mut output_rows = 0_u64;
    let mut inspected_rows = 0_u64;
    let mut decode_ms = 0_u64;
    let mut inference_ms = 0_u64;
    let mut quarantine = WorkflowQuarantineSummary::default();
    let mut pipeline_error = None;
    for item in &receiver {
        if cancelled(&cancel) {
            pipeline_error = Some("Workflow Model run was cancelled.".to_owned());
            break;
        }
        let prefetched = match item {
            Ok(PrefetchedInput::Batch(batch)) => batch,
            Ok(PrefetchedInput::Quarantine {
                rows,
                reason,
                engine_decode_ms,
            }) => {
                inspected_rows = inspected_rows.saturating_add(rows);
                decode_ms = decode_ms.saturating_add(engine_decode_ms);
                quarantine.count = quarantine.count.saturating_add(rows);
                if quarantine.examples.len() < MAX_QUARANTINE_EXAMPLES {
                    quarantine.examples.push(reason);
                }
                if !cancelled(&cancel) {
                    publish(ModelRunEvent::Running {
                        detail: format!("Scored {inspected_rows} of {} rows", scan.rows),
                        progress: WorkflowNodeProgress {
                            completed: inspected_rows.min(scan.rows),
                            total: scan.rows,
                            unit: "rows".to_owned(),
                        },
                    });
                }
                continue;
            }
            Err(error) => {
                pipeline_error = Some(error);
                break;
            }
        };
        let scored = match score_batch(
            &engine,
            &prefetched.batch,
            scan.plan.media_column(&request.media_column),
            &request.concept_columns,
            inspected_rows,
        ) {
            Ok(scored) => scored,
            Err(error) => {
                pipeline_error = Some(error);
                break;
            }
        };
        inspected_rows = inspected_rows.saturating_add(prefetched.batch.num_rows() as u64);
        decode_ms = decode_ms
            .saturating_add(prefetched.engine_decode_ms)
            .saturating_add(scored.decode_ms);
        inference_ms = inference_ms.saturating_add(scored.inference_ms);
        quarantine.count = quarantine.count.saturating_add(scored.quarantine.count);
        let remaining = MAX_QUARANTINE_EXAMPLES.saturating_sub(quarantine.examples.len());
        quarantine
            .examples
            .extend(scored.quarantine.examples.into_iter().take(remaining));
        if let Some(output) = scored.output {
            let rows = output.num_rows();
            if let Err(error) = append_output(&repository, &request.output_relation, output) {
                pipeline_error = Some(error);
                break;
            }
            output_rows = output_rows.saturating_add(rows as u64);
        }
        if !cancelled(&cancel) {
            publish(ModelRunEvent::Running {
                detail: format!("Scored {inspected_rows} of {} rows", scan.rows),
                progress: WorkflowNodeProgress {
                    completed: inspected_rows.min(scan.rows),
                    total: scan.rows,
                    unit: "rows".to_owned(),
                },
            });
        }
    }
    drop(receiver);
    let producer_result = producer
        .join()
        .map_err(|_| "Workflow image prefetch worker panicked.".to_owned())?;
    if pipeline_error.is_none() {
        pipeline_error = producer_result.err();
    }
    if cancelled(&cancel) {
        pipeline_error = Some("Workflow Model run was cancelled.".to_owned());
    }
    if pipeline_error.is_none() && inspected_rows != scan.rows {
        pipeline_error = Some(format!(
            "Workflow Model input changed between fingerprinting and scoring: fingerprinted {} rows, inspected {inspected_rows}. Run again.",
            scan.rows
        ));
    }
    if quarantine.count.saturating_mul(2) > scan.rows {
        let first_failure = quarantine
            .examples
            .first()
            .map(|example| format!(" First failure: {example}."))
            .unwrap_or_default();
        pipeline_error = Some(
            format!(
                "{} of {} input rows could not be decoded; more than half the relation was quarantined.",
                quarantine.count, scan.rows
            ) + &first_failure,
        );
    }
    if let Some(error) = pipeline_error {
        if let Err(cleanup) =
            replace_output_with_empty(&repository, &request.output_relation, scan.output_schema)
        {
            repository.invalidate_idle_connections();
            return Err(format!(
                "{MODEL_CLEANUP_FAILURE_PREFIX} after {error}: {cleanup}. Do not use that relation until a successful rerun replaces it."
            ));
        }
        repository.invalidate_idle_connections();
        return Err(error);
    }

    repository.invalidate_idle_connections();

    let mut warnings = scan_warnings;
    if let Some(warning) = fallback_warning {
        warnings.push(warning);
    }
    if quarantine.count > 0 {
        warnings.push(format!(
            "{} input rows were quarantined; inspect the bounded examples before using the scores.",
            quarantine.count
        ));
    }
    Ok(ModelRunResult {
        rows: usize::try_from(output_rows).unwrap_or(usize::MAX),
        quarantine,
        metadata: WorkflowModelRunMetadata {
            model_id: SIGLIP_BASE_MODEL_ID.to_owned(),
            artifact_revision: SIGLIP_BASE_REVISION.to_owned(),
            output_relation: request.output_relation,
            concept_columns: request.concept_columns,
            fingerprint: scan.fingerprint,
            reused: false,
            device: device_name,
            input_rows: scan.rows,
            output_rows,
            decode_ms,
            inference_ms,
        },
        warnings,
    })
}

struct InputScan {
    rows: u64,
    fingerprint: String,
    output_schema: SchemaRef,
    plan: InputPlan,
    reusable: bool,
    warnings: Vec<String>,
}

fn scan_input(
    repository: &SpiralRepository,
    request: &ModelRunRequest,
    cancel: &AtomicBool,
) -> Result<InputScan, String> {
    let relation = request.input_relation.clone();
    let media_column = request.media_column.clone();
    let concepts = request.concept_columns.clone();
    repository.run_blocking(move |connection| {
        let source_schema = connection
            .table_schema(&relation)?
            .ok_or_else(|| format!("Upstream Spiral relation {relation:?} no longer exists."))?;
        let input_kind = classify_input(&source_schema, &media_column)?;
        let digest = model_fingerprint_preamble(&relation, &media_column, &concepts);
        let mut statement = connection.new_statement()?;
        let input_sql = match input_kind {
            InputKind::Images { .. } => format!("SELECT * FROM {relation}"),
            InputKind::VideoManifest { content_fields } => {
                let content = if content_fields {
                    ", byte_size, payload.sha256 AS content_sha256"
                } else {
                    ""
                };
                format!(
                    "SELECT episode_id, stream_id, segment_index, timeline_offset_ns, fps, frames, \
                     media_relation{content} FROM {relation}"
                )
            }
        };
        statement.set_sql_query(input_sql)?;
        let mut reader = statement.execute()?;
        let writer = DigestWriter(digest);
        let mut writer = StreamWriter::try_new(writer, reader.schema().as_ref())
            .map_err(|error| format!("starting Workflow input fingerprint: {error}"))?;
        let mut rows = 0_u64;
        let mut segments = Vec::new();
        for batch in &mut reader {
            if cancelled(cancel) {
                return Err("Workflow Model run was cancelled.".to_owned());
            }
            let batch = batch.map_err(|error| format!("reading Workflow Model input: {error}"))?;
            for row in 0..batch.num_rows() {
                writer
                    .write(&batch.slice(row, 1))
                    .map_err(|error| format!("fingerprinting Workflow input row: {error}"))?;
                match input_kind {
                    InputKind::Images { .. } => rows = rows.saturating_add(1),
                    InputKind::VideoManifest { .. } => {
                        if segments.len() >= MAX_VIDEO_SEGMENTS {
                            return Err(format!(
                                "Workflow video manifest exceeds the bounded limit of {MAX_VIDEO_SEGMENTS} segments."
                            ));
                        }
                        let segment = video_segment(&batch, row)?;
                        rows = rows.saturating_add(segment.frames);
                        segments.push(segment);
                    }
                }
            }
        }
        let writer = writer
            .into_inner()
            .map_err(|error| format!("finishing Workflow input fingerprint: {error}"))?;
        drop(reader);
        drop(statement);
        let mut digest = writer.0;
        let mut warnings = Vec::new();
        let (output_schema, plan) = match input_kind {
            InputKind::Images { media_index } => (
                derived_schema(&source_schema, media_index, &concepts)?,
                InputPlan::Images,
            ),
            InputKind::VideoManifest { content_fields } => {
                validate_video_segments(&segments)?;
                if content_fields {
                    fingerprint_video_content(
                        connection,
                        &relation,
                        &segments,
                        cancel,
                        &mut digest,
                    )?;
                } else if !segments.is_empty() {
                    warnings.push(
                        "This video manifest has no durable payload identity; the derived relation will be recomputed instead of reused."
                            .to_owned(),
                    );
                }
                (
                    video_output_schema(&concepts),
                    InputPlan::VideoManifest { segments },
                )
            }
        };
        Ok(InputScan {
            rows,
            fingerprint: digest_hex(digest.finalize().as_slice()),
            output_schema,
            plan,
            reusable: matches!(input_kind, InputKind::Images { .. })
                || matches!(input_kind, InputKind::VideoManifest { content_fields: true })
                || rows == 0,
            warnings,
        })
    })
}

fn model_fingerprint_preamble(
    relation: &str,
    media_column: &str,
    concepts: &[WorkflowConceptColumn],
) -> Sha256 {
    let mut digest = Sha256::new();
    digest.update(b"spiral-workflow-siglip-fingerprint\0v2");
    fingerprint_field(&mut digest, b"model_id", SIGLIP_BASE_MODEL_ID.as_bytes());
    fingerprint_field(
        &mut digest,
        b"artifact_revision",
        SIGLIP_BASE_REVISION.as_bytes(),
    );
    fingerprint_field(&mut digest, b"input_relation", relation.as_bytes());
    fingerprint_field(&mut digest, b"media_column", media_column.as_bytes());
    for (index, mapping) in concepts.iter().enumerate() {
        fingerprint_field(&mut digest, b"concept_index", &(index as u64).to_le_bytes());
        fingerprint_field(&mut digest, b"concept", mapping.concept.as_bytes());
        fingerprint_field(
            &mut digest,
            b"score_column",
            mapping.score_column.as_bytes(),
        );
    }
    digest
}

fn fingerprint_field(digest: &mut Sha256, tag: &[u8], value: &[u8]) {
    digest.update((tag.len() as u64).to_le_bytes());
    digest.update(tag);
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

#[derive(Clone, Copy)]
enum InputKind {
    Images { media_index: usize },
    VideoManifest { content_fields: bool },
}

fn classify_input(schema: &Schema, media_column: &str) -> Result<InputKind, String> {
    let index = schema
        .index_of(media_column)
        .map_err(|_| format!("Upstream relation has no media field {media_column:?}."))?;
    let field = schema.field(index);
    let semantic = field
        .metadata()
        .get(EXTENSION_TYPE_NAME_KEY)
        .map(String::as_str);
    if semantic == Some(IMAGE_EXTENSION_NAME) {
        return Ok(InputKind::Images { media_index: index });
    }
    if media_column == "media_relation" && has_video_manifest_fields(schema) {
        return Ok(InputKind::VideoManifest {
            content_fields: has_video_content_fields(schema),
        });
    }
    Err(format!(
        "Field {media_column:?} is neither a decoded Spiral image nor the media_relation field of a video-segment manifest."
    ))
}

fn has_video_content_fields(schema: &Schema) -> bool {
    let Ok(payload) = schema.field_with_name("payload") else {
        return false;
    };
    schema.index_of("byte_size").is_ok()
        && payload
            .metadata()
            .get(EXTENSION_TYPE_NAME_KEY)
            .is_some_and(|name| name == BLOB_EXTENSION_NAME)
}

fn has_video_manifest_fields(schema: &Schema) -> bool {
    [
        "episode_id",
        "stream_id",
        "segment_index",
        "timeline_offset_ns",
        "fps",
        "frames",
        "media_relation",
    ]
    .iter()
    .all(|field| schema.index_of(field).is_ok())
}

fn derived_schema(
    source: &Schema,
    media_index: usize,
    concepts: &[WorkflowConceptColumn],
) -> Result<SchemaRef, String> {
    let mut fields = source
        .fields()
        .iter()
        .enumerate()
        .filter_map(|(index, field)| (index != media_index).then_some(field.as_ref().clone()))
        .collect::<Vec<_>>();
    let mut names = fields
        .iter()
        .map(|field| field.name().clone())
        .collect::<BTreeSet<_>>();
    for mapping in concepts {
        if !names.insert(mapping.score_column.clone()) {
            return Err(format!(
                "Derived score field {:?} collides with an upstream field.",
                mapping.score_column
            ));
        }
        fields.push(Field::new(&mapping.score_column, DataType::Float32, false));
    }
    Ok(Arc::new(Schema::new_with_metadata(
        fields,
        source.metadata().clone(),
    )))
}

fn video_output_schema(concepts: &[WorkflowConceptColumn]) -> SchemaRef {
    let mut fields = vec![
        Field::new("episode_id", DataType::Utf8, false),
        Field::new("stream_id", DataType::Utf8, false),
        Field::new("segment_index", DataType::UInt32, false),
        Field::new("frame_index", DataType::UInt64, false),
        Field::new("episode_time_ns", DataType::UInt64, false),
    ];
    fields.extend(
        concepts
            .iter()
            .map(|mapping| Field::new(&mapping.score_column, DataType::Float32, false)),
    );
    Arc::new(Schema::new(fields))
}

fn video_segment(batch: &RecordBatch, row: usize) -> Result<VideoSegment, String> {
    let text = |name: &str| -> Result<String, String> {
        let column = batch
            .column_by_name(name)
            .ok_or_else(|| format!("Video manifest is missing {name:?}."))?;
        string_value(column.as_ref(), row)
            .map(str::to_owned)
            .ok_or_else(|| format!("Video manifest field {name:?} is null or not UTF-8."))
    };
    let integer = |name: &str| -> Result<u64, String> {
        let column = batch
            .column_by_name(name)
            .ok_or_else(|| format!("Video manifest is missing {name:?}."))?;
        u64_value(column.as_ref(), row).ok_or_else(|| {
            format!("Video manifest field {name:?} is null or not an unsigned integer.")
        })
    };
    let media_relation = text("media_relation")?;
    validate_qualified_relation(&media_relation)
        .map_err(|error| format!("Video manifest media_relation is unsafe: {error}"))?;
    let fps = u32::try_from(integer("fps")?)
        .map_err(|_| "Video manifest fps overflowed UInt32.".to_owned())?;
    let frames = integer("frames")?;
    if fps == 0 || frames == 0 {
        return Err("Video manifest fps and frames must be positive.".to_owned());
    }
    let timeline = batch
        .column_by_name("timeline_offset_ns")
        .ok_or_else(|| "Video manifest is missing \"timeline_offset_ns\".".to_owned())?;
    let timeline_offset_ns = if timeline.is_null(row) {
        0
    } else {
        u64_value(timeline.as_ref(), row).ok_or_else(|| {
            "Video manifest field \"timeline_offset_ns\" is not an unsigned integer.".to_owned()
        })?
    };
    Ok(VideoSegment {
        episode_id: text("episode_id")?,
        stream_id: text("stream_id")?,
        segment_index: u32::try_from(integer("segment_index")?)
            .map_err(|_| "Video manifest segment_index overflowed UInt32.".to_owned())?,
        timeline_offset_ns,
        fps,
        frames,
        media_relation,
        byte_size: batch
            .column_by_name("byte_size")
            .and_then(|column| u64_value(column.as_ref(), row)),
        content_sha256: batch
            .column_by_name("content_sha256")
            .and_then(|column| string_value(column.as_ref(), row))
            .map(str::to_owned),
    })
}

fn fingerprint_video_content(
    connection: &mut RepositoryConnection,
    relation: &str,
    segments: &[VideoSegment],
    cancel: &AtomicBool,
    digest: &mut Sha256,
) -> Result<(), String> {
    for segment in segments {
        if cancelled(cancel) {
            return Err("Workflow Model run was cancelled.".to_owned());
        }
        if let Some(content_sha256) = segment.content_sha256.as_deref()
            && content_sha256.len() == 64
            && content_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            digest.update(b"sha256:");
            digest.update(content_sha256.to_ascii_lowercase().as_bytes());
            continue;
        }
        let byte_size = segment.byte_size.ok_or_else(|| {
            "Video manifest payload identity is incomplete: byte_size is missing.".to_owned()
        })?;
        if byte_size == 0 {
            return Err("Video manifest payload identity has a zero byte_size.".to_owned());
        }
        digest.update(b"payload:");
        digest.update(byte_size.to_le_bytes());
        let mut offset = 0_u64;
        while offset < byte_size {
            if cancelled(cancel) {
                return Err("Workflow Model run was cancelled.".to_owned());
            }
            let size = (byte_size - offset).min(FINGERPRINT_READ_BYTES);
            let sql = format!(
                "SELECT spql.io.read_bytes(payload, CAST({offset} AS UINT64), \
                 CAST({size} AS UINT64)) AS bytes FROM {relation} \
                 WHERE episode_id = {episode} AND stream_id = {stream} \
                 AND segment_index = {segment_index}",
                episode = string_literal(&segment.episode_id),
                stream = string_literal(&segment.stream_id),
                segment_index = segment.segment_index,
            );
            let bytes = read_one_binary(connection, &sql)?;
            if bytes.len() != usize::try_from(size).unwrap_or(usize::MAX) {
                return Err(format!(
                    "Video manifest payload fingerprint expected {size} bytes at offset {offset}, got {}.",
                    bytes.len()
                ));
            }
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(&bytes);
            offset = offset.saturating_add(size);
        }
    }
    Ok(())
}

fn read_one_binary(connection: &mut RepositoryConnection, sql: &str) -> Result<Vec<u8>, String> {
    let mut statement = connection.new_statement()?;
    statement.set_sql_query(sql)?;
    let mut reader = statement.execute()?;
    let mut value = None;
    for batch in &mut reader {
        let batch = batch.map_err(|error| format!("reading video payload identity: {error}"))?;
        let column = batch
            .column_by_name("bytes")
            .ok_or_else(|| "Video payload identity query returned no bytes field.".to_owned())?;
        for row in 0..batch.num_rows() {
            if value.is_some() {
                return Err("Video payload identity query returned more than one row.".to_owned());
            }
            value = binary_value(column.as_ref(), row).map(<[u8]>::to_vec);
        }
    }
    value.ok_or_else(|| "Video payload identity query returned no bytes.".to_owned())
}

fn binary_value(array: &dyn arrow_array::Array, row: usize) -> Option<&[u8]> {
    if array.is_null(row) {
        return None;
    }
    if let Some(values) = array.as_any().downcast_ref::<BinaryArray>() {
        Some(values.value(row))
    } else if let Some(values) = array.as_any().downcast_ref::<LargeBinaryArray>() {
        Some(values.value(row))
    } else {
        array
            .as_any()
            .downcast_ref::<BinaryViewArray>()
            .map(|values| values.value(row))
    }
}

fn validate_video_segments(segments: &[VideoSegment]) -> Result<(), String> {
    let mut ordered = segments.to_vec();
    ordered.sort_by(|left, right| {
        (
            &left.episode_id,
            &left.stream_id,
            left.timeline_offset_ns,
            left.segment_index,
        )
            .cmp(&(
                &right.episode_id,
                &right.stream_id,
                right.timeline_offset_ns,
                right.segment_index,
            ))
    });
    for pair in ordered.windows(2) {
        let [current, next] = pair else { continue };
        if current.episode_id != next.episode_id || current.stream_id != next.stream_id {
            continue;
        }
        let duration = current
            .frames
            .saturating_mul(1_000_000_000)
            .div_ceil(u64::from(current.fps));
        if next.timeline_offset_ns < current.timeline_offset_ns.saturating_add(duration) {
            return Err(format!(
                "Video manifest stream {:?}/{:?} has overlapping segments {} and {}.",
                current.episode_id, current.stream_id, current.segment_index, next.segment_index
            ));
        }
    }
    Ok(())
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

fn output_relation_matches(
    repository: &SpiralRepository,
    relation: &str,
    expected: &Schema,
    expected_rows: u64,
) -> Result<bool, String> {
    let relation = relation.to_owned();
    repository.run_blocking(move |connection| {
        if connection
            .table_schema(&relation)?
            .is_none_or(|schema| !output_schema_matches(&schema, expected))
        {
            return Ok(false);
        }
        let count = run_query(
            connection,
            &format!("SELECT COUNT(*) AS row_count FROM {relation}"),
            1,
        )?
        .rows
        .first()
        .and_then(|row| row.get("row_count"))
        .and_then(serde_json::Value::as_u64);
        Ok(count == Some(expected_rows))
    })
}

fn output_schema_matches(actual: &Schema, expected: &Schema) -> bool {
    actual.fields().len() == expected.fields().len()
        && actual
            .fields()
            .iter()
            .zip(expected.fields())
            .all(|(actual, expected)| {
                actual.name() == expected.name()
                    && actual.is_nullable() == expected.is_nullable()
                    && storage_type_matches(actual.data_type(), expected.data_type())
                    && actual.metadata().get(EXTENSION_TYPE_NAME_KEY)
                        == expected.metadata().get(EXTENSION_TYPE_NAME_KEY)
            })
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
}

fn replace_output_with_empty(
    repository: &SpiralRepository,
    relation: &str,
    schema: SchemaRef,
) -> Result<(), String> {
    let relation = relation.to_owned();
    repository.run_blocking(move |connection| {
        let namespace = relation
            .split_once('.')
            .map(|(namespace, _)| namespace)
            .ok_or_else(|| "Derived Workflow relation was not qualified.".to_owned())?;
        ensure_dataset_namespace(connection, namespace)?;
        replace_batches(connection, &relation, vec![RecordBatch::new_empty(schema)])
    })
}

fn append_output(
    repository: &SpiralRepository,
    relation: &str,
    batch: RecordBatch,
) -> Result<(), String> {
    let relation = relation.to_owned();
    repository.run_blocking(move |connection| ingest_batch(connection, &relation, batch))
}

fn produce_image_batches(
    repository: &SpiralRepository,
    relation: &str,
    chunk_rows: usize,
    cancel: &AtomicBool,
    sender: SyncSender<Result<PrefetchedInput, String>>,
) -> Result<(), String> {
    let relation = relation.to_owned();
    repository.run_blocking(move |connection| {
        let mut statement = connection.new_statement()?;
        statement.set_sql_query(format!("SELECT * FROM {relation}"))?;
        let mut reader = statement.execute()?;
        for batch in &mut reader {
            if cancelled(cancel) {
                return Ok(());
            }
            let batch = batch.map_err(|error| format!("reading Workflow Model input: {error}"))?;
            let mut batch_offset = 0;
            while batch_offset < batch.num_rows() {
                if cancelled(cancel) {
                    return Ok(());
                }
                let rows = chunk_rows.min(batch.num_rows() - batch_offset);
                let batch = compact_batch_rows(&batch, batch_offset, rows)?;
                if sender
                    .send(Ok(PrefetchedInput::Batch(PrefetchedBatch {
                        batch,
                        engine_decode_ms: 0,
                    })))
                    .is_err()
                {
                    return Ok(());
                }
                batch_offset += rows;
            }
        }
        Ok(())
    })
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

fn produce_video_batches(
    repository: &SpiralRepository,
    segments: &[VideoSegment],
    chunk_rows: usize,
    cancel: &AtomicBool,
    sender: SyncSender<Result<PrefetchedInput, String>>,
) -> Result<(), String> {
    for segment in segments {
        let mut start = 0_u64;
        while start < segment.frames {
            if cancelled(cancel) {
                return Ok(());
            }
            let count = (segment.frames - start).min(chunk_rows as u64);
            let sql = video_decode_sql(segment, start, count)?;
            let decode_started = Instant::now();
            let decoded = repository.run_blocking(|connection| {
                let mut statement = connection.new_statement()?;
                statement.set_sql_query(&sql)?;
                let reader = statement.execute()?;
                let mut batches = Vec::new();
                for batch in reader {
                    batches.push(
                        batch.map_err(|error| format!("reading decoded video frames: {error}"))?,
                    );
                }
                Ok(batches)
            });
            let engine_decode_ms = elapsed_millis(decode_started);
            match decoded {
                Ok(batches) => {
                    let mut returned = 0_u64;
                    let mut decode_time_available = engine_decode_ms;
                    for batch in batches {
                        let normalized = normalize_video_batch(segment, start, count, &batch)?;
                        returned = returned.saturating_add(normalized.num_rows() as u64);
                        if sender
                            .send(Ok(PrefetchedInput::Batch(PrefetchedBatch {
                                batch: normalized,
                                engine_decode_ms: std::mem::take(&mut decode_time_available),
                            })))
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                    if returned < count {
                        let missing = count - returned;
                        let reason = format!(
                            "segment {:?}/{:?}/{} frames {}..{}: Spiral returned {returned} of {count} decoded frames",
                            segment.episode_id,
                            segment.stream_id,
                            segment.segment_index,
                            start,
                            start.saturating_add(count),
                        );
                        if sender
                            .send(Ok(PrefetchedInput::Quarantine {
                                rows: missing,
                                reason,
                                engine_decode_ms: decode_time_available,
                            }))
                            .is_err()
                        {
                            return Ok(());
                        }
                    } else if returned > count {
                        return Err(format!(
                            "Spiral returned {returned} frames for a {count}-frame Workflow request."
                        ));
                    }
                }
                Err(error) => {
                    let reason = format!(
                        "segment {:?}/{:?}/{} frames {}..{}: {error}",
                        segment.episode_id,
                        segment.stream_id,
                        segment.segment_index,
                        start,
                        start.saturating_add(count),
                    );
                    if sender
                        .send(Ok(PrefetchedInput::Quarantine {
                            rows: count,
                            reason,
                            engine_decode_ms,
                        }))
                        .is_err()
                    {
                        return Ok(());
                    }
                }
            }
            start = start.saturating_add(count);
        }
    }
    Ok(())
}

fn video_decode_sql(segment: &VideoSegment, start: u64, count: u64) -> Result<String, String> {
    validate_qualified_relation(&segment.media_relation)
        .map_err(|error| format!("unsafe video media relation: {error}"))?;
    let end = start
        .checked_add(count)
        .ok_or_else(|| "Workflow video frame range overflowed.".to_owned())?;
    let indices = (start..end)
        .map(|frame| frame.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "SELECT CAST(f.request_index AS BIGINT) AS request_index, \
         spql.video.decode(f.frame, 'auto') AS {DECODED_IMAGE_COLUMN} FROM \
         (SELECT media FROM {relation} WHERE episode_id = {episode} AND stream_id = {stream} \
         AND segment_index = {segment_index}) AS a CROSS JOIN LATERAL \
         spql.video.frames(a.media, spql.video.frame_indices(ARRAY[{indices}])) AS f \
         ORDER BY request_index",
        relation = segment.media_relation,
        episode = string_literal(&segment.episode_id),
        stream = string_literal(&segment.stream_id),
        segment_index = segment.segment_index,
    ))
}

fn normalize_video_batch(
    segment: &VideoSegment,
    start: u64,
    count: u64,
    batch: &RecordBatch,
) -> Result<RecordBatch, String> {
    let request_indices = batch
        .column_by_name("request_index")
        .ok_or_else(|| "Decoded video batch has no request_index.".to_owned())?;
    let image = batch
        .column_by_name(DECODED_IMAGE_COLUMN)
        .ok_or_else(|| "Decoded video batch has no image field.".to_owned())?
        .clone();
    let image_field = batch
        .schema()
        .field_with_name(DECODED_IMAGE_COLUMN)
        .map_err(|_| "Decoded video batch has no image schema field.".to_owned())?
        .clone();
    let mut frames = Vec::with_capacity(batch.num_rows());
    let mut times = Vec::with_capacity(batch.num_rows());
    let mut seen = BTreeSet::new();
    for row in 0..batch.num_rows() {
        let request_index = u64_value(request_indices.as_ref(), row)
            .ok_or_else(|| "Decoded video request_index is null or invalid.".to_owned())?;
        if request_index >= count || !seen.insert(request_index) {
            return Err(format!(
                "Decoded video returned invalid request_index {request_index} for {count} requested frames."
            ));
        }
        let frame = start.saturating_add(request_index);
        let relative = (u128::from(frame) * 1_000_000_000_u128) / u128::from(segment.fps);
        let relative = u64::try_from(relative)
            .map_err(|_| "Decoded video timestamp overflowed UInt64.".to_owned())?;
        frames.push(frame);
        times.push(segment.timeline_offset_ns.saturating_add(relative));
    }
    let rows = batch.num_rows();
    let schema = Arc::new(Schema::new(vec![
        Field::new("episode_id", DataType::Utf8, false),
        Field::new("stream_id", DataType::Utf8, false),
        Field::new("segment_index", DataType::UInt32, false),
        Field::new("frame_index", DataType::UInt64, false),
        Field::new("episode_time_ns", DataType::UInt64, false),
        image_field,
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
                segment.episode_id.as_str(),
                rows,
            ))) as ArrayRef,
            Arc::new(StringArray::from_iter_values(std::iter::repeat_n(
                segment.stream_id.as_str(),
                rows,
            ))) as ArrayRef,
            Arc::new(UInt32Array::from_iter_values(std::iter::repeat_n(
                segment.segment_index,
                rows,
            ))) as ArrayRef,
            Arc::new(UInt64Array::from(frames)) as ArrayRef,
            Arc::new(UInt64Array::from(times)) as ArrayRef,
            image,
        ],
    )
    .map_err(|error| format!("normalizing decoded Workflow video frames: {error}"))
}

struct ScoredBatch {
    output: Option<RecordBatch>,
    quarantine: WorkflowQuarantineSummary,
    decode_ms: u64,
    inference_ms: u64,
}

fn score_batch(
    engine: &SiglipEngine,
    batch: &RecordBatch,
    media_column: &str,
    concept_columns: &[WorkflowConceptColumn],
    row_offset: u64,
) -> Result<ScoredBatch, String> {
    let media_index = batch
        .schema()
        .index_of(media_column)
        .map_err(|_| format!("Workflow Model batch has no media field {media_column:?}."))?;
    let images = batch
        .column(media_index)
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| "Decoded Spiral image field did not use struct storage.".to_owned())?;
    let decode_started = Instant::now();
    let mut pixels = Vec::new();
    let mut valid_indices = Vec::new();
    let mut quarantine = WorkflowQuarantineSummary::default();
    for row in 0..batch.num_rows() {
        match decode_image_row(images, row, &engine.processor) {
            Ok(image) => {
                pixels.extend(image);
                valid_indices.push(
                    u32::try_from(row).map_err(|_| "Workflow Model batch index overflowed u32")?,
                );
            }
            Err(error) => {
                quarantine.count = quarantine.count.saturating_add(1);
                if quarantine.examples.len() < MAX_QUARANTINE_EXAMPLES {
                    quarantine.examples.push(format!(
                        "row {}: {error}",
                        row_offset.saturating_add(row as u64)
                    ));
                }
            }
        }
    }
    let decode_ms = elapsed_millis(decode_started);
    if valid_indices.is_empty() {
        return Ok(ScoredBatch {
            output: None,
            quarantine,
            decode_ms,
            inference_ms: 0,
        });
    }
    let inference_started = Instant::now();
    let scores = engine.score(&pixels, valid_indices.len())?;
    let inference_ms = elapsed_millis(inference_started);
    let indices = UInt32Array::from(valid_indices);
    let mut output_columns = Vec::with_capacity(batch.num_columns() - 1 + scores.len());
    for (index, column) in batch.columns().iter().enumerate() {
        if index != media_index {
            output_columns.push(
                take(column.as_ref(), &indices, None)
                    .map_err(|error| format!("copying Workflow identity fields: {error}"))?,
            );
        }
    }
    for concept_index in 0..concept_columns.len() {
        output_columns.push(Arc::new(Float32Array::from(
            scores
                .iter()
                .map(|row| row[concept_index])
                .collect::<Vec<_>>(),
        )) as ArrayRef);
    }
    let schema = derived_schema(batch.schema().as_ref(), media_index, concept_columns)?;
    let output = RecordBatch::try_new(schema, output_columns)
        .map_err(|error| format!("building Workflow Model score batch: {error}"))?;
    Ok(ScoredBatch {
        output: Some(output),
        quarantine,
        decode_ms,
        inference_ms,
    })
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
    do_normalize: bool,
    do_rescale: bool,
    do_resize: bool,
    image_mean: Vec<f32>,
    image_std: Vec<f32>,
    resample: u8,
    rescale_factor: f32,
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
        if !source.do_resize || !source.do_rescale || !source.do_normalize {
            return Err(
                "Pinned SigLIP preprocessor must resize, rescale, and normalize images.".to_owned(),
            );
        }
        if source.resample != 3 {
            return Err(format!(
                "Pinned SigLIP preprocessor uses unsupported resample mode {}; expected PIL bicubic mode 3.",
                source.resample
            ));
        }
        if source.size.width != model_image_size || source.size.height != model_image_size {
            return Err(format!(
                "Pinned SigLIP processor dimensions {}x{} do not match model input {model_image_size}x{model_image_size}.",
                source.size.width, source.size.height
            ));
        }
        let image_mean: [f32; 3] = source.image_mean.try_into().map_err(|values: Vec<f32>| {
            format!(
                "Pinned SigLIP preprocessor has {} mean values; expected 3.",
                values.len()
            )
        })?;
        let image_std: [f32; 3] = source.image_std.try_into().map_err(|values: Vec<f32>| {
            format!(
                "Pinned SigLIP preprocessor has {} standard-deviation values; expected 3.",
                values.len()
            )
        })?;
        if !source.rescale_factor.is_finite()
            || source.rescale_factor <= 0.0
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
            rescale_factor: source.rescale_factor,
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

impl SiglipEngine {
    fn load_preferred(
        artifacts: &SiglipArtifacts,
        concepts: &[&str],
    ) -> Result<(Self, Option<String>), String> {
        #[cfg(target_os = "macos")]
        {
            match Device::new_metal(0) {
                Ok(device) => match Self::load(artifacts, concepts, device, "Metal") {
                    Ok(engine) => return Ok((engine, None)),
                    Err(error) => {
                        let warning = format!(
                            "Metal SigLIP initialization failed; this run used CPU instead: {error}"
                        );
                        return Self::load(artifacts, concepts, Device::Cpu, "CPU")
                            .map(|engine| (engine, Some(warning)));
                    }
                },
                Err(error) => {
                    let warning =
                        format!("Metal is unavailable; this run used CPU instead: {error}");
                    return Self::load(artifacts, concepts, Device::Cpu, "CPU")
                        .map(|engine| (engine, Some(warning)));
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::load(artifacts, concepts, Device::Cpu, "CPU").map(|engine| {
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
        device: Device,
        device_name: &str,
    ) -> Result<Self, String> {
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

    fn score(&self, pixels: &[f32], images: usize) -> Result<Vec<Vec<f32>>, String> {
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
        let logits = self
            .text_features
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

struct DigestWriter(Sha256);

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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
        Err("Workflow Model run was cancelled.".to_owned())
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

    use crate::backend::storage::manager::{
        execute_update_sql, ingest_batch, run_query, table_exists,
    };
    use crate::backend::storage::settings::{
        BackendStateStore, environment_spiral_endpoint, environment_spiral_spfs_endpoint,
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

    #[test]
    fn derived_schema_removes_pixels_and_adds_nonnullable_scores() {
        let schema = Schema::new(vec![
            Field::new("frame_index", DataType::UInt64, false),
            image_field(),
        ]);
        let output = derived_schema(
            &schema,
            1,
            &[WorkflowConceptColumn {
                concept: "red cube".to_owned(),
                score_column: "score_red_cube".to_owned(),
            }],
        )
        .unwrap();
        assert_eq!(output.fields().len(), 2);
        assert_eq!(output.field(0).name(), "frame_index");
        assert_eq!(output.field(1).name(), "score_red_cube");
        assert_eq!(output.field(1).data_type(), &DataType::Float32);
        assert!(!output.field(1).is_nullable());
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
    fn durable_view_storage_is_compatible_with_the_logical_output_schema() {
        let expected = Schema::new(vec![
            Field::new("episode_id", DataType::Utf8, false),
            Field::new("score", DataType::Float32, false),
        ]);
        let actual = Schema::new(vec![
            Field::new("episode_id", DataType::Utf8View, false),
            Field::new("score", DataType::Float32, false),
        ]);
        assert!(output_schema_matches(&actual, &expected));

        let unsafe_change = Schema::new(vec![
            Field::new("episode_id", DataType::Utf8View, false),
            Field::new("score", DataType::Float64, false),
        ]);
        assert!(!output_schema_matches(&unsafe_change, &expected));
    }

    #[test]
    fn chunk_rows_are_derived_from_budget_and_model_dimensions() {
        let bytes_per_image = 224 * 224 * 3 * std::mem::size_of::<f32>();
        assert_eq!(chunk_rows_for_budget(224, bytes_per_image * 7), 7);
        assert_eq!(chunk_rows_for_budget(224, 1), MIN_MODEL_CHUNK_ROWS);
        assert_eq!(chunk_rows_for_budget(224, usize::MAX), MAX_MODEL_CHUNK_ROWS);
    }

    #[test]
    fn video_manifest_contract_generates_scoped_engine_decode_sql() {
        let schema = Schema::new(vec![
            Field::new("episode_id", DataType::Utf8, false),
            Field::new("stream_id", DataType::Utf8, false),
            Field::new("segment_index", DataType::UInt32, false),
            Field::new("timeline_offset_ns", DataType::UInt64, true),
            Field::new("fps", DataType::UInt32, false),
            Field::new("frames", DataType::UInt64, false),
            Field::new("media_relation", DataType::Utf8, false),
        ]);
        assert!(matches!(
            classify_input(&schema, "media_relation"),
            Ok(InputKind::VideoManifest {
                content_fields: false
            })
        ));
        let segment = VideoSegment {
            episode_id: "ep'42".to_owned(),
            stream_id: "camera.front".to_owned(),
            segment_index: 3,
            timeline_offset_ns: 5,
            fps: 30,
            frames: 20,
            media_relation: "pick_place_video.assets".to_owned(),
            byte_size: None,
            content_sha256: None,
        };
        let sql = video_decode_sql(&segment, 4, 3).unwrap();
        assert!(sql.contains("spql.video.frames"));
        assert!(sql.contains("spql.video.decode"));
        assert!(!sql.contains("spql.image.resize"));
        assert!(sql.contains("ARRAY[4, 5, 6]"));
        assert!(sql.contains("episode_id = 'ep''42'"));
        assert!(sql.contains("segment_index = 3"));
    }

    #[test]
    fn invalid_timeline_offset_type_is_not_silently_treated_as_zero() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("episode_id", DataType::Utf8, false),
                Field::new("stream_id", DataType::Utf8, false),
                Field::new("segment_index", DataType::UInt32, false),
                Field::new("timeline_offset_ns", DataType::Utf8, false),
                Field::new("fps", DataType::UInt32, false),
                Field::new("frames", DataType::UInt64, false),
                Field::new("media_relation", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["episode"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["camera"])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![0])) as ArrayRef,
                Arc::new(StringArray::from(vec!["not-an-integer"])) as ArrayRef,
                Arc::new(UInt32Array::from(vec![30])) as ArrayRef,
                Arc::new(UInt64Array::from(vec![3])) as ArrayRef,
                Arc::new(StringArray::from(vec!["video.assets"])) as ArrayRef,
            ],
        )
        .unwrap();
        assert!(video_segment(&batch, 0).is_err());
    }

    #[test]
    fn overlapping_video_segments_are_rejected_before_decode() {
        let segment = VideoSegment {
            episode_id: "episode".to_owned(),
            stream_id: "camera".to_owned(),
            segment_index: 0,
            timeline_offset_ns: 0,
            fps: 10,
            frames: 20,
            media_relation: "video.assets".to_owned(),
            byte_size: None,
            content_sha256: None,
        };
        let mut overlapping = segment.clone();
        overlapping.segment_index = 1;
        overlapping.timeline_offset_ns = 1_000_000_000;
        assert!(validate_video_segments(&[segment, overlapping]).is_err());
    }

    #[test]
    fn fingerprint_fields_are_length_delimited_against_ambiguous_concatenation() {
        let mappings = vec![WorkflowConceptColumn {
            concept: "black square".to_owned(),
            score_column: "score_black_square".to_owned(),
        }];
        let first = model_fingerprint_preamble("a.b", "cd", &mappings).finalize();
        let second = model_fingerprint_preamble("a.bc", "d", &mappings).finalize();
        assert_ne!(first.as_slice(), second.as_slice());
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
        let repository = Arc::new(
            SpiralRepository::new(
                stored
                    .as_ref()
                    .and_then(BackendStateStore::spiral_endpoint)
                    .unwrap_or_else(environment_spiral_endpoint),
                stored
                    .as_ref()
                    .and_then(BackendStateStore::spiral_spfs_endpoint)
                    .unwrap_or_else(environment_spiral_spfs_endpoint),
                Some(project),
                stored.as_ref().and_then(BackendStateStore::credentials),
                namespace.clone(),
            )
            .expect("the Workflow golden repository must be valid"),
        );
        let _cleanup = GoldenRelations {
            repository: repository.clone(),
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
                Field::new("frame_index", DataType::UInt64, false),
                stored_image_field,
            ])),
            vec![Arc::new(UInt64Array::from(vec![0, 1])) as ArrayRef, images],
        )
        .expect("the golden image fixture must match its schema");
        let seed_relation = input_relation.clone();
        let seed_namespace = namespace.clone();
        repository
            .run_exclusive_blocking(move |connection| {
                ensure_dataset_namespace(connection, &seed_namespace)?;
                ingest_batch(connection, &seed_relation, batch)
            })
            .expect("seeding the golden image relation must succeed");
        repository.invalidate_idle_connections();

        let concept_columns = vec![
            WorkflowConceptColumn {
                concept: "black square".to_owned(),
                score_column: "score_black_square".to_owned(),
            },
            WorkflowConceptColumn {
                concept: "white square".to_owned(),
                score_column: "score_white_square".to_owned(),
            },
        ];
        let mut events = Vec::new();
        let result = run_model(
            repository.clone(),
            ModelRunRequest {
                input_relation,
                media_column: "image".to_owned(),
                output_relation: output_relation.clone(),
                concept_columns,
                previous: None,
                previous_quarantine: WorkflowQuarantineSummary::default(),
            },
            Arc::new(AtomicBool::new(false)),
            |event| events.push(event),
        )
        .expect("the golden SigLIP run must complete with a warning");
        assert_eq!(result.metadata.input_rows, 2);
        assert_eq!(result.metadata.output_rows, 1);
        assert_eq!(result.quarantine.count, 1);
        assert_eq!(result.quarantine.examples.len(), 1);
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("quarantined"))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ModelRunEvent::Running { progress, .. }
                if progress.completed == 2 && progress.total == 2 && progress.unit == "rows"
        )));

        let scored_relation = output_relation.clone();
        let scored = repository
            .run_blocking(move |connection| {
                run_query(
                    connection,
                    &format!(
                        "SELECT score_black_square, score_white_square FROM {scored_relation}"
                    ),
                    1,
                )
            })
            .expect("the golden score relation must be queryable");
        assert_eq!(scored.rows.len(), 1);
        let black = scored.rows[0]
            .get("score_black_square")
            .and_then(serde_json::Value::as_f64)
            .expect("black score must be numeric");
        let white = scored.rows[0]
            .get("score_white_square")
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
            result.metadata.device, result.metadata.decode_ms, result.metadata.inference_ms,
        );
    }

    struct GoldenRelations {
        repository: Arc<SpiralRepository>,
        relations: Vec<String>,
        namespace: String,
    }

    impl Drop for GoldenRelations {
        fn drop(&mut self) {
            let relations = self.relations.clone();
            let namespace = self.namespace.clone();
            let _ = self.repository.run_exclusive_blocking(move |connection| {
                for relation in relations {
                    if table_exists(connection, &relation)? {
                        execute_update_sql(connection, &format!("DROP TABLE {relation}"))?;
                    }
                }
                if table_exists(connection, &namespace)? {
                    execute_update_sql(connection, &format!("DROP TABLE {namespace}"))?;
                }
                Ok(())
            });
        }
    }
}
