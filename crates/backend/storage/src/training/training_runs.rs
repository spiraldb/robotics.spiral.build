//! Storage-owned relational rows for the durable training-run history.
//!
//! Studio's training snapshot is a wire contract that may be renamed at any
//! time. Persistence therefore stops at these storage-owned rows: every
//! persisted column, string enumeration, and list is declared here, together
//! with the field-by-field projection between a snapshot and those rows, so a
//! rename on either side stops compiling here instead of silently changing
//! what an operator's history means.

use std::sync::Arc;

use arrow_array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray, UInt32Array,
    UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use spiral_robotics_common::training::{
    ActArchitectureConfig, ActAttentionConfig, ActInputConfig, ActLayerCounts, ActTrainingConfig,
    TrainingCheckpoint, TrainingConfig, TrainingMetricPoint, TrainingPolicy, TrainingRun,
    TrainingRunPhase, TrainingSnapshot, TrainingSource,
};

use crate::db::ProjectStore;
use crate::db::{ArrowColumns, ListColumnBuilder as _, StringListColumn};
use crate::db::{Table, column as db};

/// Durable training-history operations for one project.
pub(crate) struct TrainingHistoryPersistence<'a> {
    project_store: &'a ProjectStore,
}

impl<'a> TrainingHistoryPersistence<'a> {
    pub(crate) const fn new(project_store: &'a ProjectStore) -> Self {
        Self { project_store }
    }

    pub(crate) async fn load(&self) -> Result<Vec<TrainingRun>, String> {
        if !self.project_store.is_configured() {
            return Ok(Vec::new());
        }
        let table_prefix = self.project_store.table_prefix();
        let mut connection = self.project_store.connect().await?;
        let rows = connection
            .relation_rows(
                &Table::TrainingRuns.qualified_name(table_prefix),
                TrainingRunRow::decode,
            )
            .await?;
        let metrics = connection
            .relation_rows(
                &Table::TrainingRunMetrics.qualified_name(table_prefix),
                StoredTrainingMetric::decode,
            )
            .await?;
        let checkpoints = connection
            .relation_rows(
                &Table::TrainingRunCheckpoints.qualified_name(table_prefix),
                StoredTrainingCheckpoint::decode,
            )
            .await?;
        Ok(TrainingHistoryAssembly::new(rows, metrics, checkpoints)
            .finish()
            .restored())
    }

    /// Replace all three history relations in one transaction.
    pub(crate) async fn save(&self, snapshot: &TrainingSnapshot) -> Result<(), String> {
        let table_prefix = self.project_store.table_prefix();
        let batches = StoredTrainingHistory::stored(snapshot).batches()?;
        let mut connection = self.project_store.connect().await?;
        connection.begin_transaction().await?;
        let mutation = async {
            for (table, batch) in batches {
                connection
                    .replace_batches(&table.qualified_name(table_prefix), vec![batch])
                    .await?;
            }
            Ok(())
        }
        .await;
        connection
            .finish_transaction(mutation, "training history")
            .await
    }
}

/// Policy family recorded with a training run.
///
/// The persisted spelling is owned here; a renamed wire enumeration is mapped
/// explicitly below rather than silently changing stored bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StoredTrainingPolicy {
    Act,
}

impl StoredTrainingPolicy {
    const fn stored(policy: TrainingPolicy) -> Self {
        match policy {
            TrainingPolicy::Act => Self::Act,
        }
    }

    const fn persisted(self) -> &'static str {
        match self {
            Self::Act => "act",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "act" => Ok(Self::Act),
            other => Err(format!("stored training policy {other:?} is not known")),
        }
    }
}

/// Lifecycle state recorded with a training run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StoredTrainingPhase {
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

impl StoredTrainingPhase {
    const fn stored(phase: TrainingRunPhase) -> Self {
        match phase {
            TrainingRunPhase::Starting => Self::Starting,
            TrainingRunPhase::Running => Self::Running,
            TrainingRunPhase::Stopping => Self::Stopping,
            TrainingRunPhase::Completed => Self::Completed,
            TrainingRunPhase::Stopped => Self::Stopped,
            TrainingRunPhase::Failed => Self::Failed,
        }
    }

    const fn restored(self) -> TrainingRunPhase {
        match self {
            Self::Starting => TrainingRunPhase::Starting,
            Self::Running => TrainingRunPhase::Running,
            Self::Stopping => TrainingRunPhase::Stopping,
            Self::Completed => TrainingRunPhase::Completed,
            Self::Stopped => TrainingRunPhase::Stopped,
            Self::Failed => TrainingRunPhase::Failed,
        }
    }

    const fn persisted(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "stopping" => Ok(Self::Stopping),
            "completed" => Ok(Self::Completed),
            "stopped" => Ok(Self::Stopped),
            "failed" => Ok(Self::Failed),
            other => Err(format!("stored training phase {other:?} is not known")),
        }
    }
}

/// Immutable identity of one persisted run.
#[derive(Clone, Debug, PartialEq)]
struct StoredTrainingIdentity {
    run_id: String,
    run_name: String,
    /// Dataset the run actually drew episodes from, captured at start.
    source_dataset_id: String,
}

/// Operator-chosen training set and execution target.
#[derive(Clone, Debug, PartialEq)]
struct StoredTrainingSelection {
    dataset_id: String,
    policy: StoredTrainingPolicy,
    query_sql: String,
    state_column: String,
    action_column: String,
    device: String,
}

/// Optimizer schedule recorded with a run.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StoredTrainingOptimization {
    steps: u64,
    batch_size: u64,
    validation_split: f64,
    learning_rate: f64,
    checkpoint_every: u64,
    chunk_size: u64,
    seed: u64,
}

/// Sample-preparation settings recorded with an ACT run.
#[derive(Clone, Debug, PartialEq)]
struct StoredActInput {
    camera_streams: Vec<String>,
    image_width: u64,
    image_height: u64,
    video_decoder: String,
    pretrained_backbone: bool,
}

/// Transformer depth recorded with an ACT run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StoredActLayerCounts {
    encoder: u64,
    variational_encoder: u64,
    decoder: u64,
}

/// Model shape recorded with an ACT run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StoredActArchitecture {
    hidden_dim: u64,
    latent_dim: u64,
    layers: StoredActLayerCounts,
}

