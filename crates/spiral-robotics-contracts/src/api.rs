//! The wire contract between the studio server and the web UI.
//!
//! Both sides depend on this module, so a change to a field name is a compile
//! error rather than a runtime surprise. Every type derives both halves of
//! serde because the server sends what the browser receives and vice versa.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::spiral::column as spiral_column;
use spiral_robotics_core::calibration::{Direction, MotorCalibration};
pub use spiral_robotics_core::operation::{
    CAUTION_FOLLOW_ERROR_DEGREES, CAUTION_LIMIT_FRACTION, CAUTION_VOLTAGE_MARGIN_VOLTS,
};

pub const DEFAULT_SPIRAL_ENDPOINT: &str = "https://api.spiraldb.dev";
pub const DEFAULT_SPIRAL_SPFS_ENDPOINT: &str = "https://spfs.spiraldb.dev";
use spiral_robotics_core::operation::{
    Lifecycle, MotorHealth, MotorHealthLimits, MotorHealthReadings, RobotCandidate,
    assess_motor_health, choose_teleoperation_pair, episode_is_usable,
};

/// The immutable Spiral destination plus project-owned Studio preferences.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DataSettings {
    pub spiral: SpiralSettings,
    #[serde(default)]
    pub training: TrainingRuntimeSettings,
    pub video: VideoSettings,
}

/// HTTP and Arrow IPC endpoint selected for future training sessions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingRuntimeSettings {
    pub endpoint: String,
}

impl Default for TrainingRuntimeSettings {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:50051".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralSettings {
    /// Spiral control-plane API endpoint.
    pub endpoint: String,
    /// Spiral File System endpoint used for relation and binary payloads.
    #[serde(default = "default_spiral_spfs_endpoint")]
    pub spfs_endpoint: String,
    pub project_id: String,
    /// Public half of the client-credentials pair used by the backend.
    #[serde(default)]
    pub client_id: String,
    /// The secret is write-only and never crosses back to the browser.
    #[serde(default)]
    pub client_secret_configured: bool,
    pub namespace: String,
    #[serde(default)]
    pub endpoint_restart_required: bool,
}

