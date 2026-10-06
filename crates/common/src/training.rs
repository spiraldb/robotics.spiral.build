//! Policy training: what a run trains on, how its model sees the data, and
//! what a run reports. See `docs/training.md`.

use serde::{Deserialize, Serialize};

use crate::datasets::DatasetSelection;

/// The policy family a run trains.
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

/// What a run trains on: one published curated view of one dataset.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrainingSource {
    dataset: DatasetSelection,
    view: String,
}

impl TrainingSource {
    pub fn new(dataset: DatasetSelection, view: impl Into<String>) -> Self {
        Self {
            dataset,
            view: view.into(),
        }
    }
    pub const fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub fn view(&self) -> &str {
        &self.view
    }
}

/// How the model sees the data: which cameras, at what size, on what clock.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrainingObservation {
    /// Video topics, in the order the policy reads them.
    cameras: Vec<String>,
    /// The size every camera's frames are resized to.
    image: ImageSize,
    /// The training clock: ticks per second over each kept interval.
    rate_hz: f64,
}

impl Default for TrainingObservation {
    fn default() -> Self {
        Self {
            cameras: Vec::new(),
            image: ImageSize::new(320, 240),
            rate_hz: 30.0,
        }
    }
}

impl TrainingObservation {
    pub fn cameras(&self) -> &[String] {
        &self.cameras
    }
    pub const fn image(&self) -> ImageSize {
        self.image
    }
    pub const fn rate_hz(&self) -> f64 {
        self.rate_hz
    }
    pub fn set_cameras(&mut self, cameras: Vec<String>) {
        self.cameras = cameras;
    }
    pub const fn set_image(&mut self, image: ImageSize) {
        self.image = image;
    }
    pub const fn set_rate_hz(&mut self, rate_hz: f64) {
        self.rate_hz = rate_hz;
    }
}

/// A frame size in pixels.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImageSize {
    width: u32,
    height: u32,
}

impl ImageSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
    pub const fn width(self) -> u32 {
        self.width
    }
    pub const fn height(self) -> u32 {
        self.height
    }
    /// This size divided by `divisor` on both sides, at least one pixel.
    pub const fn divided(self, divisor: u32) -> Self {
        let width = self.width / divisor;
        let height = self.height / divisor;
        Self::new(
            if width == 0 { 1 } else { width },
            if height == 0 { 1 } else { height },
        )
    }
}

/// How the optimizer runs.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrainingOptimizationConfig {
    steps: u64,
    batch_size: usize,
    /// Fraction of episodes held out from optimizer updates.
    validation_split: f64,
    learning_rate: f64,
    checkpoint_every: u64,
    /// Actions predicted per observation.
    chunk_size: usize,
    seed: u64,
}

impl Default for TrainingOptimizationConfig {
    fn default() -> Self {
        Self {
            steps: 100_000,
            batch_size: 8,
            validation_split: 0.1,
            learning_rate: 1.0e-5,
            checkpoint_every: 20_000,
            chunk_size: 100,
            seed: 1_000,
        }
    }
}

/// ACT's architecture, kept together so checkpoints stay self-describing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActTrainingConfig {
    #[serde(flatten)]
    architecture: ActArchitectureConfig,
    #[serde(flatten)]
    attention: ActAttentionConfig,
    /// Start the vision backbone from its ImageNet weights.
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

impl Default for ActTrainingConfig {
    fn default() -> Self {
        Self {
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
            pretrained_backbone: true,
        }
    }
}

impl ActTrainingConfig {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        architecture: ActArchitectureConfig,
        attention: ActAttentionConfig,
        pretrained_backbone: bool,
    ) -> Self {
        Self {
            architecture,
            attention,
            pretrained_backbone,
        }
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
    pub const fn pretrained_backbone(&self) -> bool {
        self.pretrained_backbone
    }
}

/// Everything needed to reproduce one run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrainingConfig {
    run_name: String,
    policy: TrainingPolicy,
    source: TrainingSource,
    observation: TrainingObservation,
    optimization: TrainingOptimizationConfig,
    act: ActTrainingConfig,
}

