//! Native policy-training contracts: one run configuration, and what a
//! worker reports about the runs it holds.

use serde::{Deserialize, Serialize};

/// The policy family executed by the native training runtime.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingPolicy {
    #[default]
    Act,
}

impl TrainingPolicy {
    pub const fn mcap_training_blocker(self) -> Option<&'static str> {
        match self {
            Self::Act => Some(
                "MCAP camera training is unavailable until Spiral supports decoding MCAP video into training images.",
            ),
        }
    }

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
    #[serde(flatten)]
    input: ActInputConfig,
    #[serde(flatten)]
    architecture: ActArchitectureConfig,
    #[serde(flatten)]
    attention: ActAttentionConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActInputConfig {
    /// Camera stream IDs that must be present for every sampled anchor.
    /// An empty list discovers every camera stream shared by the selected
    /// dataset episodes, matching LeRobot's feature-driven policy inputs.
    camera_streams: Vec<String>,
    /// Requested image width. Zero preserves the recorded camera width.
    image_width: usize,
    /// Requested image height. Zero preserves the recorded camera height.
    image_height: usize,
    /// Spiral video decoder implementation: auto, ffmpeg, videotoolbox, or nvdec.
    video_decoder: String,
    /// Use the ImageNet initialization expected by vision ACT.
    pretrained_backbone: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActArchitectureConfig {
    hidden_dim: usize,
    latent_dim: usize,
    encoder_layers: usize,
    vae_encoder_layers: usize,
    decoder_layers: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActLayerCounts {
    encoder: usize,
    variational_encoder: usize,
    decoder: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActAttentionConfig {
    attention_heads: usize,
    feedforward_dim: usize,
    kl_weight: f64,
}

impl ActInputConfig {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        camera_streams: Vec<String>,
        image_size: (usize, usize),
        video_decoder: impl Into<String>,
    ) -> Self {
        Self {
            camera_streams,
            image_width: image_size.0,
            image_height: image_size.1,
            video_decoder: video_decoder.into(),
            pretrained_backbone: true,
        }
    }
}

impl ActArchitectureConfig {
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new(hidden_dim: usize, latent_dim: usize, layers: ActLayerCounts) -> Self {
        Self {
            hidden_dim,
            latent_dim,
            encoder_layers: layers.encoder,
            vae_encoder_layers: layers.variational_encoder,
            decoder_layers: layers.decoder,
        }
    }
}

impl ActAttentionConfig {
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new(attention_heads: usize, feedforward_dim: usize, kl_weight: f64) -> Self {
        Self {
            attention_heads,
            feedforward_dim,
            kl_weight,
        }
    }
}

impl Default for ActTrainingConfig {
    fn default() -> Self {
        Self {
            input: ActInputConfig {
                camera_streams: Vec::new(),
                image_width: 0,
                image_height: 0,
                video_decoder: "auto".to_owned(),
                pretrained_backbone: true,
            },
            architecture: ActArchitectureConfig {
                hidden_dim: 512,
                latent_dim: 32,
                encoder_layers: 4,
                vae_encoder_layers: 4,
                // The original ACT requested seven decoder layers but returned
                // only the first layer's output. LeRobot preserves that
                // effective architecture.
                decoder_layers: 1,
            },
            attention: ActAttentionConfig {
                attention_heads: 8,
                feedforward_dim: 3_200,
                kl_weight: 10.0,
            },
        }
    }
}

impl ActTrainingConfig {
    pub fn set_camera_streams(&mut self, streams: Vec<String>) {
        self.input.camera_streams = streams;
    }
    pub const fn set_image_width(&mut self, width: usize) {
        self.input.image_width = width;
    }
    pub const fn set_image_height(&mut self, height: usize) {
        self.input.image_height = height;
    }
    pub fn camera_streams(&self) -> &[String] {
        &self.input.camera_streams
    }
    pub const fn image_width(&self) -> usize {
        self.input.image_width
    }
    pub const fn image_height(&self) -> usize {
        self.input.image_height
    }
    pub fn video_decoder(&self) -> &str {
        &self.input.video_decoder
    }
    pub const fn hidden_dim(&self) -> usize {
        self.architecture.hidden_dim
    }
    pub const fn latent_dim(&self) -> usize {
        self.architecture.latent_dim
    }
    pub const fn encoder_layers(&self) -> usize {
        self.architecture.encoder_layers
    }
    pub const fn vae_encoder_layers(&self) -> usize {
        self.architecture.vae_encoder_layers
    }
    pub const fn decoder_layers(&self) -> usize {
        self.architecture.decoder_layers
    }
    pub const fn attention_heads(&self) -> usize {
        self.attention.attention_heads
    }
    pub const fn feedforward_dim(&self) -> usize {
        self.attention.feedforward_dim
    }
    pub const fn kl_weight(&self) -> f64 {
        self.attention.kl_weight
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        input: ActInputConfig,
        architecture: ActArchitectureConfig,
        attention: ActAttentionConfig,
    ) -> Self {
        Self {
            input,
            architecture,
            attention,
        }
    }
}

/// Lifecycle of one training run, as its worker reports it.
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
    /// The run's worker no longer holds it, or cannot be reached.
    Unavailable,
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
            Self::Unavailable => "Unavailable",
        }
    }

    pub const fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// Everything needed to reproduce one run. Spiral is the only sample source.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrainingConfig {
    #[serde(flatten)]
    selection: TrainingDataSelection,
    #[serde(flatten)]
    optimization: TrainingOptimizationConfig,
    act: ActTrainingConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TrainingDataSelection {
    run_name: String,
    /// Relational dataset selected by the operator. The configured Spiral
    /// namespace remains a storage location; this ID is the training set.
    dataset_id: String,
    policy: TrainingPolicy,
    sql: String,
    state_column: String,
    action_column: String,
    device: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct TrainingOptimizationConfig {
    steps: u64,
    batch_size: usize,
    /// Fraction of covered episodes held out from optimizer updates.
    validation_split: f64,
    learning_rate: f64,
    checkpoint_every: u64,
    chunk_size: usize,
    seed: u64,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            selection: TrainingDataSelection::new("act-run", "default", ""),
            optimization: TrainingOptimizationConfig {
                steps: 100_000,
                batch_size: 8,
                validation_split: 0.1,
                learning_rate: 1.0e-5,
                checkpoint_every: 20_000,
                chunk_size: 100,
                seed: 1_000,
            },
            act: ActTrainingConfig::default(),
        }
    }
}