impl Default for SpiralSettings {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_SPIRAL_ENDPOINT.to_owned(),
            spfs_endpoint: DEFAULT_SPIRAL_SPFS_ENDPOINT.to_owned(),
            project_id: String::new(),
            client_id: String::new(),
            client_secret_configured: false,
            namespace: "spiral_robotics".to_owned(),
            endpoint_restart_required: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VideoSettings {
    pub enabled: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Update from the settings screen.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateDataSettingsRequest {
    /// Backend-local bootstrap pointer for the Spiral control-plane API.
    #[serde(default = "default_spiral_endpoint")]
    pub spiral_endpoint: String,
    /// Backend-local bootstrap pointer for Spiral File System payloads.
    #[serde(default = "default_spiral_spfs_endpoint")]
    pub spiral_spfs_endpoint: String,
    /// Backend-local bootstrap pointer used to select the one active Spiral
    /// project before project-owned settings can be read.
    #[serde(default)]
    pub project_id: String,
    /// OAuth client ID passed to SpiralDB as the ADBC database username.
    #[serde(default)]
    pub client_id: String,
    /// Write-only client secret. An empty value preserves the saved secret.
    #[serde(default)]
    pub client_secret: String,
    /// Explicitly remove a previously saved client secret.
    #[serde(default)]
    pub clear_client_secret: bool,
    #[serde(default)]
    pub training: TrainingRuntimeSettings,
    pub video: VideoSettings,
}

impl Default for UpdateDataSettingsRequest {
    fn default() -> Self {
        Self {
            spiral_endpoint: default_spiral_endpoint(),
            spiral_spfs_endpoint: default_spiral_spfs_endpoint(),
            project_id: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            clear_client_secret: false,
            training: TrainingRuntimeSettings::default(),
            video: VideoSettings::default(),
        }
    }
}

fn default_spiral_endpoint() -> String {
    DEFAULT_SPIRAL_ENDPOINT.to_owned()
}

fn default_spiral_spfs_endpoint() -> String {
    DEFAULT_SPIRAL_SPFS_ENDPOINT.to_owned()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionTestResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQueryRequest {
    pub sql: String,
    #[serde(default = "default_query_rows")]
    pub max_rows: usize,
}

fn default_query_rows() -> usize {
    100
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SpiralQueryResponse {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

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
            Self::Starting => Lifecycle::Pending,
            Self::Running => Lifecycle::Running,
            Self::Stopping => Lifecycle::Stopping,
            Self::Completed | Self::Stopped | Self::Failed => Lifecycle::Finished,
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
    spiral_column::STATE.to_owned()
}

fn default_training_action_column() -> String {
    spiral_column::ACTION.to_owned()
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

const MAX_TRAINING_METRIC_POINTS: usize = 2_400;
const TRAINING_METRIC_COMPACTION_TARGET: usize = MAX_TRAINING_METRIC_POINTS / 2;
const TRAINING_METRIC_SERIES_COUNT: usize = 12;
const MAX_TRAINING_LOG_LINES: usize = 120;
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

/// Operator-approved launch parameters for one backend-owned policy session.
pub use spiral_robotics_core::operation::{
    DEFAULT_ROLLOUT_DURATION_SECONDS, MAX_ROLLOUT_ACTION_STEPS, MAX_ROLLOUT_DURATION_SECONDS,
    MIN_ROLLOUT_DURATION_SECONDS,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartRolloutRequest {
    pub run_id: String,
    pub checkpoint_step: u64,
    pub robot: String,
    /// Time spent actively applying policy actions, excluding load and warm-up.
    #[serde(default = "default_rollout_duration_seconds")]
    pub duration_seconds: u32,
}

const fn default_rollout_duration_seconds() -> u32 {
    DEFAULT_ROLLOUT_DURATION_SECONDS
}

/// Authoritative lifecycle of the native inference and motor-control task.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutPhase {
    #[default]
    Idle,
    Loading,
    WarmingUp,
    Running,
    Stopping,
    Failed,
}

impl RolloutPhase {
    pub const fn active(self) -> bool {
        match self {
            Self::Loading => Lifecycle::Pending,
            Self::WarmingUp | Self::Running => Lifecycle::Running,
            Self::Stopping => Lifecycle::Stopping,
            Self::Idle | Self::Failed => Lifecycle::Finished,
        }
        .active()
    }
}

/// Measurements accumulated by the active native rollout session.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutMetrics {
    pub inference_count: u64,
    pub command_count: u64,
    pub missed_deadlines: u64,
    pub last_latency_ms: Option<f64>,
    pub last_model_ms: Option<f64>,
    pub mean_latency_ms: Option<f64>,
    pub max_latency_ms: Option<f64>,
}

/// Browser-safe state for rollout. Model bytes and device handles stay server-side.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutSnapshot {
    pub phase: RolloutPhase,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub checkpoint_step: Option<u64>,
    pub robot: Option<String>,
    #[serde(default)]
    pub duration_seconds: Option<u32>,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
    #[serde(default)]
    pub warning: Option<String>,
    pub metrics: RolloutMetrics,
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
                    run.current_step = run.current_step.max(point.step);
                    insert_training_metric(&mut run.metrics, *point);
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

fn insert_training_metric(metrics: &mut Vec<TrainingMetricPoint>, point: TrainingMetricPoint) {
    match metrics.binary_search_by_key(&point.step, |existing| existing.step) {
        Ok(index) => metrics[index] = point,
        Err(index) => metrics.insert(index, point),
    }
    if metrics.len() > MAX_TRAINING_METRIC_POINTS {
        compact_training_metrics(metrics);
    }
}

/// Keep a bounded, whole-run view instead of deleting the oldest steps.
///
/// Event samples, endpoints, and the extrema of every plotted metric survive
/// each compaction pass. Remaining capacity is filled uniformly so reconnecting
/// clients retain continuous temporal coverage while the newest interval stays
/// dense between passes.
fn compact_training_metrics(metrics: &mut Vec<TrainingMetricPoint>) {
    use std::collections::BTreeSet;

    if metrics.len() <= TRAINING_METRIC_COMPACTION_TARGET {
        return;
    }

    let source = std::mem::take(metrics);
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
            for series in 0..TRAINING_METRIC_SERIES_COUNT {
                let values = (bucket_start..bucket_end).filter_map(|index| {
                    training_metric_value(&source[index], series)
                        .filter(|value| value.is_finite())
                        .map(|value| (index, value))
                });
                let mut minimum = None::<(usize, f64)>;
                let mut maximum = None::<(usize, f64)>;
                for (index, value) in values {
                    if minimum.is_none_or(|(_, current)| value < current) {
                        minimum = Some((index, value));
                    }
                    if maximum.is_none_or(|(_, current)| value > current) {
                        maximum = Some((index, value));
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

    *metrics = source
        .into_iter()
        .enumerate()
        .filter_map(|(index, point)| keep.contains(&index).then_some(point))
        .collect();
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

fn training_metric_value(point: &TrainingMetricPoint, series: usize) -> Option<f64> {
    match series {
        0 => Some(point.train_loss),
        1 => point.validation_loss,
        2 => Some(point.transfer_bytes_per_second),
        3 => Some(point.examples_per_second),
        4 => point.cpu_percent,
        5 => point.gpu_percent,
        6 => point.memory_bytes.map(|value| value as f64),
        7 => point.loader_ms,
        8 => point.arrow_encode_ms,
        9 => point.upload_ms,
        10 => point.device_copy_ms,
        11 => point.model_ms,
        _ => None,
    }
}

/// Replay a trajectory fetched directly from SpiralDB.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartSpiralReplayRequest {
    pub episode: String,
    pub robot: String,
    pub stream: String,
}

/// Everything the UI needs to draw the setup screens.
///
/// This changes only when a robot, camera, or mode changes, so it is sent on
/// connect and then whenever it actually differs.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StudioSnapshot {
    /// Whether backend-owned services are still starting, usable, or running
    /// with a non-fatal bootstrap problem.
    #[serde(default)]
    pub readiness: BackendReadiness,
    /// Human-readable name of the active persistent store.
    pub storage: String,
    /// Every robot configuration found in SpiralDB, by ID.
    pub robots: Vec<RobotSummary>,
    /// Cameras the server has discovered.
    pub cameras: Vec<CameraStatus>,
    /// The currently selected operating mode.
    pub mode: ModeStatus,
    /// Problems worth showing the operator: invalid robot configurations, cameras
    /// that failed to probe, and similar non-fatal trouble.
    pub issues: Vec<String>,
}

/// Lifecycle of the backend services needed by the browser application.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendReadinessState {
    #[default]
    Bootstrapping,
    Ready,
    Degraded,
}

/// Browser-visible readiness of the backend and its Spiral catalog boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackendReadiness {
    pub state: BackendReadinessState,
    /// True only when catalog-dependent requests may safely be admitted.
    pub catalog_available: bool,
    /// Short operator-facing description of the current phase or problem.
    pub detail: String,
}

impl Default for BackendReadiness {
    fn default() -> Self {
        Self {
            state: BackendReadinessState::Bootstrapping,
            catalog_available: false,
            detail: "Starting Studio services".to_owned(),
        }
    }
}

/// One robot instance: exactly one arm, stored in SpiralDB.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RobotSummary {
    pub id: String,
    pub robot_type: Option<String>,
    pub port: Option<PortSettings>,
    pub motors: Vec<MotorSummary>,
    pub cameras: Vec<CameraBinding>,
    /// True when every motor has a stored calibration.
    pub calibrated: bool,
    /// Why this robot cannot be used, when its configuration is structurally wrong.
    ///
    /// A configuration can still describe an unusable robot — two
    /// motors sharing a bus ID, for instance. Carrying the reason here keeps
    /// the card from silently rendering as an empty robot.
    pub problem: Option<String>,
    pub connection: ConnectionStatus,
}

/// Result of reinterpreting historical measured state with a robot's current calibration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CalibrationReapplyResponse {
    pub episodes_updated: u64,
    pub frames_updated: u64,
    pub episodes_unchanged: u64,
    pub episodes_skipped: u64,
    pub warnings: Vec<String>,
}

/// Pick the best connected pair for a recording-led teleoperation session.
///
/// Robot type is a preference rather than a requirement: a configured leader
/// and follower win, while two otherwise ready compatible arms still work.
/// Compatibility is checked here so the Record button and the server make the
/// same decision instead of offering a pair the control thread must reject.
pub fn default_teleoperation_pair(robots: &[RobotSummary]) -> Option<(String, String)> {
    let candidates = robots
        .iter()
        .map(|robot| RobotCandidate {
            id: robot.id.clone(),
            robot_type: robot.robot_type.clone(),
            connected: robot.connection.connected,
            calibrated: robot.calibrated,
            valid: robot.problem.is_none(),
            joints: robot
                .motors
                .iter()
                .map(|motor| motor.name.clone())
                .collect(),
        })
        .collect::<Vec<_>>();
    choose_teleoperation_pair(&candidates)
}

/// A motor as configured, with its calibration when one exists.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MotorSummary {
    pub name: String,
    pub id: u32,
    #[serde(with = "direction_serde")]
    pub direction: Direction,
    pub model: Option<String>,
    #[serde(default, with = "motor_calibration_option_serde")]
    pub calibration: Option<MotorCalibration>,
    /// What this motor will fault at, read from its EEPROM when it connected.
    ///
    /// Lives on the snapshot rather than the telemetry frame because it does
    /// not change while an arm is running; resending it twenty times a second
    /// would be resending a constant.
    #[serde(default)]
    pub limits: MotorLimits,
}

mod direction_serde {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use spiral_robotics_core::calibration::Direction;