/// Attention settings recorded with an ACT run.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StoredActAttention {
    attention_heads: u64,
    feedforward_dim: u64,
    kl_weight: f64,
}

/// Complete ACT configuration recorded with a run.
#[derive(Clone, Debug, PartialEq)]
struct StoredActConfig {
    input: StoredActInput,
    architecture: StoredActArchitecture,
    attention: StoredActAttention,
}

impl StoredActConfig {
    fn stored(act: &ActTrainingConfig) -> Self {
        Self {
            input: StoredActInput {
                camera_streams: act.camera_streams().to_vec(),
                image_width: act.image_width() as u64,
                image_height: act.image_height() as u64,
                video_decoder: act.video_decoder().to_owned(),
                pretrained_backbone: act.pretrained_backbone(),
            },
            architecture: StoredActArchitecture {
                hidden_dim: act.hidden_dim() as u64,
                latent_dim: act.latent_dim() as u64,
                layers: StoredActLayerCounts {
                    encoder: act.encoder_layers() as u64,
                    variational_encoder: act.vae_encoder_layers() as u64,
                    decoder: act.decoder_layers() as u64,
                },
            },
            attention: StoredActAttention {
                attention_heads: act.attention_heads() as u64,
                feedforward_dim: act.feedforward_dim() as u64,
                kl_weight: act.kl_weight(),
            },
        }
    }

    fn restored(&self) -> ActTrainingConfig {
        ActTrainingConfig::new(
            ActInputConfig::new(
                self.input.camera_streams.clone(),
                (
                    self.input.image_width as usize,
                    self.input.image_height as usize,
                ),
                self.input.video_decoder.clone(),
            )
            .with_pretrained_backbone(self.input.pretrained_backbone),
            ActArchitectureConfig::new(
                self.architecture.hidden_dim as usize,
                self.architecture.latent_dim as usize,
                ActLayerCounts::new(
                    self.architecture.layers.encoder as usize,
                    self.architecture.layers.variational_encoder as usize,
                    self.architecture.layers.decoder as usize,
                ),
            ),
            ActAttentionConfig::new(
                self.attention.attention_heads as usize,
                self.attention.feedforward_dim as usize,
                self.attention.kl_weight,
            ),
        )
    }
}

/// Mutable execution state recorded with a run.
#[derive(Clone, Debug, PartialEq)]
struct StoredTrainingLifecycle {
    phase: StoredTrainingPhase,
    current_step: u64,
    started_ms: i64,
    finished_ms: Option<i64>,
    message: Option<String>,
}

/// One sample on a run's optimizer-step axis.
#[derive(Clone, Debug, PartialEq)]
struct StoredTrainingProgress {
    step: u64,
    train_loss: f64,
    validation_loss: Option<f64>,
    learning_rate: f64,
    elapsed_seconds: f64,
    event: Option<String>,
}

/// Host and device utilization sampled with one metric point.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StoredTrainingResources {
    transfer_bytes_per_second: f64,
    examples_per_second: f64,
    cpu_percent: Option<f64>,
    gpu_percent: Option<f64>,
    memory_bytes: Option<u64>,
}

/// Per-stage durations sampled with one metric point.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StoredTrainingTimings {
    loader_ms: Option<f64>,
    arrow_encode_ms: Option<f64>,
    upload_ms: Option<f64>,
    device_copy_ms: Option<f64>,
    model_ms: Option<f64>,
}

/// One metric point together with the utilization sampled alongside it.
#[derive(Clone, Debug, PartialEq)]
struct StoredTrainingMetric {
    progress: StoredTrainingProgress,
    resources: StoredTrainingResources,
    timings: StoredTrainingTimings,
}

impl StoredTrainingMetric {
    /// Floats cross verbatim in both directions: Arrow holds a diverged run's
    /// NaN natively, so no measurement is invented or discarded here.
    fn stored(metric: &TrainingMetricPoint) -> Self {
        Self {
            progress: StoredTrainingProgress {
                step: metric.step(),
                train_loss: metric.train_loss(),
                validation_loss: metric.validation_loss(),
                learning_rate: metric.learning_rate(),
                elapsed_seconds: metric.elapsed_seconds(),
                event: metric.event().map(str::to_owned),
            },
            resources: StoredTrainingResources {
                transfer_bytes_per_second: metric.transfer_bytes_per_second(),
                examples_per_second: metric.examples_per_second(),
                cpu_percent: metric.cpu_percent(),
                gpu_percent: metric.gpu_percent(),
                memory_bytes: metric.memory_bytes(),
            },
            timings: StoredTrainingTimings {
                loader_ms: metric.loader_ms(),
                arrow_encode_ms: metric.arrow_encode_ms(),
                upload_ms: metric.upload_ms(),
                device_copy_ms: metric.device_copy_ms(),
                model_ms: metric.model_ms(),
            },
        }
    }

    fn restored(&self) -> TrainingMetricPoint {
        TrainingMetricPoint::new(self.progress.step, self.progress.train_loss)
            .with_learning_rate(self.progress.learning_rate)
            .with_validation_loss(self.progress.validation_loss)
            .with_timeline(self.progress.elapsed_seconds, self.progress.event.clone())
            .with_throughput(
                self.resources.transfer_bytes_per_second,
                self.resources.examples_per_second,
            )
            .with_utilization(self.resources.cpu_percent, self.resources.gpu_percent)
            .with_memory_bytes(self.resources.memory_bytes)
            .with_loader_ms(self.timings.loader_ms)
            .with_transport_timings(self.timings.arrow_encode_ms, self.timings.upload_ms)
            .with_runtime_timings(self.timings.device_copy_ms, self.timings.model_ms)
    }
}

/// One inference checkpoint published by a run.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StoredTrainingCheckpoint {
    step: u64,
    created_ms: i64,
    validation_loss: Option<f64>,
    byte_size: u64,
    inference_ready: bool,
    recommended_action_steps: u64,
    sample_hz: f64,
}

impl StoredTrainingCheckpoint {
    fn stored(checkpoint: &TrainingCheckpoint) -> Self {
        Self {
            step: checkpoint.step(),
            created_ms: checkpoint.created_ms(),
            validation_loss: checkpoint.validation_loss(),
            byte_size: checkpoint.byte_size(),
            inference_ready: checkpoint.inference_ready(),
            recommended_action_steps: checkpoint.recommended_action_steps() as u64,
            sample_hz: checkpoint.sample_hz(),
        }
    }