impl TrainingConfig {
    /// The default run of `source`.
    pub fn new(run_name: impl Into<String>, source: TrainingSource) -> Self {
        Self {
            run_name: run_name.into(),
            policy: TrainingPolicy::default(),
            source,
            observation: TrainingObservation::default(),
            optimization: TrainingOptimizationConfig::default(),
            act: ActTrainingConfig::default(),
        }
    }
    pub fn run_name(&self) -> &str {
        &self.run_name
    }
    pub const fn policy(&self) -> TrainingPolicy {
        self.policy
    }
    pub const fn source(&self) -> &TrainingSource {
        &self.source
    }
    pub const fn observation(&self) -> &TrainingObservation {
        &self.observation
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
    pub const fn seed(&self) -> u64 {
        self.optimization.seed
    }
    pub const fn act(&self) -> &ActTrainingConfig {
        &self.act
    }

    pub fn set_run_name(&mut self, run_name: impl Into<String>) {
        self.run_name = run_name.into();
    }
    pub fn set_source(&mut self, source: TrainingSource) {
        self.source = source;
    }
    pub fn observation_mut(&mut self) -> &mut TrainingObservation {
        &mut self.observation
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

/// Lifecycle of one training run.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingRunPhase {
    /// Studio is uploading the run's data to Modal.
    #[default]
    Uploading,
    /// Modal has the run and is starting its job.
    Starting,
    /// The job is decoding and sampling the data.
    Preparing,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
    /// Nobody can say where the run is, and why is in its message.
    Unavailable,
}

impl TrainingRunPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Uploading => "Uploading",
            Self::Starting => "Starting",
            Self::Preparing => "Preparing",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Completed => "Completed",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
            Self::Unavailable => "Unavailable",
        }
    }

    pub const fn active(self) -> bool {
        matches!(
            self,
            Self::Uploading | Self::Starting | Self::Preparing | Self::Running | Self::Stopping
        )
    }

    /// Whether the run is still getting ready for its first step, so its
    /// message, not its step, says how far it has got.
    pub const fn before_first_step(self) -> bool {
        matches!(self, Self::Uploading | Self::Starting | Self::Preparing)
    }
}

/// One sample on the shared training-step axis, exactly as the job reports it.
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
    /// Job process CPU usage; 100% represents one fully occupied core.
    cpu_percent: Option<f64>,
    /// Whole-device GPU utilization.
    gpu_percent: Option<f64>,
    /// Current resident memory used by the job process.
    memory_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingStageTimings {
    /// Batch assembly from the prepared samples.
    loader_ms: Option<f64>,
    /// Host-to-device transfer time.
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

/// An inference checkpoint a run saved.
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

/// Whether a run could start now, and why not.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingReadiness {
    ready: bool,
    summary: String,
}

impl Default for TrainingReadiness {
    fn default() -> Self {
        Self::new(false, "Checking the training service…")
    }
}

impl TrainingReadiness {
    pub fn new(ready: bool, summary: impl Into<String>) -> Self {
        Self {
            ready,
            summary: summary.into(),
        }
    }
    pub const fn ready(&self) -> bool {
        self.ready
    }
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

/// What a run was given, resolved when it started: how much of the view it
/// trains on, and the joints its state and action carry.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingInputSummary {
    episodes: usize,
    intervals: usize,
    seconds: f64,
    state: Vec<String>,
    action: Vec<String>,
}

impl TrainingInputSummary {
    /// `intervals` kept intervals of `episodes` episodes, `seconds` long in all.
    pub fn new(episodes: usize, intervals: usize, seconds: f64) -> Self {
        Self {
            episodes,
            intervals,
            seconds,
            state: Vec::new(),
            action: Vec::new(),
        }
    }
    /// The joints the state and the action carry, in order.
    pub fn with_joints(mut self, state: Vec<String>, action: Vec<String>) -> Self {
        self.state = state;
        self.action = action;
        self
    }
    pub const fn episodes(&self) -> usize {
        self.episodes
    }
    pub const fn intervals(&self) -> usize {
        self.intervals
    }
    pub const fn seconds(&self) -> f64 {
        self.seconds
    }
    /// What each state value is, in order.
    pub fn state(&self) -> &[String] {
        &self.state
    }
    /// What each action value is, in order.
    pub fn action(&self) -> &[String] {
        &self.action
    }
}

/// One run as Studio shows it: what it was asked to do, and where it is.
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
    input: TrainingInputSummary,
    started_ms: i64,
}

/// Where a run is now.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRunStatus {
    phase: TrainingRunPhase,
    current_step: u64,
    message: Option<String>,
}

/// The metrics and log tail a run's job retains.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRunHistory {
    metrics: Vec<TrainingMetricPoint>,
    logs: Vec<String>,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self::new(
            "act-run-001",
            TrainingSource::new(
                DatasetSelection::new(vec!["dataset_default".to_owned()])
                    .expect("one nonempty name is a dataset path"),
                "",
            ),
        )
    }
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
    pub const fn input(&self) -> &TrainingInputSummary {
        &self.identity.input
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
    pub fn new(
        id: impl Into<String>,
        config: TrainingConfig,
        input: TrainingInputSummary,
        started_ms: i64,
    ) -> Self {
        Self {
            id: id.into(),
            config,
            input,
            started_ms,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn config(&self) -> &TrainingConfig {
        &self.config
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

    /// A run nobody can say the state of, and why.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::new(TrainingRunPhase::Unavailable, 0, Some(reason.into()))
    }

    pub const fn phase(&self) -> TrainingRunPhase {
        self.phase
    }
}

impl TrainingRunHistory {
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
        let config = TrainingConfig::default();
        assert_eq!(config.batch_size(), 8);
        assert_eq!(config.steps(), 100_000);
        assert_eq!(config.checkpoint_every(), 20_000);
        assert_eq!(config.validation_split(), 0.1);
        assert!(config.observation().cameras().is_empty());
    }

    #[test]
    fn halving_a_size_never_reaches_zero() {
        assert_eq!(ImageSize::new(848, 480).divided(2), ImageSize::new(424, 240));
        assert_eq!(ImageSize::new(3, 1).divided(4), ImageSize::new(1, 1));
    }
}
