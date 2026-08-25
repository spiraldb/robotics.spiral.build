//! Native policy-training contracts and bounded run history.

use serde::{Deserialize, Serialize};
use spiral_robotics_core::operation::OperationLifecycle;

use crate::datasets::default_dataset_id;

/// The policy family executed by the native training runtime.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingPolicy {
    #[default]
    Act,
}

impl TrainingPolicy {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Act => "ACT · vision",
        }
    }
}

/// Model-specific ACT settings kept together so checkpoints remain
/// self-describing as the training UI grows.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActTrainingConfig {
    /// Camera stream IDs that must be present for every sampled anchor.
    /// An empty list discovers every camera stream shared by the selected
    /// dataset episodes, matching LeRobot's feature-driven policy inputs.
    #[serde(default = "default_training_camera_streams")]
    pub camera_streams: Vec<String>,
    /// Requested image width. Zero preserves the recorded camera width.
    #[serde(default = "default_training_image_size")]
    pub image_width: usize,
    /// Requested image height. Zero preserves the recorded camera height.
    #[serde(default = "default_training_image_size")]
    pub image_height: usize,
    /// Spiral video decoder implementation: auto, ffmpeg, videotoolbox, or nvdec.
    #[serde(default = "default_training_video_decoder")]
    pub video_decoder: String,
    /// Use the ImageNet initialization expected by vision ACT.
    #[serde(default = "default_act_pretrained_backbone")]
    pub pretrained_backbone: bool,
    #[serde(default = "default_act_hidden_dim")]
    pub hidden_dim: usize,
    #[serde(default = "default_act_latent_dim")]
    pub latent_dim: usize,
    #[serde(default = "default_act_encoder_layers")]
    pub encoder_layers: usize,
    #[serde(default = "default_act_vae_encoder_layers")]
    pub vae_encoder_layers: usize,
    #[serde(default = "default_act_decoder_layers")]
    pub decoder_layers: usize,
    #[serde(default = "default_act_attention_heads")]
    pub attention_heads: usize,
    #[serde(default = "default_act_feedforward_dim")]
    pub feedforward_dim: usize,
    #[serde(default = "default_act_kl_weight")]
    pub kl_weight: f64,
}

impl Default for ActTrainingConfig {
    fn default() -> Self {
        Self {
            camera_streams: default_training_camera_streams(),
            image_width: default_training_image_size(),
            image_height: default_training_image_size(),
            video_decoder: default_training_video_decoder(),
            pretrained_backbone: default_act_pretrained_backbone(),
            hidden_dim: default_act_hidden_dim(),
            latent_dim: default_act_latent_dim(),
            encoder_layers: default_act_encoder_layers(),
            vae_encoder_layers: default_act_vae_encoder_layers(),
            decoder_layers: default_act_decoder_layers(),
            attention_heads: default_act_attention_heads(),
            feedforward_dim: default_act_feedforward_dim(),
            kl_weight: default_act_kl_weight(),
        }
    }
}

/// Authoritative lifecycle of one backend-owned native training run.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingRunPhase {
    #[default]
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

impl TrainingRunPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Completed => "Completed",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
        }
    }

    pub const fn active(self) -> bool {
        match self {
            Self::Starting => OperationLifecycle::Pending,
            Self::Running => OperationLifecycle::Running,
            Self::Stopping => OperationLifecycle::Stopping,
            Self::Completed | Self::Stopped | Self::Failed => OperationLifecycle::Finished,
        }
        .active()
    }
}

/// Everything needed to reproduce one run. Spiral is the only sample source.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StartTrainingRequest {
    pub run_name: String,
    /// Relational dataset selected by the operator. The configured Spiral
    /// namespace remains a storage location; this ID is the training set.
    #[serde(default = "default_dataset_id")]
    pub dataset_id: String,
    #[serde(default)]
    pub policy: TrainingPolicy,
    pub sql: String,
    #[serde(default = "default_training_state_column")]
    pub state_column: String,
    #[serde(default = "default_training_action_column")]
    pub action_column: String,
    #[serde(default = "default_training_device")]
    pub device: String,
    #[serde(default = "default_training_steps")]
    pub steps: u64,
    #[serde(default = "default_training_batch_size")]
    pub batch_size: usize,
    /// Fraction of covered episodes held out from optimizer updates.
    ///
    /// Missing values deserialize to zero so historical run records keep their
    /// original no-validation semantics. Newly created requests default to 10%.
    #[serde(default)]
    pub validation_split: f64,
    #[serde(default = "default_training_learning_rate")]
    pub learning_rate: f64,
    #[serde(default = "default_training_checkpoint_every")]
    pub checkpoint_every: u64,
    #[serde(default = "default_training_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_training_seed")]
    pub seed: u64,
    #[serde(default)]
    pub act: ActTrainingConfig,
}