    fn restored(&self) -> TrainingCheckpoint {
        TrainingCheckpoint::new(self.step, self.created_ms)
            .with_validation_loss(self.validation_loss)
            .with_artifact(self.byte_size, self.inference_ready)
            .with_rollout(self.recommended_action_steps as usize, self.sample_hz)
    }
}

/// The child rows keyed by a run's `run_id`, plus its bounded log tail.
#[derive(Clone, Debug, Default, PartialEq)]
struct StoredTrainingRunHistory {
    metrics: Vec<StoredTrainingMetric>,
    checkpoints: Vec<StoredTrainingCheckpoint>,
    logs: Vec<String>,
}

/// One durable training run and the child rows keyed by its `run_id`.
#[derive(Clone, Debug, PartialEq)]
struct StoredTrainingRun {
    identity: StoredTrainingIdentity,
    selection: StoredTrainingSelection,
    optimization: StoredTrainingOptimization,
    act: StoredActConfig,
    lifecycle: StoredTrainingLifecycle,
    history: StoredTrainingRunHistory,
}

impl StoredTrainingRun {
    fn stored(run: &TrainingRun) -> Self {
        let config = run.config();
        Self {
            identity: StoredTrainingIdentity {
                run_id: run.id().to_owned(),
                run_name: config.run_name().to_owned(),
                source_dataset_id: run.source().dataset_id().to_owned(),
            },
            selection: StoredTrainingSelection {
                dataset_id: config.dataset_id().to_owned(),
                policy: StoredTrainingPolicy::stored(config.policy()),
                query_sql: config.sql().to_owned(),
                state_column: config.state_column().to_owned(),
                action_column: config.action_column().to_owned(),
                device: config.device().to_owned(),
            },
            optimization: StoredTrainingOptimization {
                steps: config.steps(),
                batch_size: config.batch_size() as u64,
                validation_split: config.validation_split(),
                learning_rate: config.learning_rate(),
                checkpoint_every: config.checkpoint_every(),
                chunk_size: config.chunk_size() as u64,
                seed: config.seed(),
            },
            act: StoredActConfig::stored(config.act()),
            lifecycle: StoredTrainingLifecycle {
                phase: StoredTrainingPhase::stored(run.phase()),
                current_step: run.current_step(),
                started_ms: run.started_ms(),
                finished_ms: run.finished_ms(),
                message: run.message().map(str::to_owned),
            },
            history: StoredTrainingRunHistory {
                metrics: run
                    .metrics()
                    .iter()
                    .map(StoredTrainingMetric::stored)
                    .collect(),
                checkpoints: run
                    .checkpoints()
                    .iter()
                    .map(StoredTrainingCheckpoint::stored)
                    .collect(),
                logs: run.logs().to_vec(),
            },
        }
    }

    fn restored(&self) -> TrainingRun {
        TrainingRun::starting(&self.identity.run_id, self.restored_config())
            .with_source(TrainingSource::new(&self.identity.source_dataset_id))
            .with_phase(self.lifecycle.phase.restored(), self.lifecycle.current_step)
            .with_started_ms(self.lifecycle.started_ms)
            .with_completion(self.lifecycle.finished_ms, self.lifecycle.message.clone())
            .with_metrics(
                self.history
                    .metrics
                    .iter()
                    .map(StoredTrainingMetric::restored)
                    .collect(),
            )
            .with_checkpoints(
                self.history
                    .checkpoints
                    .iter()
                    .map(StoredTrainingCheckpoint::restored)
                    .collect(),
            )
            .with_logs(self.history.logs.clone())
    }

    fn restored_config(&self) -> TrainingConfig {
        // Both policy enums have the single ACT variant today; the match keeps
        // a future storage variant from silently restoring as the default.
        match self.selection.policy {
            StoredTrainingPolicy::Act => {}
        }
        let mut config = TrainingConfig::for_dataset(
            &self.identity.run_name,
            &self.selection.dataset_id,
            &self.selection.query_sql,
        );
        config.set_device(&self.selection.device);
        config.set_state_column(&self.selection.state_column);
        config.set_action_column(&self.selection.action_column);
        config.set_steps(self.optimization.steps);
        config.set_batch_size(self.optimization.batch_size as usize);
        config.set_validation_split(self.optimization.validation_split);
        config.set_learning_rate(self.optimization.learning_rate);
        config.set_checkpoint_every(self.optimization.checkpoint_every);
        config.set_chunk_size(self.optimization.chunk_size as usize);
        config.set_seed(self.optimization.seed);
        *config.act_mut() = self.act.restored();
        config
    }
}

/// The complete persisted training history, newest run first.
#[derive(Clone, Debug, Default, PartialEq)]
struct StoredTrainingHistory {
    runs: Vec<StoredTrainingRun>,
}

impl StoredTrainingHistory {
    /// Project one wire snapshot onto storage-owned rows.
    ///
    /// Readiness is live service state, not history, so it is never stored: a
    /// restored snapshot always keeps the readiness the process already
    /// computed.
    fn stored(snapshot: &TrainingSnapshot) -> Self {
        Self {
            runs: snapshot
                .runs()
                .iter()
                .map(StoredTrainingRun::stored)
                .collect(),
        }
    }

    fn restored(&self) -> Vec<TrainingRun> {
        self.runs.iter().map(StoredTrainingRun::restored).collect()
    }
}

/// Arrow schemas for the three relations that hold one training history.
pub(super) struct TrainingRunSchema;