impl TrainingConfig {
    pub fn for_dataset(
        run_name: impl Into<String>,
        dataset_id: impl Into<String>,
        sql: impl Into<String>,
    ) -> Self {
        let defaults = Self::default();
        Self {
            selection: TrainingDataSelection::new(run_name, dataset_id, sql),
            ..defaults
        }
    }
    pub fn run_name(&self) -> &str {
        &self.selection.run_name
    }
    pub fn dataset_id(&self) -> &str {
        &self.selection.dataset_id
    }
    pub const fn policy(&self) -> TrainingPolicy {
        self.selection.policy
    }
    pub fn sql(&self) -> &str {
        &self.selection.sql
    }
    pub fn state_column(&self) -> &str {
        &self.selection.state_column
    }
    pub fn action_column(&self) -> &str {
        &self.selection.action_column
    }
    pub fn device(&self) -> &str {
        &self.selection.device
    }
    pub const fn steps(&self) -> u64 {
        self.optimization.steps
    }
    pub const fn batch_size(&self) -> usize {
        self.optimization.batch_size
    }
    pub const fn validation_split(&self) -> f64 {
        self.optimization.validation_split
    }
    pub const fn learning_rate(&self) -> f64 {
        self.optimization.learning_rate
    }
    pub const fn checkpoint_every(&self) -> u64 {
        self.optimization.checkpoint_every
    }
    pub const fn chunk_size(&self) -> usize {
        self.optimization.chunk_size
    }
    pub const fn act(&self) -> &ActTrainingConfig {
        &self.act
    }
    pub fn act_mut(&mut self) -> &mut ActTrainingConfig {
        &mut self.act
    }