impl Default for StartTrainingRequest {
    fn default() -> Self {
        Self {
            run_name: "act-run".to_owned(),
            dataset_id: default_dataset_id(),
            policy: TrainingPolicy::Act,
            sql: String::new(),
            state_column: default_training_state_column(),
            action_column: default_training_action_column(),
            device: default_training_device(),
            steps: default_training_steps(),
            batch_size: default_training_batch_size(),
            validation_split: default_training_validation_split(),
            learning_rate: default_training_learning_rate(),
            checkpoint_every: default_training_checkpoint_every(),
            chunk_size: default_training_chunk_size(),
            seed: default_training_seed(),
            act: ActTrainingConfig::default(),
        }
    }
}

fn default_training_state_column() -> String {
    "state".to_owned()
}

fn default_training_action_column() -> String {
    "action".to_owned()
}

fn default_training_device() -> String {
    "auto".to_owned()
}

const fn default_training_steps() -> u64 {
    100_000
}

const fn default_training_batch_size() -> usize {
    8
}

const fn default_training_validation_split() -> f64 {
    0.1
}

const fn default_training_learning_rate() -> f64 {
    1.0e-5
}

const fn default_training_checkpoint_every() -> u64 {
    20_000
}

const fn default_training_chunk_size() -> usize {
    100
}

const fn default_training_seed() -> u64 {
    1_000
}

fn default_training_camera_streams() -> Vec<String> {
    Vec::new()
}

const fn default_training_image_size() -> usize {
    0
}

fn default_training_video_decoder() -> String {
    "auto".to_owned()
}

const fn default_act_pretrained_backbone() -> bool {
    true
}

const fn default_act_hidden_dim() -> usize {
    512
}

const fn default_act_latent_dim() -> usize {
    32
}

const fn default_act_encoder_layers() -> usize {
    4
}

const fn default_act_vae_encoder_layers() -> usize {
    4
}

const fn default_act_decoder_layers() -> usize {
    // The original ACT requested seven decoder layers but returned only the
    // first layer's output. LeRobot preserves that effective architecture.
    1
}

const fn default_act_attention_heads() -> usize {
    8
}

const fn default_act_feedforward_dim() -> usize {
    3_200
}

const fn default_act_kl_weight() -> f64 {
    10.0
}

/// One sample on the shared training-step axis used by every Studio plot.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingMetricPoint {
    pub step: u64,
    pub train_loss: f64,
    pub validation_loss: Option<f64>,
    pub learning_rate: f64,
    /// Latest requested video payload bytes divided by total sample preparation time.
    pub transfer_bytes_per_second: f64,
    pub examples_per_second: f64,
    /// Studio process CPU usage; 100% represents one fully occupied core.
    #[serde(default)]
    pub cpu_percent: Option<f64>,
    /// Whole-device GPU utilization when the active backend exposes it.
    #[serde(default)]
    pub gpu_percent: Option<f64>,
    /// Current resident memory used by the Studio process.
    #[serde(default)]
    pub memory_bytes: Option<u64>,
    /// Spiral query, frame decode, normalization, and batch assembly time.
    #[serde(default)]
    pub loader_ms: Option<f64>,
    /// Studio-side Arrow IPC materialization time.
    #[serde(default)]
    pub arrow_encode_ms: Option<f64>,
    /// HTTP request/response time outside the remote runtime handler.
    #[serde(default)]
    pub upload_ms: Option<f64>,
    /// Runtime-side conversion and host-to-device transfer time.
    #[serde(default)]
    pub device_copy_ms: Option<f64>,
    /// Runtime-side optimizer or validation compute time.
    #[serde(default)]
    pub model_ms: Option<f64>,
    pub elapsed_seconds: f64,
    #[serde(default)]
    pub event: Option<String>,
}

/// An inference checkpoint durably ingested into Spiral.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingCheckpoint {
    pub step: u64,
    pub created_ms: i64,
    pub validation_loss: Option<f64>,
    pub byte_size: u64,
    /// Old run-history entries default to false and cannot be selected for rollout.
    #[serde(default)]
    pub inference_ready: bool,
    /// Number of actions to execute before refreshing the observation.
    #[serde(default)]
    pub recommended_action_steps: usize,
    /// Control frequency inherited from the recorded dataset.
    #[serde(default)]
    pub sample_hz: f64,
}