impl TrainingRunSchema {
    pub(super) fn runs() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::RUN_ID, DataType::Utf8, false),
            Field::new(db::ORDINAL, DataType::UInt32, false),
            Field::new(db::RUN_NAME, DataType::Utf8, false),
            Field::new(db::CONFIG_DATASET_ID, DataType::Utf8, false),
            Field::new(db::SOURCE_DATASET_ID, DataType::Utf8, false),
            Field::new(db::POLICY, DataType::Utf8, false),
            Field::new(db::QUERY_SQL, DataType::Utf8, false),
            Field::new(db::STATE_COLUMN, DataType::Utf8, false),
            Field::new(db::ACTION_COLUMN, DataType::Utf8, false),
            Field::new(db::DEVICE, DataType::Utf8, false),
            Field::new(db::STEPS, DataType::UInt64, false),
            Field::new(db::BATCH_SIZE, DataType::UInt64, false),
            Field::new(db::VALIDATION_SPLIT, DataType::Float64, false),
            Field::new(db::LEARNING_RATE, DataType::Float64, false),
            Field::new(db::CHECKPOINT_EVERY, DataType::UInt64, false),
            Field::new(db::CHUNK_SIZE, DataType::UInt64, false),
            Field::new(db::SEED, DataType::UInt64, false),
            Field::new(db::CAMERA_STREAMS, StringListColumn::data_type(), false),
            Field::new(db::IMAGE_WIDTH, DataType::UInt64, false),
            Field::new(db::IMAGE_HEIGHT, DataType::UInt64, false),
            Field::new(db::VIDEO_DECODER, DataType::Utf8, false),
            Field::new(db::PRETRAINED_BACKBONE, DataType::Boolean, false),
            Field::new(db::HIDDEN_DIM, DataType::UInt64, false),
            Field::new(db::LATENT_DIM, DataType::UInt64, false),
            Field::new(db::ENCODER_LAYERS, DataType::UInt64, false),
            Field::new(db::VAE_ENCODER_LAYERS, DataType::UInt64, false),
            Field::new(db::DECODER_LAYERS, DataType::UInt64, false),
            Field::new(db::ATTENTION_HEADS, DataType::UInt64, false),
            Field::new(db::FEEDFORWARD_DIM, DataType::UInt64, false),
            Field::new(db::KL_WEIGHT, DataType::Float64, false),
            Field::new(db::PHASE, DataType::Utf8, false),
            Field::new(db::CURRENT_STEP, DataType::UInt64, false),
            Field::new(db::STARTED_MS, DataType::Int64, false),
            Field::new(db::FINISHED_MS, DataType::Int64, true),
            Field::new(db::MESSAGE, DataType::Utf8, true),
            Field::new(db::LOGS, StringListColumn::data_type(), false),
        ]))
    }

    pub(super) fn metrics() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::RUN_ID, DataType::Utf8, false),
            Field::new(db::STEP, DataType::UInt64, false),
            Field::new(db::TRAIN_LOSS, DataType::Float64, false),
            Field::new(db::VALIDATION_LOSS, DataType::Float64, true),
            Field::new(db::LEARNING_RATE, DataType::Float64, false),
            Field::new(db::ELAPSED_SECONDS, DataType::Float64, false),
            Field::new(db::EVENT, DataType::Utf8, true),
            Field::new(db::TRANSFER_BYTES_PER_SECOND, DataType::Float64, false),
            Field::new(db::EXAMPLES_PER_SECOND, DataType::Float64, false),
            Field::new(db::CPU_PERCENT, DataType::Float64, true),
            Field::new(db::GPU_PERCENT, DataType::Float64, true),
            Field::new(db::MEMORY_BYTES, DataType::UInt64, true),
            Field::new(db::LOADER_MS, DataType::Float64, true),
            Field::new(db::ARROW_ENCODE_MS, DataType::Float64, true),
            Field::new(db::UPLOAD_MS, DataType::Float64, true),
            Field::new(db::DEVICE_COPY_MS, DataType::Float64, true),
            Field::new(db::MODEL_MS, DataType::Float64, true),
        ]))
    }

    pub(super) fn checkpoints() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new(db::RUN_ID, DataType::Utf8, false),
            Field::new(db::STEP, DataType::UInt64, false),
            Field::new(db::CREATED_MS, DataType::Int64, false),
            Field::new(db::VALIDATION_LOSS, DataType::Float64, true),
            Field::new(db::BYTE_SIZE, DataType::UInt64, false),
            Field::new(db::INFERENCE_READY, DataType::Boolean, false),
            Field::new(db::RECOMMENDED_ACTION_STEPS, DataType::UInt64, false),
            Field::new(db::SAMPLE_HZ, DataType::Float64, false),
        ]))
    }
}