    pub(super) fn serialize<S>(direction: &Direction, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match direction {
            Direction::Normal => "normal",
            Direction::Reversed => "reversed",
        })
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Direction, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "normal" => Ok(Direction::Normal),
            "reversed" => Ok(Direction::Reversed),
            value => Err(de::Error::unknown_variant(value, &["normal", "reversed"])),
        }
    }
}

mod motor_calibration_option_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use spiral_robotics_core::calibration::{Direction, MotorCalibration};

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        id: u32,
        #[serde(with = "super::direction_serde")]
        direction: Direction,
        range_min: i64,
        center: i64,
        range_max: i64,
    }

    pub(super) fn serialize<S>(
        calibration: &Option<MotorCalibration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        calibration
            .map(|motor| Wire {
                id: motor.id,
                direction: motor.direction,
                range_min: motor.range_min,
                center: motor.center,
                range_max: motor.range_max,
            })
            .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<MotorCalibration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            Option::<Wire>::deserialize(deserializer)?.map(|wire| MotorCalibration {
                id: wire.id,
                direction: wire.direction,
                range_min: wire.range_min,
                center: wire.center,
                range_max: wire.range_max,
            }),
        )
    }
}

/// The limits a motor enforces on itself.
///
/// These are read from the hardware rather than written down here. A motor
/// already knows the temperature and voltage it will fault at, and a threshold
/// copied out of a datasheet is a threshold that can disagree with the motor in
/// front of the operator.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MotorLimits {
    pub max_temperature_celsius: Option<f64>,
    pub min_voltage_volts: Option<f64>,
    pub max_voltage_volts: Option<f64>,
    /// Configured torque ceiling as a fraction of the motor's rated torque.
    pub max_load: Option<f64>,
}

/// Serial settings for a robot's motor bus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortSettings {
    pub path: String,
    pub baud_rate: u32,
}

/// A camera assigned to a robot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CameraBinding {
    /// Role the camera plays for this robot, such as `wrist` or `top`.
    pub name: String,
    /// Stable device identifier reported by camera discovery.
    pub id: String,
    /// Where the camera is bolted, when the robot's file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<CameraMount>,
}

/// Where a camera sits and how much of the bench it can see.
///
/// Optional, and stays optional. A camera with no mount is a camera whose
/// position nobody has told the studio, and the honest thing to do with an
/// unknown position is not to draw one — a frustum at the origin would be a
/// claim about the hardware rather than a picture of it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CameraMount {
    /// Link in the robot description the camera rides, or `None` for one fixed
    /// to the bench. A wrist camera moves with the wrist; a top camera does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// Offset from that frame, in metres.
    #[serde(default)]
    pub xyz: [f64; 3],
    /// Roll, pitch, and yaw from that frame, in radians.
    #[serde(default)]
    pub rpy: [f64; 3],
    /// Horizontal field of view, in degrees.
    #[serde(default = "default_field_of_view")]
    pub fov_degrees: f64,
}

/// Field of view assumed for a mount that does not state one.
///
/// A middling webcam lens. It is a drawing default, not a measurement, and the
/// frustum it produces answers "is this pointed at the workspace" rather than
/// "what exactly is in shot".
fn default_field_of_view() -> f64 {
    62.0
}

/// Whether a robot's motor bus is currently open.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionStatus {
    pub connected: bool,
    /// Populated when the last connect or transfer attempt failed.
    pub error: Option<String>,
}

/// A camera the server knows about.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CameraStatus {
    /// Stable device identifier, also used in stream URLs.
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// True when a capture thread is running and serving frames.
    pub streaming: bool,
    /// Frames delivered since the stream opened.
    pub frames: u64,
    pub error: Option<String>,
}

/// A serial port offered as a candidate motor bus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SerialPortSummary {
    pub path: String,
    /// `usb`, `bluetooth`, `pci`, or `unknown`.
    pub kind: String,
    pub description: Option<String>,
    /// ID of the robot currently holding this port open, if any.
    pub used_by: Option<String>,
}

/// The operating mode the studio is in.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ModeSelection {
    /// Motors are read but never commanded.
    #[default]
    Idle,
    /// The UI commands one robot's joints directly.
    Manual { robot: String },
    /// A leader arm's measured pose is mirrored onto a follower arm.
    Teleoperate { leader: String, follower: String },
    /// A recorded episode's joint targets are played back onto one arm.
    ///
    /// The trajectory itself is not carried here: this type is the *published*
    /// mode, and a mode the browser can post is a mode the browser could post
    /// a hundred megabytes into. Replay is started through its own endpoint,
    /// which loads the episode server-side and hands the samples to the
    /// control thread directly.
    Replay { robot: String, episode: String },
    /// A backend-owned remote policy session commands one calibrated arm.
    Policy { robot: String, session: String },
}