/// Dataset selection captured when a run starts. Project and namespace come
/// from Studio's one process-owned Spiral destination.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingSource {
    /// Relational dataset row whose episodes supplied this run.
    #[serde(default = "default_dataset_id")]
    pub dataset_id: String,
}

/// Result of checking every dependency needed before a native run can start.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingReadiness {
    pub ready: bool,
    pub spiral_ready: bool,
    #[serde(default)]
    pub runtime: Option<TrainingRuntimeInfo>,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingRuntimeInfo {
    pub id: String,
    pub display_name: String,
    pub runtime: String,
    pub devices: Vec<String>,
}

impl Default for TrainingReadiness {
    fn default() -> Self {
        Self {
            ready: false,
            spiral_ready: false,
            runtime: None,
            summary: "Checking the training runtime…".to_owned(),
        }
    }
}

/// Browser-safe run state. Checkpoint bytes and task handles never cross the
/// wire; only their durable Spiral identity does.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRun {
    pub id: String,
    pub config: StartTrainingRequest,
    #[serde(default)]
    pub source: TrainingSource,
    pub phase: TrainingRunPhase,
    pub current_step: u64,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
    pub metrics: Vec<TrainingMetricPoint>,
    pub checkpoints: Vec<TrainingCheckpoint>,
    pub logs: Vec<String>,
}

/// Complete training state published through the Studio event stream.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingSnapshot {
    #[serde(default)]
    pub readiness: TrainingReadiness,
    pub runs: Vec<TrainingRun>,
}

pub(crate) const MAX_TRAINING_METRIC_POINTS: usize = 2_400;
const TRAINING_METRIC_COMPACTION_TARGET: usize = MAX_TRAINING_METRIC_POINTS / 2;
const TRAINING_METRIC_SERIES_COUNT: usize = 12;
pub(crate) const MAX_TRAINING_LOG_LINES: usize = 120;
const MAX_TRAINING_CHECKPOINTS: usize = 1_000;
pub const MAX_TRAINING_HISTORY_RUNS: usize = 40;

/// Incremental training event sent after the initial snapshot.
///
/// Metrics are intentionally one point at a time: republishing every retained
/// run for every optimizer step made event bandwidth grow with history.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrainingUpdate {
    /// Replace the bootstrap placeholder with the state restored from SpiralDB.
    Snapshot {
        snapshot: Box<TrainingSnapshot>,
    },
    Readiness {
        readiness: TrainingReadiness,
    },
    RunAdded {
        run: Box<TrainingRun>,
    },
    RunRemoved {
        run_id: String,
    },
    RunConfigured {
        run_id: String,
        config: Box<StartTrainingRequest>,
    },
    RunState {
        run_id: String,
        phase: TrainingRunPhase,
        current_step: u64,
        finished_ms: Option<i64>,
        message: Option<String>,
    },
    Metric {
        run_id: String,
        point: Box<TrainingMetricPoint>,
    },
    Checkpoint {
        run_id: String,
        checkpoint: TrainingCheckpoint,
    },
    Log {
        run_id: String,
        line: String,
    },
}

impl TrainingSnapshot {
    pub fn active_run(&self) -> Option<&TrainingRun> {
        self.runs.iter().find(|run| run.phase.active())
    }

    pub fn apply(&mut self, update: TrainingUpdate) {
        match update {
            TrainingUpdate::Snapshot { snapshot } => *self = *snapshot,
            TrainingUpdate::Readiness { readiness } => self.readiness = readiness,
            TrainingUpdate::RunAdded { run } => {
                self.runs.retain(|existing| existing.id != run.id);
                self.runs.insert(0, *run);
                self.runs.truncate(MAX_TRAINING_HISTORY_RUNS);
            }
            TrainingUpdate::RunRemoved { run_id } => {
                self.runs.retain(|run| run.id != run_id);
            }
            TrainingUpdate::RunConfigured { run_id, config } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.config = *config;
                }
            }
            TrainingUpdate::RunState {
                run_id,
                phase,
                current_step,
                finished_ms,
                message,
            } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.phase = phase;
                    run.current_step = current_step;
                    run.finished_ms = finished_ms;
                    run.message = message;
                }
            }
            TrainingUpdate::Metric { run_id, point } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.insert_metric(*point);
                }
            }
            TrainingUpdate::Checkpoint { run_id, checkpoint } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.current_step = run.current_step.max(checkpoint.step);
                    run.checkpoints.push(checkpoint);
                    let excess = run
                        .checkpoints
                        .len()
                        .saturating_sub(MAX_TRAINING_CHECKPOINTS);
                    if excess > 0 {
                        run.checkpoints.drain(..excess);
                    }
                }
            }
            TrainingUpdate::Log { run_id, line } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
                    run.logs.push(line);
                    let excess = run.logs.len().saturating_sub(MAX_TRAINING_LOG_LINES);
                    if excess > 0 {
                        run.logs.drain(..excess);
                    }
                }
            }
        }
    }
}