impl StoredTrainingHistory {
    /// Encode the whole history as the three relations that replace it, in the
    /// order they must be written. Run order is preserved in `ordinal` so the
    /// newest-first display order survives a relational round trip without
    /// depending on physical row order.
    fn batches(&self) -> Result<[(Table, RecordBatch); 3], String> {
        let runs = self.runs.as_slice();
        let ordinals = (0..runs.len())
            .map(u32::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "training history exceeds the persisted run bound".to_owned())?;

        let mut camera_streams = StringListColumn::builder();
        let mut logs = StringListColumn::builder();
        for run in runs {
            camera_streams.append_optional_list(Some(&run.act.input.camera_streams));
            logs.append_optional_list(Some(&run.history.logs));
        }

        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.identity.run_id),
            )),
            Arc::new(UInt32Array::from(ordinals)),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.identity.run_name),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.selection.dataset_id),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.identity.source_dataset_id),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| run.selection.policy.persisted()),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.selection.query_sql),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.selection.state_column),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.selection.action_column),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.selection.device),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.optimization.steps),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.optimization.batch_size),
            )),
            Arc::new(Float64Array::from_iter_values(
                runs.iter().map(|run| run.optimization.validation_split),
            )),
            Arc::new(Float64Array::from_iter_values(
                runs.iter().map(|run| run.optimization.learning_rate),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.optimization.checkpoint_every),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.optimization.chunk_size),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.optimization.seed),
            )),
            Arc::new(camera_streams.finish()),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.act.input.image_width),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.act.input.image_height),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| &run.act.input.video_decoder),
            )),
            Arc::new(BooleanArray::from_iter(
                runs.iter()
                    .map(|run| Some(run.act.input.pretrained_backbone)),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.act.architecture.hidden_dim),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.act.architecture.latent_dim),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.act.architecture.layers.encoder),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter()
                    .map(|run| run.act.architecture.layers.variational_encoder),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.act.architecture.layers.decoder),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.act.attention.attention_heads),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.act.attention.feedforward_dim),
            )),
            Arc::new(Float64Array::from_iter_values(
                runs.iter().map(|run| run.act.attention.kl_weight),
            )),
            Arc::new(StringArray::from_iter_values(
                runs.iter().map(|run| run.lifecycle.phase.persisted()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                runs.iter().map(|run| run.lifecycle.current_step),
            )),
            Arc::new(Int64Array::from_iter_values(
                runs.iter().map(|run| run.lifecycle.started_ms),
            )),
            Arc::new(Int64Array::from_iter(
                runs.iter().map(|run| run.lifecycle.finished_ms),
            )),
            Arc::new(StringArray::from_iter(
                runs.iter().map(|run| run.lifecycle.message.as_deref()),
            )),
            Arc::new(logs.finish()),
        ];
        let runs_batch = RecordBatch::try_new(TrainingRunSchema::runs(), columns)
            .map_err(|error| format!("building training run batch: {error}"))?;

        Ok([
            (Table::TrainingRuns, runs_batch),
            (Table::TrainingRunMetrics, Self::metric_batch(runs)?),
            (Table::TrainingRunCheckpoints, Self::checkpoint_batch(runs)?),
        ])
    }

    fn metric_batch(runs: &[StoredTrainingRun]) -> Result<RecordBatch, String> {
        let points = runs
            .iter()
            .flat_map(|run| {
                run.history
                    .metrics
                    .iter()
                    .map(|metric| (&run.identity.run_id, metric))
            })
            .collect::<Vec<_>>();
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                points.iter().map(|(run_id, _)| *run_id),
            )),
            Arc::new(UInt64Array::from_iter_values(
                points.iter().map(|(_, metric)| metric.progress.step),
            )),
            Arc::new(Float64Array::from_iter_values(
                points.iter().map(|(_, metric)| metric.progress.train_loss),
            )),
            Arc::new(Float64Array::from_iter(
                points
                    .iter()
                    .map(|(_, metric)| metric.progress.validation_loss),
            )),
            Arc::new(Float64Array::from_iter_values(
                points
                    .iter()
                    .map(|(_, metric)| metric.progress.learning_rate),
            )),
            Arc::new(Float64Array::from_iter_values(
                points
                    .iter()
                    .map(|(_, metric)| metric.progress.elapsed_seconds),
            )),
            Arc::new(StringArray::from_iter(
                points
                    .iter()
                    .map(|(_, metric)| metric.progress.event.as_deref()),
            )),
            Arc::new(Float64Array::from_iter_values(
                points
                    .iter()
                    .map(|(_, metric)| metric.resources.transfer_bytes_per_second),
            )),
            Arc::new(Float64Array::from_iter_values(
                points
                    .iter()
                    .map(|(_, metric)| metric.resources.examples_per_second),
            )),
            Arc::new(Float64Array::from_iter(
                points
                    .iter()
                    .map(|(_, metric)| metric.resources.cpu_percent),
            )),
            Arc::new(Float64Array::from_iter(
                points
                    .iter()
                    .map(|(_, metric)| metric.resources.gpu_percent),
            )),
            Arc::new(UInt64Array::from_iter(
                points
                    .iter()
                    .map(|(_, metric)| metric.resources.memory_bytes),
            )),
            Arc::new(Float64Array::from_iter(
                points.iter().map(|(_, metric)| metric.timings.loader_ms),
            )),
            Arc::new(Float64Array::from_iter(
                points
                    .iter()
                    .map(|(_, metric)| metric.timings.arrow_encode_ms),
            )),
            Arc::new(Float64Array::from_iter(
                points.iter().map(|(_, metric)| metric.timings.upload_ms),
            )),
            Arc::new(Float64Array::from_iter(
                points
                    .iter()
                    .map(|(_, metric)| metric.timings.device_copy_ms),
            )),
            Arc::new(Float64Array::from_iter(
                points.iter().map(|(_, metric)| metric.timings.model_ms),
            )),
        ];
        RecordBatch::try_new(TrainingRunSchema::metrics(), columns)
            .map_err(|error| format!("building training metric batch: {error}"))
    }

    fn checkpoint_batch(runs: &[StoredTrainingRun]) -> Result<RecordBatch, String> {
        let rows = runs
            .iter()
            .flat_map(|run| {
                run.history
                    .checkpoints
                    .iter()
                    .map(|checkpoint| (&run.identity.run_id, checkpoint))
            })
            .collect::<Vec<_>>();
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|(run_id, _)| *run_id),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|(_, checkpoint)| checkpoint.step),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|(_, checkpoint)| checkpoint.created_ms),
            )),
            Arc::new(Float64Array::from_iter(
                rows.iter()
                    .map(|(_, checkpoint)| checkpoint.validation_loss),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|(_, checkpoint)| checkpoint.byte_size),
            )),
            Arc::new(BooleanArray::from_iter(
                rows.iter()
                    .map(|(_, checkpoint)| Some(checkpoint.inference_ready)),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter()
                    .map(|(_, checkpoint)| checkpoint.recommended_action_steps),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|(_, checkpoint)| checkpoint.sample_hz),
            )),
        ];
        RecordBatch::try_new(TrainingRunSchema::checkpoints(), columns)
            .map_err(|error| format!("building training checkpoint batch: {error}"))
    }
}

/// One decoded run row before its child rows are attached.
struct TrainingRunRow {
    ordinal: u32,
    run: StoredTrainingRun,
}