impl ModeSelection {
    /// Human-readable name for status text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Manual { .. } => "Manual",
            Self::Teleoperate { .. } => "Teleoperation",
            Self::Replay { .. } => "Replay",
            Self::Policy { .. } => "Policy",
        }
    }
}

/// The selected mode plus whatever went wrong entering or running it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModeStatus {
    pub selection: ModeSelection,
    /// True once the mode is actually driving motors.
    pub active: bool,
    pub error: Option<String>,
}

/// A live sample of every connected robot.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TelemetryFrame {
    pub sequence: u64,
    /// Measured rate of the hardware control loop.
    pub loop_hz: f64,
    /// Requested rate of the hardware control loop.
    ///
    /// Keeping the target beside the measurement lets operators distinguish a
    /// deliberately slow configuration from a loop that cannot keep up with
    /// its motor buses.
    pub target_hz: f64,
    pub robots: Vec<RobotTelemetry>,
    /// Present only while a take is being captured.
    pub recording: Option<RecordingProgress>,
    /// Present only while an episode is being played back.
    pub replay: Option<ReplayProgress>,
}

/// One robot's live state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RobotTelemetry {
    pub id: String,
    pub torque: bool,
    pub joints: Vec<JointTelemetry>,
    pub error: Option<String>,
    /// Present only while a calibration sweep is running for this robot.
    pub calibration: Option<CalibrationProgress>,
}

/// One joint's live state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JointTelemetry {
    pub name: String,
    pub id: u32,
    /// Raw encoder reading, always available.
    pub raw: i64,
    /// Degrees from the calibrated center; `None` until the joint is calibrated.
    pub degrees: Option<f64>,
    /// Position within the calibrated travel, `-1.0` to `1.0`.
    pub normalized: Option<f64>,
    /// Last raw position commanded to this joint, when it is being driven.
    pub goal_raw: Option<i64>,
    /// Final calibrated target before the controller's safety ramp.
    pub target_raw: Option<i64>,
    /// Final target in degrees from the calibrated center.
    ///
    /// This is carried explicitly because encoder direction is part of the
    /// calibration and cannot be reconstructed from [`JointRange`] alone.
    pub target_degrees: Option<f64>,
    pub range: Option<JointRange>,

    /// What the motor reports about its own condition.
    ///
    /// Every one of these is optional and stays optional: a bus whose firmware
    /// will not answer the wider status read still drives the arm perfectly
    /// well, and a missing measurement reported as zero would put a cold motor
    /// and an unreadable one in the same cell. Unlike the fields above, none of
    /// these depend on calibration — they are the only readings an operator can
    /// get out of an arm that has never been calibrated.
    #[serde(default)]
    pub temperature_celsius: Option<f64>,
    #[serde(default)]
    pub voltage_volts: Option<f64>,
    #[serde(default)]
    pub current_amps: Option<f64>,
    /// Signed fraction of the motor's rated torque.
    #[serde(default)]
    pub load: Option<f64>,
    #[serde(default)]
    pub speed_degrees_per_second: Option<f64>,
    /// How fast this motor is heating, in degrees per minute.
    ///
    /// A temperature alone cannot be acted on: fifty-eight degrees is fine, or
    /// four minutes from a shutdown, depending entirely on which way it is
    /// going. Measured over a window rather than between frames, because these
    /// registers report whole degrees and a difference taken across two frames
    /// is quantization noise wearing a unit. `None` until the window fills.
    #[serde(default)]
    pub temperature_rise_per_minute: Option<f64>,
    /// Faults the motor is reporting about itself, not thresholds applied here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub faults: Vec<MotorFault>,
}

/// A fault a motor reports about itself.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorFault {
    Overheat,
    Overload,
    Voltage,
    /// A fault bit the motor set that this build does not have a name for.
    ///
    /// Carried rather than dropped: an operator who can see that something is
    /// wrong can act on it, and a fault silently swallowed because it was
    /// unfamiliar is the one that ends a session.
    Unrecognized,
}

impl MotorFault {
    /// How the studio says this on the page.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Overheat => "overheating",
            Self::Overload => "overloaded",
            Self::Voltage => "supply voltage",
            Self::Unrecognized => "motor fault",
        }
    }
}

/// How a motor is doing against its own limits.
///
/// Ordered worst-last so an arm's condition is the maximum of its motors'.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotorCondition {
    #[default]
    Nominal,
    Caution,
    Fault,
}

impl JointTelemetry {
    /// This motor's condition, judged against the limits it reported itself.
    ///
    /// A motor raising its own fault bit is a fault whatever the limits say —
    /// that is the hardware's verdict and it outranks any margin applied here.
    /// Without limits there is nothing to be near, so only the motor's own
    /// faults can speak.
    pub fn condition(&self, limits: &MotorLimits) -> MotorCondition {
        match assess_motor_health(
            MotorHealthReadings {
                faulted: !self.faults.is_empty(),
                temperature_celsius: self.temperature_celsius,
                voltage_volts: self.voltage_volts,
                load: self.load,
            },
            MotorHealthLimits {
                max_temperature_celsius: limits.max_temperature_celsius,
                min_voltage_volts: limits.min_voltage_volts,
                max_voltage_volts: limits.max_voltage_volts,
                max_load: limits.max_load,
            },
        ) {
            MotorHealth::Nominal => MotorCondition::Nominal,
            MotorHealth::Caution => MotorCondition::Caution,
            MotorHealth::Fault => MotorCondition::Fault,
        }
    }
}

/// The calibrated travel of one joint.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JointRange {
    pub min: i64,
    pub center: i64,
    pub max: i64,
}

/// Live progress of a calibration sweep.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CalibrationProgress {
    pub joints: Vec<CalibrationJointProgress>,
    /// True once every joint has moved far enough to record a range.
    pub ready: bool,
}

/// One joint's progress through its calibration sweep.
///
/// Carries its own name rather than relying on position, so the UI never has
/// to assume two independently built lists are in the same order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CalibrationJointProgress {
    pub name: String,
    pub min: i64,
    pub position: i64,
    pub max: i64,
}