impl TrainingRun {
    fn insert_metric(&mut self, point: TrainingMetricPoint) {
        self.current_step = self.current_step.max(point.step);
        match self
            .metrics
            .binary_search_by_key(&point.step, |existing| existing.step)
        {
            Ok(index) => self.metrics[index] = point,
            Err(index) => self.metrics.insert(index, point),
        }
        if self.metrics.len() > MAX_TRAINING_METRIC_POINTS {
            self.compact_metrics();
        }
    }

    /// Keep a bounded, whole-run view instead of deleting the oldest steps.
    ///
    /// Event samples, endpoints, and the extrema of every plotted metric survive
    /// each compaction pass. Remaining capacity is filled uniformly so reconnecting
    /// clients retain continuous temporal coverage while the newest interval stays
    /// dense between passes.
    fn compact_metrics(&mut self) {
        use std::collections::BTreeSet;

        if self.metrics.len() <= TRAINING_METRIC_COMPACTION_TARGET {
            return;
        }

        let source = std::mem::take(&mut self.metrics);
        let mut keep = BTreeSet::from([0, source.len() - 1]);
        let event_indices = source
            .iter()
            .enumerate()
            .filter_map(|(index, point)| point.event.is_some().then_some(index))
            .collect::<Vec<_>>();
        retain_evenly(
            &mut keep,
            &event_indices,
            TRAINING_METRIC_COMPACTION_TARGET / 8,
        );

        let extrema_capacity = TRAINING_METRIC_COMPACTION_TARGET.saturating_sub(keep.len());
        let bucket_count = extrema_capacity / (TRAINING_METRIC_SERIES_COUNT * 2);
        if bucket_count > 0 {
            let bucket_size = source.len().div_ceil(bucket_count);
            for bucket_start in (0..source.len()).step_by(bucket_size) {
                let bucket_end = (bucket_start + bucket_size).min(source.len());
                for series_index in 0..TRAINING_METRIC_SERIES_COUNT {
                    let values = (bucket_start..bucket_end).filter_map(|index| {
                        source[index]
                            .series_value(series_index)
                            .filter(|metric_value| metric_value.is_finite())
                            .map(|metric_value| (index, metric_value))
                    });
                    let mut minimum = None::<(usize, f64)>;
                    let mut maximum = None::<(usize, f64)>;
                    for (index, metric_value) in values {
                        if minimum.is_none_or(|(_, current)| metric_value < current) {
                            minimum = Some((index, metric_value));
                        }
                        if maximum.is_none_or(|(_, current)| metric_value > current) {
                            maximum = Some((index, metric_value));
                        }
                    }
                    if let Some((index, _)) = minimum {
                        keep.insert(index);
                    }
                    if let Some((index, _)) = maximum {
                        keep.insert(index);
                    }
                }
            }
        }

        let remaining = TRAINING_METRIC_COMPACTION_TARGET.saturating_sub(keep.len());
        let all_indices = (0..source.len()).collect::<Vec<_>>();
        retain_evenly(&mut keep, &all_indices, remaining);

        self.metrics = source
            .into_iter()
            .enumerate()
            .filter_map(|(index, point)| keep.contains(&index).then_some(point))
            .collect();
    }
}

fn retain_evenly(keep: &mut std::collections::BTreeSet<usize>, candidates: &[usize], limit: usize) {
    if candidates.is_empty() || limit == 0 {
        return;
    }
    if candidates.len() <= limit {
        keep.extend(candidates.iter().copied());
        return;
    }
    for slot in 0..limit {
        let index = slot * (candidates.len() - 1) / limit.saturating_sub(1).max(1);
        keep.insert(candidates[index]);
    }
}

impl TrainingMetricPoint {
    fn series_value(&self, series_index: usize) -> Option<f64> {
        match series_index {
            0 => Some(self.train_loss),
            1 => self.validation_loss,
            2 => Some(self.transfer_bytes_per_second),
            3 => Some(self.examples_per_second),
            4 => self.cpu_percent,
            5 => self.gpu_percent,
            6 => self.memory_bytes.map(|memory_bytes| memory_bytes as f64),
            7 => self.loader_ms,
            8 => self.arrow_encode_ms,
            9 => self.upload_ms,
            10 => self.device_copy_ms,
            11 => self.model_ms,
            _ => None,
        }
    }
}