impl TrainingRunRow {
    fn decode(batch: &RecordBatch, row: usize) -> Result<Self, String> {
        let columns = ArrowColumns::new(batch, "training run");
        Ok(Self {
            ordinal: columns.ordinal(db::ORDINAL, row)?,
            run: StoredTrainingRun {
                identity: StoredTrainingIdentity {
                    run_id: columns.text(db::RUN_ID, row)?.to_owned(),
                    run_name: columns.text(db::RUN_NAME, row)?.to_owned(),
                    source_dataset_id: columns.text(db::SOURCE_DATASET_ID, row)?.to_owned(),
                },
                selection: StoredTrainingSelection {
                    dataset_id: columns.text(db::CONFIG_DATASET_ID, row)?.to_owned(),
                    policy: StoredTrainingPolicy::parse(columns.text(db::POLICY, row)?)?,
                    query_sql: columns.text(db::QUERY_SQL, row)?.to_owned(),
                    state_column: columns.text(db::STATE_COLUMN, row)?.to_owned(),
                    action_column: columns.text(db::ACTION_COLUMN, row)?.to_owned(),
                    device: columns.text(db::DEVICE, row)?.to_owned(),
                },
                optimization: StoredTrainingOptimization {
                    steps: columns.unsigned(db::STEPS, row)?,
                    batch_size: columns.unsigned(db::BATCH_SIZE, row)?,
                    validation_split: columns.float(db::VALIDATION_SPLIT, row)?,
                    learning_rate: columns.float(db::LEARNING_RATE, row)?,
                    checkpoint_every: columns.unsigned(db::CHECKPOINT_EVERY, row)?,
                    chunk_size: columns.unsigned(db::CHUNK_SIZE, row)?,
                    seed: columns.unsigned(db::SEED, row)?,
                },
                act: StoredActConfig {
                    input: StoredActInput {
                        camera_streams: columns.text_list(db::CAMERA_STREAMS, row)?,
                        image_width: columns.unsigned(db::IMAGE_WIDTH, row)?,
                        image_height: columns.unsigned(db::IMAGE_HEIGHT, row)?,
                        video_decoder: columns.text(db::VIDEO_DECODER, row)?.to_owned(),
                        pretrained_backbone: columns.boolean(db::PRETRAINED_BACKBONE, row)?,
                    },
                    architecture: StoredActArchitecture {
                        hidden_dim: columns.unsigned(db::HIDDEN_DIM, row)?,
                        latent_dim: columns.unsigned(db::LATENT_DIM, row)?,
                        layers: StoredActLayerCounts {
                            encoder: columns.unsigned(db::ENCODER_LAYERS, row)?,
                            variational_encoder: columns.unsigned(db::VAE_ENCODER_LAYERS, row)?,
                            decoder: columns.unsigned(db::DECODER_LAYERS, row)?,
                        },
                    },
                    attention: StoredActAttention {
                        attention_heads: columns.unsigned(db::ATTENTION_HEADS, row)?,
                        feedforward_dim: columns.unsigned(db::FEEDFORWARD_DIM, row)?,
                        kl_weight: columns.float(db::KL_WEIGHT, row)?,
                    },
                },
                lifecycle: StoredTrainingLifecycle {
                    phase: StoredTrainingPhase::parse(columns.text(db::PHASE, row)?)?,
                    current_step: columns.unsigned(db::CURRENT_STEP, row)?,
                    started_ms: columns.timestamp(db::STARTED_MS, row)?,
                    finished_ms: columns.optional_timestamp(db::FINISHED_MS, row)?,
                    message: columns.optional_text(db::MESSAGE, row)?.map(str::to_owned),
                },
                history: StoredTrainingRunHistory {
                    metrics: Vec::new(),
                    checkpoints: Vec::new(),
                    logs: columns.text_list(db::LOGS, row)?,
                },
            },
        })
    }
}

impl StoredTrainingMetric {
    fn decode(batch: &RecordBatch, row: usize) -> Result<(String, Self), String> {
        let columns = ArrowColumns::new(batch, "training metric");
        let metric = Self {
            progress: StoredTrainingProgress {
                step: columns.unsigned(db::STEP, row)?,
                train_loss: columns.float(db::TRAIN_LOSS, row)?,
                validation_loss: columns.optional_float(db::VALIDATION_LOSS, row)?,
                learning_rate: columns.float(db::LEARNING_RATE, row)?,
                elapsed_seconds: columns.float(db::ELAPSED_SECONDS, row)?,
                event: columns.optional_text(db::EVENT, row)?.map(str::to_owned),
            },
            resources: StoredTrainingResources {
                transfer_bytes_per_second: columns.float(db::TRANSFER_BYTES_PER_SECOND, row)?,
                examples_per_second: columns.float(db::EXAMPLES_PER_SECOND, row)?,
                cpu_percent: columns.optional_float(db::CPU_PERCENT, row)?,
                gpu_percent: columns.optional_float(db::GPU_PERCENT, row)?,
                memory_bytes: columns.optional_unsigned(db::MEMORY_BYTES, row)?,
            },
            timings: StoredTrainingTimings {
                loader_ms: columns.optional_float(db::LOADER_MS, row)?,
                arrow_encode_ms: columns.optional_float(db::ARROW_ENCODE_MS, row)?,
                upload_ms: columns.optional_float(db::UPLOAD_MS, row)?,
                device_copy_ms: columns.optional_float(db::DEVICE_COPY_MS, row)?,
                model_ms: columns.optional_float(db::MODEL_MS, row)?,
            },
        };
        Ok((columns.text(db::RUN_ID, row)?.to_owned(), metric))
    }
}

impl StoredTrainingCheckpoint {
    fn decode(batch: &RecordBatch, row: usize) -> Result<(String, Self), String> {
        let columns = ArrowColumns::new(batch, "training checkpoint");
        let checkpoint = Self {
            step: columns.unsigned(db::STEP, row)?,
            created_ms: columns.timestamp(db::CREATED_MS, row)?,
            validation_loss: columns.optional_float(db::VALIDATION_LOSS, row)?,
            byte_size: columns.unsigned(db::BYTE_SIZE, row)?,
            inference_ready: columns.boolean(db::INFERENCE_READY, row)?,
            recommended_action_steps: columns.unsigned(db::RECOMMENDED_ACTION_STEPS, row)?,
            sample_hz: columns.float(db::SAMPLE_HZ, row)?,
        };
        Ok((columns.text(db::RUN_ID, row)?.to_owned(), checkpoint))
    }
}

/// Assemble decoded run rows and their child rows into one ordered history.
struct TrainingHistoryAssembly {
    rows: Vec<TrainingRunRow>,
    metrics: Vec<(String, StoredTrainingMetric)>,
    checkpoints: Vec<(String, StoredTrainingCheckpoint)>,
}

impl TrainingHistoryAssembly {
    const fn new(
        rows: Vec<TrainingRunRow>,
        metrics: Vec<(String, StoredTrainingMetric)>,
        checkpoints: Vec<(String, StoredTrainingCheckpoint)>,
    ) -> Self {
        Self {
            rows,
            metrics,
            checkpoints,
        }
    }