impl CalibrationJointProgress {
    /// Travel observed so far, in encoder counts.
    pub fn span(&self) -> i64 {
        self.max.saturating_sub(self.min)
    }
}

/// One recorded episode, as summarised from SpiralDB.
///
/// An episode is one take: the operator arms the recorder, performs the task
/// once, and stops. Raw observations and actions are immutable; the operator's
/// judgement may change, and normalized measured state may be re-derived from
/// raw counts after the originating arm is recalibrated.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EpisodeSummary {
    /// Stable identifier for the episode and all of its related rows.
    pub id: String,
    /// Dataset row this episode belongs to. Streams and samples reference the
    /// episode; only the episode row carries dataset membership.
    #[serde(default = "default_dataset_id")]
    pub dataset_id: String,
    /// Arms sampled in this take, in the order their joints appear in a frame.
    pub arms: Vec<EpisodeArm>,
    pub frames: u64,
    pub duration_seconds: f64,
    /// Wall-clock intervals deliberately omitted from the episode timeline.
    ///
    /// Samples and recorded video use the active timeline, so pausing does not
    /// create a motionless training-data plateau. Each entry preserves where
    /// the omission happened and how much real time passed there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recording_gaps: Vec<RecordingGap>,
    /// Rate the take was captured at, so a replay can honour its timing.
    pub sample_hz: f64,
    /// When the take started, as milliseconds since the Unix epoch.
    ///
    /// Carried as a number rather than a formatted string so the client can
    /// render it in the operator's own locale and timezone.
    pub started_ms: i64,
    /// The mode that produced the take, in the studio's own words.
    pub source: String,
    /// Largest gap between a measured and a commanded joint angle during the
    /// take, in degrees — the same quantity the inspector shows live as
    /// `Follow Δ`, kept for the whole take instead of for one instant.
    pub max_follow_error_degrees: Option<f64>,
    /// True once the operator has judged this take unusable.
    ///
    /// A discarded take remains in SpiralDB and is excluded from training,
    /// because the judgement is the operator's and deleting on their behalf is not.
    pub discarded: bool,
    /// Why this committed take cannot currently be replayed or trained.
    pub problem: Option<String>,
}

impl EpisodeSummary {
    /// True when the take is complete enough to replay or export.
    pub fn usable(&self) -> bool {
        episode_is_usable(self.frames, self.problem.is_some())
    }

    /// The joints of the arm named, when this take sampled it.
    pub fn arm(&self, id: &str) -> Option<&EpisodeArm> {
        self.arms.iter().find(|arm| arm.id == id)
    }
}

/// Browser-owned episode catalog loaded on startup or explicit refresh.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EpisodeCatalogResponse {
    /// Every stored episode, oldest first.
    pub episodes: Vec<EpisodeSummary>,
    /// Catalog rows that could not be decoded.
    pub issues: Vec<String>,
}

/// One arm's presence in a take.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EpisodeArm {
    pub id: String,
    /// Stable hardware family/role captured with the episode, such as
    /// `so101_follower`. Training uses this instead of guessing from the ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robot_type: Option<String>,
    /// Joint names, in the order their values appear in every frame.
    pub joints: Vec<String>,
}

impl spiral_robotics_core::episode::ArmLayout for EpisodeArm {
    fn id(&self) -> &str {
        &self.id
    }

    fn joint_count(&self) -> usize {
        self.joints.len()
    }
}

/// A take being captured right now.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordingProgress {
    /// Identifier the finished take will have.
    pub episode: String,
    /// Logical dataset row this take will reference when it completes.
    #[serde(default = "default_dataset_id")]
    pub dataset_id: String,
    pub frames: u64,
    pub elapsed_seconds: f64,
    /// Real time since Record was pressed, including deliberate pauses.
    #[serde(default)]
    pub wall_elapsed_seconds: f64,
    /// Real time omitted from the active episode timeline so far.
    #[serde(default)]
    pub paused_seconds: f64,
    /// Whether samples and camera frames are currently being omitted.
    #[serde(default)]
    pub paused: bool,
    /// Arms being sampled.
    pub arms: Vec<String>,
    /// Largest `Follow Δ` seen so far in this take, in degrees.
    pub max_follow_error_degrees: Option<f64>,
    /// Set when the capture stopped itself — a full disk, or an arm that went
    /// away mid-take.
    pub error: Option<String>,
}

/// A deliberate pause in an otherwise continuous recording session.
///
/// `after_seconds` is expressed on the gap-free active timeline. This keeps
/// replay and dataset sampling contiguous while retaining enough provenance to
/// display or audit every omitted wall-clock interval.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct RecordingGap {
    pub after_seconds: f64,
    pub duration_seconds: f64,
}

/// Why a replay trajectory is loaded.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPurpose {
    /// User-facing episode transport controlled by Play/Pause.
    #[default]
    Timeline,
    /// One committed Timeline scrub moving the robot to a single pose.
    PhysicalSeek,
}

/// Whether a replay is positioning the arm or advancing through the episode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPhase {
    Seeking,
    /// Default preserves the behavior of progress sent by an older backend.
    #[default]
    Playing,
}

/// An episode being played back right now.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReplayProgress {
    pub episode: String,
    /// The arm being driven.
    pub robot: String,
    /// Frames played so far.
    pub frame: u64,
    pub frames: u64,
    pub elapsed_seconds: f64,
    pub duration_seconds: f64,
    /// Whether this is Timeline transport or a one-pose physical seek.
    #[serde(default)]
    pub purpose: ReplayPurpose,
    /// Positioning never advances the episode clock or recorded media.
    #[serde(default)]
    pub phase: ReplayPhase,
    /// Whether playback is holding its current frame without discarding the
    /// loaded trajectory. Resuming continues from this exact position.
    #[serde(default)]
    pub paused: bool,
}

/// Begin capturing a take.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartRecordingRequest {
    /// Dataset row that will own the completed episode.
    #[serde(default = "default_dataset_id")]
    pub dataset_id: String,
    /// Arms to sample. Empty means every connected arm, which is the useful
    /// default: a take is a record of the whole scene, not of one arm.
    #[serde(default)]
    pub arms: Vec<String>,
    /// A short operator note stored with the take.
    #[serde(default)]
    pub note: Option<String>,
}