    pub fn set_run_name(&mut self, run_name: impl Into<String>) {
        self.selection.run_name = run_name.into();
    }
    pub fn set_dataset_id(&mut self, dataset_id: impl Into<String>) {
        self.selection.dataset_id = dataset_id.into();
    }
    pub fn set_sql(&mut self, sql: impl Into<String>) {
        self.selection.sql = sql.into();
    }
    pub fn set_device(&mut self, device: impl Into<String>) {
        self.selection.device = device.into();
    }
    pub fn set_state_column(&mut self, column: impl Into<String>) {
        self.selection.state_column = column.into();
    }
    pub fn set_action_column(&mut self, column: impl Into<String>) {
        self.selection.action_column = column.into();
    }
    pub const fn set_steps(&mut self, steps: u64) {
        self.optimization.steps = steps;
    }
    pub const fn set_batch_size(&mut self, batch_size: usize) {
        self.optimization.batch_size = batch_size;
    }
    pub const fn set_validation_split(&mut self, split: f64) {
        self.optimization.validation_split = split;
    }
    pub const fn set_learning_rate(&mut self, learning_rate: f64) {
        self.optimization.learning_rate = learning_rate;
    }
    pub const fn set_checkpoint_every(&mut self, checkpoint_every: u64) {
        self.optimization.checkpoint_every = checkpoint_every;
    }
    pub const fn set_chunk_size(&mut self, chunk_size: usize) {
        self.optimization.chunk_size = chunk_size;
    }
}

impl TrainingDataSelection {
    fn new(
        run_name: impl Into<String>,
        dataset_id: impl Into<String>,
        sql: impl Into<String>,
    ) -> Self {
        Self {
            run_name: run_name.into(),
            dataset_id: dataset_id.into(),
            policy: TrainingPolicy::default(),
            sql: sql.into(),
            state_column: "state".to_owned(),
            action_column: "action".to_owned(),
            device: "auto".to_owned(),
        }
    }
}