    /// Child rows arrive unordered; sort each run's metrics and checkpoints by
    /// step so a reader sees the same monotonic series it wrote.
    fn finish(self) -> StoredTrainingHistory {
        let Self {
            mut rows,
            metrics,
            checkpoints,
        } = self;
        rows.sort_by(|left, right| {
            left.ordinal
                .cmp(&right.ordinal)
                .then_with(|| left.run.identity.run_id.cmp(&right.run.identity.run_id))
        });
        let runs = rows
            .into_iter()
            .map(|mut row| {
                let run_id = &row.run.identity.run_id;
                let mut run_metrics = metrics
                    .iter()
                    .filter(|(owner, _)| owner == run_id)
                    .map(|(_, metric)| metric.clone())
                    .collect::<Vec<_>>();
                run_metrics.sort_by_key(|metric| metric.progress.step);
                let mut run_checkpoints = checkpoints
                    .iter()
                    .filter(|(owner, _)| owner == run_id)
                    .map(|(_, checkpoint)| *checkpoint)
                    .collect::<Vec<_>>();
                run_checkpoints.sort_by_key(|checkpoint| checkpoint.step);
                row.run.history.metrics = run_metrics;
                row.run.history.checkpoints = run_checkpoints;
                row.run
            })
            .collect();
        StoredTrainingHistory { runs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::training::TrainingReadiness;

    fn run(run_id: &str, phase: StoredTrainingPhase) -> StoredTrainingRun {
        StoredTrainingRun {
            identity: StoredTrainingIdentity {
                run_id: run_id.to_owned(),
                run_name: format!("{run_id} name"),
                source_dataset_id: "default".to_owned(),
            },
            selection: StoredTrainingSelection {
                dataset_id: "pick-place".to_owned(),
                policy: StoredTrainingPolicy::Act,
                query_sql: "SELECT 1".to_owned(),
                state_column: "state".to_owned(),
                action_column: "action".to_owned(),
                device: "auto".to_owned(),
            },
            optimization: StoredTrainingOptimization {
                steps: 100,
                batch_size: 8,
                validation_split: 0.1,
                learning_rate: 1.0e-5,
                checkpoint_every: 20,
                chunk_size: 4,
                seed: 7,
            },
            act: StoredActConfig {
                input: StoredActInput {
                    camera_streams: vec!["wrist".to_owned()],
                    image_width: 32,
                    image_height: 24,
                    video_decoder: "auto".to_owned(),
                    pretrained_backbone: true,
                },
                architecture: StoredActArchitecture {
                    hidden_dim: 512,
                    latent_dim: 32,
                    layers: StoredActLayerCounts {
                        encoder: 4,
                        variational_encoder: 4,
                        decoder: 1,
                    },
                },
                attention: StoredActAttention {
                    attention_heads: 8,
                    feedforward_dim: 3_200,
                    kl_weight: 10.0,
                },
            },
            lifecycle: StoredTrainingLifecycle {
                phase,
                current_step: 12,
                started_ms: 1_700,
                finished_ms: Some(1_900),
                message: Some("done".to_owned()),
            },
            history: StoredTrainingRunHistory {
                metrics: vec![
                    StoredTrainingMetric {
                        progress: StoredTrainingProgress {
                            step: 2,
                            train_loss: 0.5,
                            validation_loss: None,
                            learning_rate: 1.0e-5,
                            elapsed_seconds: 0.0,
                            event: None,
                        },
                        resources: StoredTrainingResources {
                            transfer_bytes_per_second: 0.0,
                            examples_per_second: 0.0,
                            cpu_percent: None,
                            gpu_percent: None,
                            memory_bytes: None,
                        },
                        timings: StoredTrainingTimings {
                            loader_ms: Some(1.0),
                            arrow_encode_ms: None,
                            upload_ms: Some(2.0),
                            device_copy_ms: None,
                            model_ms: Some(3.0),
                        },
                    },
                    StoredTrainingMetric {
                        progress: StoredTrainingProgress {
                            step: 1,
                            train_loss: 0.7,
                            validation_loss: Some(0.8),
                            learning_rate: 1.0e-5,
                            elapsed_seconds: 4.0,
                            event: Some("validated".to_owned()),
                        },
                        resources: StoredTrainingResources {
                            transfer_bytes_per_second: 1.0,
                            examples_per_second: 2.0,
                            cpu_percent: Some(50.0),
                            gpu_percent: None,
                            memory_bytes: Some(1_024),
                        },
                        timings: StoredTrainingTimings {
                            loader_ms: None,
                            arrow_encode_ms: None,
                            upload_ms: None,
                            device_copy_ms: None,
                            model_ms: None,
                        },
                    },
                ],
                checkpoints: vec![StoredTrainingCheckpoint {
                    step: 20,
                    created_ms: 1_800,
                    validation_loss: Some(0.25),
                    byte_size: 4_096,
                    inference_ready: true,
                    recommended_action_steps: 10,
                    sample_hz: 30.0,
                }],
                logs: vec!["first".to_owned(), "second".to_owned()],
            },
        }
    }

    fn round_trip(history: &StoredTrainingHistory) -> StoredTrainingHistory {
        let [(_, runs), (_, metrics), (_, checkpoints)] = history.batches().unwrap();
        TrainingHistoryAssembly::new(
            (0..runs.num_rows())
                .map(|row| TrainingRunRow::decode(&runs, row).unwrap())
                .collect(),
            (0..metrics.num_rows())
                .map(|row| StoredTrainingMetric::decode(&metrics, row).unwrap())
                .collect(),
            (0..checkpoints.num_rows())
                .map(|row| StoredTrainingCheckpoint::decode(&checkpoints, row).unwrap())
                .collect(),
        )
        .finish()
    }

    fn populated_run() -> TrainingRun {
        let mut config = TrainingConfig::for_dataset("act-run", "pick-place", "SELECT 1");
        config.set_device("cpu");
        config.set_state_column("observation.state");
        config.set_action_column("action");
        config.set_steps(1_234);
        config.set_batch_size(16);
        config.set_validation_split(0.2);
        config.set_learning_rate(3.0e-5);
        config.set_checkpoint_every(500);
        config.set_chunk_size(64);
        config.set_seed(77);
        *config.act_mut() = ActTrainingConfig::new(
            ActInputConfig::new(vec!["wrist".to_owned()], (96, 72), "ffmpeg")
                .with_pretrained_backbone(false),
            ActArchitectureConfig::new(256, 16, ActLayerCounts::new(3, 2, 1)),
            ActAttentionConfig::new(4, 1_600, 5.0),
        );
        TrainingRun::starting("train-1", config)
            .with_source(TrainingSource::new("pick-place"))
            .with_phase(TrainingRunPhase::Completed, 900)
            .with_started_ms(1_700_000)
            .with_completion(Some(1_900_000), Some("done".to_owned()))
            .with_metrics(vec![
                TrainingMetricPoint::new(10, 0.5)
                    .with_learning_rate(3.0e-5)
                    .with_validation_loss(Some(0.6))
                    .with_timeline(12.5, Some("validated".to_owned()))
                    .with_throughput(1_024.0, 8.0)
                    .with_utilization(Some(55.5), None)
                    .with_memory_bytes(Some(2_048))
                    .with_loader_ms(Some(1.0))
                    .with_transport_timings(None, Some(3.0))
                    .with_runtime_timings(None, Some(5.0)),
                TrainingMetricPoint::new(20, 0.4).with_learning_rate(3.0e-5),
            ])
            .with_checkpoints(vec![
                TrainingCheckpoint::new(500, 1_800_000)
                    .with_validation_loss(Some(0.45))
                    .with_artifact(4_096, true)
                    .with_rollout(12, 30.0),
            ])
            .with_logs(vec!["started".to_owned(), "finished".to_owned()])
    }

    #[test]
    fn training_rows_round_trip_through_arrow_with_stable_run_order() {
        let history = StoredTrainingHistory {
            runs: vec![
                run("newest", StoredTrainingPhase::Running),
                run("oldest", StoredTrainingPhase::Completed),
            ],
        };
        let rows = history
            .batches()
            .unwrap()
            .map(|(table, batch)| (table, batch.num_rows()));
        assert_eq!(
            rows,
            [
                (Table::TrainingRuns, 2),
                (Table::TrainingRunMetrics, 4),
                (Table::TrainingRunCheckpoints, 2),
            ]
        );

        let restored = round_trip(&history);
        assert_eq!(
            restored
                .runs
                .iter()
                .map(|run| run.identity.run_id.as_str())
                .collect::<Vec<_>>(),
            ["newest", "oldest"]
        );
        // Metrics are written in insertion order and read back by step.
        let restored_steps = restored.runs[0]
            .history
            .metrics
            .iter()
            .map(|metric| metric.progress.step)
            .collect::<Vec<_>>();
        assert_eq!(restored_steps, [1, 2]);
        let expected = StoredTrainingHistory {
            runs: history
                .runs
                .iter()
                .cloned()
                .map(|mut run| {
                    run.history
                        .metrics
                        .sort_by_key(|metric| metric.progress.step);
                    run
                })
                .collect(),
        };
        assert_eq!(restored, expected);
    }

    #[test]
    fn an_empty_history_still_encodes_every_relation() {
        for (table, batch) in StoredTrainingHistory::default().batches().unwrap() {
            assert_eq!(batch.num_rows(), 0, "{table}");
        }
        assert_eq!(round_trip(&StoredTrainingHistory::default()).runs, []);
        // An empty wire snapshot projects onto no rows, and invents none back.
        let stored = StoredTrainingHistory::stored(&TrainingSnapshot::default());
        assert_eq!(stored.runs, []);
        assert_eq!(stored.restored(), []);
    }

    /// The persisted projection is total: a fully populated wire snapshot
    /// survives storage rows unchanged, non-default flags included. A field
    /// added to the contract without a mapping here fails this test.
    #[test]
    fn a_populated_snapshot_round_trips_through_storage_rows() {
        let bare = TrainingRun::starting("train-2", TrainingConfig::default())
            .with_source(TrainingSource::default())
            .with_started_ms(1_650_000);
        let snapshot =
            TrainingSnapshot::new(TrainingReadiness::default(), vec![populated_run(), bare]);

        let stored = StoredTrainingHistory::stored(&snapshot);
        assert_eq!(stored.runs.len(), 2);
        assert_eq!(stored.runs[0].identity.run_id, "train-1");
        assert_eq!(stored.runs[0].lifecycle.started_ms, 1_700_000);
        assert_eq!(stored.runs[0].lifecycle.finished_ms, Some(1_900_000));
        assert_eq!(stored.runs[0].identity.source_dataset_id, "pick-place");
        assert_eq!(
            stored.runs[0].history.metrics[0].progress.elapsed_seconds,
            12.5
        );
        assert_eq!(stored.runs[0].history.checkpoints[0].created_ms, 1_800_000);
        assert_eq!(stored.runs[0].history.checkpoints[0].byte_size, 4_096);
        assert!(!stored.runs[0].act.input.pretrained_backbone);

        assert_eq!(stored.restored().as_slice(), snapshot.runs());
        // The same snapshot also survives the Arrow relations themselves.
        assert_eq!(round_trip(&stored).restored().as_slice(), snapshot.runs());
    }

    /// Arrow holds non-finite floats natively, so a diverged run's NaN is
    /// stored verbatim: after a restart the browser sees exactly what the
    /// live stream showed, and no measurement is fabricated.
    #[test]
    fn a_diverged_metric_round_trips_as_nan_instead_of_becoming_a_measurement() {
        let run = TrainingRun::starting("train-nan", TrainingConfig::default())
            .with_source(TrainingSource::default())
            .with_phase(TrainingRunPhase::Failed, 1)
            .with_started_ms(5)
            .with_metrics(vec![
                TrainingMetricPoint::new(1, f64::NAN).with_learning_rate(1.0e-5),
            ]);
        let snapshot = TrainingSnapshot::new(TrainingReadiness::default(), vec![run]);

        let stored = StoredTrainingHistory::stored(&snapshot);
        assert!(
            stored.runs[0].history.metrics[0]
                .progress
                .train_loss
                .is_nan()
        );
        assert!(stored.restored()[0].metrics()[0].train_loss().is_nan());
        assert!(
            round_trip(&stored).runs[0].history.metrics[0]
                .progress
                .train_loss
                .is_nan()
        );
    }

    #[test]
    fn unknown_persisted_enumerations_are_rejected_rather_than_guessed() {
        assert!(StoredTrainingPhase::parse("running").is_ok());
        assert!(
            StoredTrainingPhase::parse("Running")
                .unwrap_err()
                .contains("not known")
        );
        assert!(StoredTrainingPolicy::parse("act").is_ok());
        assert!(StoredTrainingPolicy::parse("diffusion").is_err());
    }
}