impl Default for StartRecordingRequest {
    fn default() -> Self {
        Self {
            dataset_id: default_dataset_id(),
            arms: Vec::new(),
            note: None,
        }
    }
}

/// Stable fallback for old recordings and clients created before datasets
/// became first-class rows.
pub fn default_dataset_id() -> String {
    "default".to_owned()
}

/// Finish the take in progress.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopRecordingRequest {
    /// False marks the finished take discarded in the same gesture that ends
    /// it, for the common case of knowing the take was bad while it ran.
    pub keep: bool,
}

/// Pause or resume the take in progress without changing its episode ID.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetRecordingPausedRequest {
    pub paused: bool,
}

/// Pause or resume a replay without unloading its trajectory.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetReplayPausedRequest {
    pub paused: bool,
}

/// Change the operator's judgement of a stored take.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct JudgeEpisodeRequest {
    pub discarded: bool,
}

/// Play a stored episode back onto one arm.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StartReplayRequest {
    pub episode: String,
    /// The arm to drive. It must be connected and calibrated.
    pub robot: String,
    /// Which recorded arm's trajectory to play, when the take holds more than
    /// one. Defaults to the arm being driven, then to the take's only arm.
    #[serde(default)]
    pub source_arm: Option<String>,
    /// Episode-clock position where replay should begin. Older clients omit
    /// this and retain the original frame-zero behavior.
    #[serde(default)]
    pub start_seconds: f64,
}

/// What replacing the active SpiralDB project from a portable snapshot imported.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectImportSummary {
    pub relations: u64,
    pub rows: u64,
    /// Non-transactional derived resources that still need repair after the
    /// durable relational replacement committed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// What merging a portable episode bundle into one dataset imported.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EpisodeImportSummary {
    pub dataset_id: String,
    pub episodes: u64,
    pub rows: u64,
    pub first_episode: Option<String>,
    /// Non-transactional derived resources that still need repair after the
    /// durable episode rows committed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Create a new robot instance in SpiralDB.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateRobotRequest {
    pub id: String,
    /// Defaults to `so101_follower` when omitted.
    #[serde(default)]
    pub robot_type: Option<String>,
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub baud_rate: Option<u32>,
}

/// Change a robot's port or camera assignments.
///
/// Omitted fields are left exactly as they are in the stored configuration,
/// including fields this build does not understand.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct UpdateRobotRequest {
    #[serde(default)]
    pub robot_type: Option<String>,
    #[serde(default)]
    pub port: Option<PortSettings>,
    #[serde(default)]
    pub cameras: Option<Vec<CameraBinding>>,
}

/// Command joint positions in normalized `[-1.0, 1.0]` space.
///
/// Normalized commands are the only ones the API accepts: they are meaningless
/// without a calibration, which is exactly the safety property wanted here.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GoalRequest {
    pub normalized: BTreeMap<String, f64>,
}

/// Identify a motor bus by comparing the ports present before and after a cable
/// is unplugged.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentifyPortRequest {
    /// Port paths observed before the cable was removed.
    pub before: Vec<String>,
}

/// The outcome of a port-identification attempt.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentifyPortResponse {
    /// The single port that disappeared, when exactly one did.
    pub port: Option<String>,
    /// Every port that disappeared, for the ambiguous case.
    pub removed: Vec<String>,
    pub message: String,
}

/// A failed request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiError {
    pub error: String,
}

impl ApiError {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn act_defaults_match_current_lerobot_architecture() {
        let config = ActTrainingConfig::default();
        assert_eq!(config.hidden_dim, 512);
        assert_eq!(config.feedforward_dim, 3_200);
        assert_eq!(config.vae_encoder_layers, 4);
        assert_eq!(config.decoder_layers, 1);
        assert!(config.camera_streams.is_empty());
        let request = StartTrainingRequest::default();
        assert_eq!(request.batch_size, 8);
        assert_eq!(request.steps, 100_000);
        assert_eq!(request.checkpoint_every, 20_000);
        assert_eq!(request.seed, 1_000);
        assert_eq!(request.validation_split, 0.1);
    }

    #[test]
    fn historical_training_requests_keep_no_validation_semantics() {
        let mut value = serde_json::to_value(StartTrainingRequest::default()).unwrap();
        value.as_object_mut().unwrap().remove("validation_split");
        let request: StartTrainingRequest = serde_json::from_value(value).unwrap();
        assert_eq!(request.validation_split, 0.0);
    }

    #[test]
    fn older_rollout_requests_receive_the_safe_default_duration() {
        let request: StartRolloutRequest = serde_json::from_value(serde_json::json!({
            "run_id": "run-1",
            "checkpoint_step": 20_000,
            "robot": "follower"
        }))
        .expect("duration defaults for an older client");

        assert_eq!(request.duration_seconds, DEFAULT_ROLLOUT_DURATION_SECONDS);
    }