/// One sample on the shared training-step axis, exactly as the worker reports it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingMetricPoint {
    #[serde(flatten)]
    progress: TrainingMetricProgress,
    #[serde(flatten)]
    resources: TrainingResourceMetrics,
    #[serde(flatten)]
    timings: TrainingStageTimings,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingMetricProgress {
    step: u64,
    train_loss: f64,
    validation_loss: Option<f64>,
    learning_rate: f64,
    elapsed_seconds: f64,
    event: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingResourceMetrics {
    examples_per_second: f64,
    /// Worker process CPU usage; 100% represents one fully occupied core.
    cpu_percent: Option<f64>,
    /// Whole-device GPU utilization when the worker's device exposes it.
    gpu_percent: Option<f64>,
    /// Current resident memory used by the worker process.
    memory_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingStageTimings {
    /// Spiral query, frame decode, normalization, and batch assembly time.
    loader_ms: Option<f64>,
    /// Conversion and host-to-device transfer time.
    device_copy_ms: Option<f64>,
    /// Optimizer or validation compute time.
    model_ms: Option<f64>,
}

impl TrainingMetricPoint {
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new(step: u64, train_loss: f64) -> Self {
        Self {
            progress: TrainingMetricProgress {
                step,
                train_loss,
                validation_loss: None,
                learning_rate: 0.0,
                elapsed_seconds: 0.0,
                event: None,
            },
            resources: TrainingResourceMetrics {
                examples_per_second: 0.0,
                cpu_percent: None,
                gpu_percent: None,
                memory_bytes: None,
            },
            timings: TrainingStageTimings {
                loader_ms: None,
                device_copy_ms: None,
                model_ms: None,
            },
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_validation_loss(mut self, validation_loss: Option<f64>) -> Self {
        self.progress.validation_loss = validation_loss;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_event(mut self, event: impl Into<String>) -> Self {
        self.progress.event = Some(event.into());
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_examples_per_second(mut self, examples_per_second: f64) -> Self {
        self.resources.examples_per_second = examples_per_second;
        self
    }

    pub const fn step(&self) -> u64 {
        self.progress.step
    }
    pub const fn train_loss(&self) -> f64 {
        self.progress.train_loss
    }
    pub const fn validation_loss(&self) -> Option<f64> {
        self.progress.validation_loss
    }
    pub const fn learning_rate(&self) -> f64 {
        self.progress.learning_rate
    }
    pub const fn examples_per_second(&self) -> f64 {
        self.resources.examples_per_second
    }
    pub const fn cpu_percent(&self) -> Option<f64> {
        self.resources.cpu_percent
    }
    pub const fn gpu_percent(&self) -> Option<f64> {
        self.resources.gpu_percent
    }
    pub const fn memory_bytes(&self) -> Option<u64> {
        self.resources.memory_bytes
    }
    pub const fn loader_ms(&self) -> Option<f64> {
        self.timings.loader_ms
    }
    pub const fn device_copy_ms(&self) -> Option<f64> {
        self.timings.device_copy_ms
    }
    pub const fn model_ms(&self) -> Option<f64> {
        self.timings.model_ms
    }
    pub fn event(&self) -> Option<&str> {
        self.progress.event.as_deref()
    }
}

/// An inference checkpoint the worker committed to Spiral.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingCheckpoint {
    run_id: String,
    step: u64,
    validation_loss: Option<f64>,
    policy: CheckpointPolicy,
}

impl TrainingCheckpoint {
    pub fn new(run_id: impl Into<String>, step: u64) -> Self {
        Self {
            run_id: run_id.into(),
            step,
            validation_loss: None,
            policy: CheckpointPolicy::default(),
        }
    }
    pub const fn with_validation_loss(mut self, validation_loss: Option<f64>) -> Self {
        self.validation_loss = validation_loss;
        self
    }
    pub fn with_policy(mut self, policy: CheckpointPolicy) -> Self {
        self.policy = policy;
        self
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub const fn step(&self) -> u64 {
        self.step
    }
    pub const fn validation_loss(&self) -> Option<f64> {
        self.validation_loss
    }
    pub const fn policy(&self) -> &CheckpointPolicy {
        &self.policy
    }
}

/// What a checkpoint's policy expects from the robot it drives.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CheckpointPolicy {
    /// Camera streams, in the order the policy reads them.
    camera_streams: Vec<String>,
    /// Actions executed before the observation is refreshed.
    action_steps: usize,
    /// Control frequency of the training data; checkpoints written before it
    /// was recorded do not carry it.
    sample_hz: Option<f64>,
}

impl CheckpointPolicy {
    pub fn new(camera_streams: Vec<String>, action_steps: usize, sample_hz: Option<f64>) -> Self {
        Self {
            camera_streams,
            action_steps,
            sample_hz,
        }
    }
    pub fn camera_streams(&self) -> &[String] {
        &self.camera_streams
    }
    pub const fn action_steps(&self) -> usize {
        self.action_steps
    }
    pub const fn sample_hz(&self) -> Option<f64> {
        self.sample_hz
    }
}

/// Result of checking every dependency needed before a native run can start.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingReadiness {
    ready: bool,
    spiral_ready: bool,
    runtime: Option<TrainingRuntimeInfo>,
    summary: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingRuntimeInfo {
    display_name: String,
    runtime: String,
    devices: Vec<String>,
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

impl TrainingReadiness {
    pub fn new(ready: bool, spiral_ready: bool, summary: impl Into<String>) -> Self {
        Self {
            ready,
            spiral_ready,
            runtime: None,
            summary: summary.into(),
        }
    }
    pub fn with_runtime(mut self, runtime: Option<TrainingRuntimeInfo>) -> Self {
        self.runtime = runtime;
        self
    }
    pub const fn ready(&self) -> bool {
        self.ready
    }
    pub const fn spiral_ready(&self) -> bool {
        self.spiral_ready
    }
    pub const fn runtime(&self) -> Option<&TrainingRuntimeInfo> {
        self.runtime.as_ref()
    }
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

impl TrainingRuntimeInfo {
    pub fn new(display_name: impl Into<String>, runtime: impl Into<String>) -> Self {
        Self {
            display_name: display_name.into(),
            runtime: runtime.into(),
            devices: Vec::new(),
        }
    }
    pub fn with_devices(mut self, devices: Vec<String>) -> Self {
        self.devices = devices;
        self
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn runtime(&self) -> &str {
        &self.runtime
    }
    pub fn devices(&self) -> &[String] {
        &self.devices
    }
}

/// One run as Studio shows it: what it was asked to do, and what its worker
/// reports now.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRun {
    #[serde(flatten)]
    identity: TrainingRunIdentity,
    #[serde(flatten)]
    status: TrainingRunStatus,
    #[serde(flatten)]
    history: TrainingRunHistory,
}

/// What a run was asked to do, fixed when it starts.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRunIdentity {
    id: String,
    config: TrainingConfig,
    started_ms: i64,
}

/// Where a run is now.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRunStatus {
    phase: TrainingRunPhase,
    current_step: u64,
    message: Option<String>,
}

/// The metrics and log tail a worker retains for one run.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRunHistory {
    metrics: Vec<TrainingMetricPoint>,
    logs: Vec<String>,
}

impl TrainingRun {
    pub fn new(identity: TrainingRunIdentity, status: TrainingRunStatus) -> Self {
        Self {
            identity,
            status,
            history: TrainingRunHistory::default(),
        }
    }

    pub fn with_history(mut self, history: TrainingRunHistory) -> Self {
        self.history = history;
        self
    }

    pub fn id(&self) -> &str {
        &self.identity.id
    }
    pub const fn config(&self) -> &TrainingConfig {
        &self.identity.config
    }
    pub const fn phase(&self) -> TrainingRunPhase {
        self.status.phase
    }
    pub const fn current_step(&self) -> u64 {
        self.status.current_step
    }
    pub fn message(&self) -> Option<&str> {
        self.status.message.as_deref()
    }
    pub fn metrics(&self) -> &[TrainingMetricPoint] {
        &self.history.metrics
    }
    pub fn logs(&self) -> &[String] {
        &self.history.logs
    }
}

impl TrainingRunIdentity {
    pub fn new(id: impl Into<String>, config: TrainingConfig, started_ms: i64) -> Self {
        Self {
            id: id.into(),
            config,
            started_ms,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn started_ms(&self) -> i64 {
        self.started_ms
    }
}

impl TrainingRunStatus {
    pub fn new(phase: TrainingRunPhase, current_step: u64, message: Option<String>) -> Self {
        Self {
            phase,
            current_step,
            message,
        }
    }

    /// A run whose worker cannot say where it is, and why.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::new(TrainingRunPhase::Unavailable, 0, Some(reason.into()))
    }
}

impl TrainingRunHistory {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(metrics: Vec<TrainingMetricPoint>, logs: Vec<String>) -> Self {
        Self { metrics, logs }
    }
}

/// Every run of a project and whether a new one could start now.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingSnapshot {
    readiness: TrainingReadiness,
    runs: Vec<TrainingRun>,
}

impl TrainingSnapshot {
    pub fn new(readiness: TrainingReadiness, runs: Vec<TrainingRun>) -> Self {
        Self { readiness, runs }
    }
    pub const fn readiness(&self) -> &TrainingReadiness {
        &self.readiness
    }
    pub fn runs(&self) -> &[TrainingRun] {
        &self.runs
    }
    pub fn active_run(&self) -> Option<&TrainingRun> {
        self.runs.iter().find(|run| run.phase().active())
    }
}

impl ActLayerCounts {
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new(encoder: usize, variational_encoder: usize, decoder: usize) -> Self {
        Self {
            encoder,
            variational_encoder,
            decoder,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn act_defaults_match_current_lerobot_architecture() {
        let config = ActTrainingConfig::default();
        assert_eq!(config.hidden_dim(), 512);
        assert_eq!(config.feedforward_dim(), 3_200);
        assert_eq!(config.vae_encoder_layers(), 4);
        assert_eq!(config.decoder_layers(), 1);
        assert!(config.camera_streams().is_empty());
        let config = TrainingConfig::default();
        assert_eq!(config.batch_size(), 8);
        assert_eq!(config.steps(), 100_000);
        assert_eq!(config.checkpoint_every(), 20_000);
        assert_eq!(config.validation_split(), 0.1);
    }
}