    #[test]
    fn motor_summary_adapts_domain_calibration_at_the_contract_edge() {
        let summary = MotorSummary {
            name: "joint".to_owned(),
            id: 7,
            direction: Direction::Reversed,
            model: Some("sts3215".to_owned()),
            calibration: Some(MotorCalibration {
                id: 7,
                direction: Direction::Reversed,
                range_min: 100,
                center: 200,
                range_max: 300,
            }),
            limits: MotorLimits::default(),
        };

        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["direction"], "reversed");
        assert_eq!(json["calibration"]["direction"], "reversed");
        assert_eq!(
            serde_json::from_value::<MotorSummary>(json).unwrap(),
            summary
        );
    }

    fn robot(id: &str, robot_type: Option<&str>) -> RobotSummary {
        RobotSummary {
            id: id.to_owned(),
            robot_type: robot_type.map(str::to_owned),
            port: None,
            motors: vec![MotorSummary {
                name: "shoulder".to_owned(),
                id: 1,
                direction: Direction::Normal,
                model: None,
                calibration: None,
                limits: MotorLimits::default(),
            }],
            cameras: Vec::new(),
            calibrated: true,
            problem: None,
            connection: ConnectionStatus {
                connected: true,
                error: None,
            },
        }
    }

    #[test]
    fn teleoperation_pair_prefers_configured_roles_over_list_order() {
        let robots = [
            robot("follower", Some("so101_follower")),
            robot("spare", None),
            robot("leader", Some("SO101_LEADER")),
        ];

        assert_eq!(
            default_teleoperation_pair(&robots),
            Some(("leader".to_owned(), "follower".to_owned()))
        );
    }

    #[test]
    fn teleoperation_pair_falls_back_to_the_first_compatible_ready_arms() {
        let robots = [robot("left", None), robot("right", None)];

        assert_eq!(
            default_teleoperation_pair(&robots),
            Some(("left".to_owned(), "right".to_owned()))
        );
    }

    #[test]
    fn teleoperation_pair_ignores_unready_and_incompatible_arms() {
        let mut disconnected = robot("disconnected-leader", Some("leader"));
        disconnected.connection.connected = false;
        let mut incompatible = robot("incompatible-leader", Some("leader"));
        incompatible.motors[0].name = "elbow".to_owned();
        let follower = robot("follower", Some("follower"));
        let fallback = robot("fallback", None);

        assert_eq!(
            default_teleoperation_pair(&[disconnected, incompatible, follower, fallback.clone(),]),
            Some((fallback.id, "follower".to_owned()))
        );
    }

    #[test]
    fn mode_selection_round_trips_through_its_tag() {
        let cases = [
            (ModeSelection::Idle, r#"{"mode":"idle"}"#),
            (
                ModeSelection::Manual {
                    robot: "arm".to_owned(),
                },
                r#"{"mode":"manual","robot":"arm"}"#,
            ),
            (
                ModeSelection::Teleoperate {
                    leader: "left".to_owned(),
                    follower: "right".to_owned(),
                },
                r#"{"mode":"teleoperate","leader":"left","follower":"right"}"#,
            ),
            (
                ModeSelection::Replay {
                    robot: "right".to_owned(),
                    episode: "episode_000007".to_owned(),
                },
                r#"{"mode":"replay","robot":"right","episode":"episode_000007"}"#,
            ),
            (
                ModeSelection::Policy {
                    robot: "right".to_owned(),
                    session: "rollout-1".to_owned(),
                },
                r#"{"mode":"policy","robot":"right","session":"rollout-1"}"#,
            ),
        ];

        for (mode, json) in cases {
            assert_eq!(serde_json::to_string(&mode).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<ModeSelection>(json).unwrap(),
                mode,
                "{json} should round-trip"
            );
        }
    }

    #[test]
    fn optional_request_fields_may_be_omitted() {
        let created: CreateRobotRequest = serde_json::from_str(r#"{"id":"arm"}"#).unwrap();
        assert_eq!(created.id, "arm");
        assert_eq!(created.robot_type, None);

        let update: UpdateRobotRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(update, UpdateRobotRequest::default());

        let settings: UpdateDataSettingsRequest = serde_json::from_str(
            r#"{
                "video": { "enabled": true }
            }"#,
        )
        .unwrap();
        assert_eq!(settings.spiral_endpoint, DEFAULT_SPIRAL_ENDPOINT);
        assert_eq!(settings.spiral_spfs_endpoint, DEFAULT_SPIRAL_SPFS_ENDPOINT);
        assert_eq!(settings.training, TrainingRuntimeSettings::default());
    }

    #[test]
    fn older_replay_progress_defaults_to_timeline_playback() {
        let replay: ReplayProgress = serde_json::from_str(
            r#"{
                "episode":"episode_000001",
                "robot":"follower",
                "frame":12,
                "frames":60,
                "elapsed_seconds":0.2,
                "duration_seconds":1.0
            }"#,
        )
        .unwrap();

        assert_eq!(replay.purpose, ReplayPurpose::Timeline);
        assert_eq!(replay.phase, ReplayPhase::Playing);
        assert!(!replay.paused);
    }

    #[test]
    fn an_unfinished_take_is_listed_but_not_offered() {
        let mut episode = EpisodeSummary {
            id: "episode_000001".to_owned(),
            dataset_id: "default".to_owned(),
            arms: vec![EpisodeArm {
                id: "follower".to_owned(),
                robot_type: Some("so101_follower".to_owned()),
                joints: vec!["shoulder_pan".to_owned()],
            }],
            frames: 120,
            duration_seconds: 2.0,
            recording_gaps: Vec::new(),
            sample_hz: 60.0,
            started_ms: 1_770_000_000_000,
            source: "Teleoperation".to_owned(),
            max_follow_error_degrees: Some(3.5),
            discarded: false,
            problem: None,
        };
        assert!(episode.usable());
        assert_eq!(episode.arm("follower").map(|arm| arm.joints.len()), Some(1));
        assert_eq!(episode.arm("leader"), None);

        episode.problem = Some("recording did not finish".to_owned());
        assert!(!episode.usable(), "a truncated take must not be offered");

        // An empty take is unusable even without a recorded reason: there is
        // nothing in it to replay.
        episode.problem = None;
        episode.frames = 0;
        assert!(!episode.usable());
    }

    #[test]
    fn calibration_span_saturates_instead_of_overflowing() {
        let progress = CalibrationJointProgress {
            name: "gripper".to_owned(),
            min: i64::MIN,
            position: 0,
            max: i64::MAX,
        };
        assert_eq!(progress.span(), i64::MAX);
    }

    fn joint() -> JointTelemetry {
        JointTelemetry {
            name: "shoulder_pan".to_owned(),
            id: 1,
            raw: 2048,
            degrees: None,
            normalized: None,
            goal_raw: None,
            target_raw: None,
            target_degrees: None,
            range: None,
            temperature_celsius: Some(30.0),
            voltage_volts: Some(7.4),
            current_amps: Some(0.2),
            load: Some(0.1),
            speed_degrees_per_second: Some(0.0),
            temperature_rise_per_minute: Some(0.0),
            faults: Vec::new(),
        }
    }

    fn limits() -> MotorLimits {
        MotorLimits {
            max_temperature_celsius: Some(70.0),
            min_voltage_volts: Some(4.5),
            max_voltage_volts: Some(9.0),
            max_load: Some(1.0),
        }
    }

    #[test]
    fn a_motor_within_its_own_limits_is_nominal() {
        assert_eq!(joint().condition(&limits()), MotorCondition::Nominal);
    }

    #[test]
    fn nearing_a_limit_is_caution_and_the_motors_own_verdict_is_fault() {
        let hot = JointTelemetry {
            temperature_celsius: Some(70.0 * CAUTION_LIMIT_FRACTION),
            ..joint()
        };
        assert_eq!(hot.condition(&limits()), MotorCondition::Caution);

        let strained = JointTelemetry {
            load: Some(-0.9),
            ..joint()
        };
        assert_eq!(
            strained.condition(&limits()),
            MotorCondition::Caution,
            "load is judged by how hard it is pushing, not which way"
        );

        let sagging = JointTelemetry {
            voltage_volts: Some(4.5 + CAUTION_VOLTAGE_MARGIN_VOLTS - 0.1),
            ..joint()
        };
        assert_eq!(sagging.condition(&limits()), MotorCondition::Caution);

        // The motor's own fault byte outranks every margin applied here.
        let faulted = JointTelemetry {
            faults: vec![MotorFault::Overheat],
            ..joint()
        };
        assert_eq!(faulted.condition(&limits()), MotorCondition::Fault);
    }

    #[test]
    fn without_limits_only_the_motors_own_faults_can_speak() {
        let scorching = JointTelemetry {
            temperature_celsius: Some(200.0),
            ..joint()
        };

        assert_eq!(
            scorching.condition(&MotorLimits::default()),
            MotorCondition::Nominal,
            "there is nothing to be near, and a threshold invented here would be a guess"
        );
        assert_eq!(
            JointTelemetry {
                faults: vec![MotorFault::Unrecognized],
                ..scorching
            }
            .condition(&MotorLimits::default()),
            MotorCondition::Fault
        );
    }

    #[test]
    fn an_arms_condition_is_the_worst_of_its_motors() {
        assert_eq!(
            [
                MotorCondition::Nominal,
                MotorCondition::Fault,
                MotorCondition::Caution
            ]
            .into_iter()
            .max(),
            Some(MotorCondition::Fault)
        );
    }

    #[test]
    fn incremental_training_updates_stay_bounded_without_dropping_early_history() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "run-a".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };
        for step in 0..=12_000 {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(TrainingMetricPoint {
                    step,
                    train_loss: if step == 123 { 10_000.0 } else { 1.0 },
                    validation_loss: (step == 6_000).then_some(4.0),
                    cpu_percent: (step == 777).then_some(9_000.0),
                    ..TrainingMetricPoint::default()
                }),
            });
        }
        for line in 0..=MAX_TRAINING_LOG_LINES {
            snapshot.apply(TrainingUpdate::Log {
                run_id: "run-a".to_owned(),
                line: format!("line {line}"),
            });
        }

        let run = &snapshot.runs[0];
        assert!(run.metrics.len() <= MAX_TRAINING_METRIC_POINTS);
        assert_eq!(run.metrics.first().map(|point| point.step), Some(0));
        assert_eq!(run.metrics.last().map(|point| point.step), Some(12_000));
        assert!(run.metrics.iter().any(|point| point.step == 123));
        assert!(run.metrics.iter().any(|point| point.step == 777));
        assert!(run.metrics.iter().any(|point| point.step == 6_000));
        assert_eq!(run.logs.len(), MAX_TRAINING_LOG_LINES);
        assert_eq!(run.logs[0], "line 1");
        assert_eq!(run.current_step, 12_000);
    }

    #[test]
    fn repeated_or_late_metric_updates_replace_the_step_without_regressing_progress() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "run-a".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };
        for (step, loss) in [(2, 2.0), (1, 1.0), (2, 0.5)] {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(TrainingMetricPoint {
                    step,
                    train_loss: loss,
                    ..TrainingMetricPoint::default()
                }),
            });
        }

        let run = &snapshot.runs[0];
        assert_eq!(run.current_step, 2);
        assert_eq!(run.metrics.len(), 2);
        assert_eq!(run.metrics[0].step, 1);
        assert_eq!(run.metrics[1].step, 2);
        assert_eq!(run.metrics[1].train_loss, 0.5);
    }

    #[test]
    fn resolved_training_configuration_updates_the_run_snapshot() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "run-a".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };
        let mut config = StartTrainingRequest::default();
        config.act.camera_streams = vec![
            "follower.camera.gripper".to_owned(),
            "follower.camera.top".to_owned(),
        ];
        snapshot.apply(TrainingUpdate::RunConfigured {
            run_id: "run-a".to_owned(),
            config: Box::new(config.clone()),
        });
        assert_eq!(snapshot.runs[0].config, config);
    }

    #[test]
    fn removing_a_training_run_removes_its_complete_snapshot_record() {
        let mut snapshot = TrainingSnapshot {
            runs: vec![
                TrainingRun {
                    id: "run-a".to_owned(),
                    metrics: vec![TrainingMetricPoint::default()],
                    checkpoints: vec![TrainingCheckpoint::default()],
                    logs: vec!["retained only inside the run".to_owned()],
                    ..TrainingRun::default()
                },
                TrainingRun {
                    id: "run-b".to_owned(),
                    ..TrainingRun::default()
                },
            ],
            ..TrainingSnapshot::default()
        };

        snapshot.apply(TrainingUpdate::RunRemoved {
            run_id: "run-a".to_owned(),
        });

        assert_eq!(snapshot.runs.len(), 1);
        assert_eq!(snapshot.runs[0].id, "run-b");
    }

    #[test]
    fn bootstrap_training_snapshot_replaces_the_placeholder_atomically() {
        let mut snapshot = TrainingSnapshot::default();
        let restored = TrainingSnapshot {
            runs: vec![TrainingRun {
                id: "restored-run".to_owned(),
                ..TrainingRun::default()
            }],
            ..TrainingSnapshot::default()
        };

        snapshot.apply(TrainingUpdate::Snapshot {
            snapshot: Box::new(restored.clone()),
        });

        assert_eq!(snapshot, restored);
    }
}
