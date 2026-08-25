//! Framework-neutral training and policy-rollout capability actions.

use std::collections::BTreeSet;

use spiral_robotics_core::identity::DatasetId;
use spiral_robotics_core::operation::RolloutDuration;
use spiral_robotics_core::query::{ReadOnlyQuery, ReadOnlyQueryMode, UnquotedSqlIdentifier};

use crate::rollout::service::RolloutRequest;
use spiral_robotics_core::OperationError;

/// One image batch expanded to f32 model inputs must leave enough memory for
/// model activations, optimizer state, and the hardware-control process itself.
pub(crate) const MAX_TRAINING_IMAGE_BATCH_BYTES: usize = 256 * 1024 * 1024;

/// Aggregate ACT compute-session budget. Independently valid dimensions must
/// not combine into a multi-gigabyte optimizer allocation that can terminate
/// the selected remote runtime.
const MAX_ACT_TRAINING_RESOURCE_BYTES: usize = 2 * 1024 * 1024 * 1024;
const ACT_NON_TRANSFORMER_RESOURCE_RESERVE_BYTES: usize = 256 * 1024 * 1024;
const ACT_IMAGE_RUNTIME_MULTIPLIER: usize = 4;
/// Parameter + gradient + two AdamW moments, plus one parameter-sized runtime
/// workspace reserve. All ACT trainable tensors use f32 today.
const ACT_TRAINING_BYTES_PER_TRANSFORMER_PARAMETER: usize = 5 * std::mem::size_of::<f32>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrainingPolicy {
    Act,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActTrainingSettings {
    input: ActTrainingInputSettings,
    model: ActTrainingModelSettings,
    layers: ActTrainingLayerSettings,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StartTrainingCommand {
    identity: TrainingCommandIdentity,
    data: TrainingDataSettings,
    execution: TrainingExecutionSettings,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActTrainingInputSettings {
    camera_streams: Vec<String>,
    image_width: usize,
    image_height: usize,
    video_decoder: String,
    pretrained_backbone: bool,
}

impl ActTrainingInputSettings {
    pub(crate) fn new(
        camera_streams: Vec<String>,
        image_size: (usize, usize),
        video_decoder: impl Into<String>,
    ) -> Self {
        Self {
            camera_streams,
            image_width: image_size.0,
            image_height: image_size.1,
            video_decoder: video_decoder.into(),
            pretrained_backbone: false,
        }
    }

    pub(crate) fn with_pretrained_backbone(mut self, pretrained: bool) -> Self {
        self.pretrained_backbone = pretrained;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ActTrainingModelSettings {
    hidden_dim: usize,
    latent_dim: usize,
    attention_heads: usize,
    feedforward_dim: usize,
    kl_weight: f64,
}

impl ActTrainingModelSettings {
    pub(crate) fn new(hidden_dim: usize, latent_dim: usize, attention_heads: usize) -> Self {
        Self {
            hidden_dim,
            latent_dim,
            attention_heads,
            feedforward_dim: 0,
            kl_weight: 0.0,
        }
    }

    pub(crate) fn with_feedforward(mut self, feedforward_dim: usize, kl_weight: f64) -> Self {
        self.feedforward_dim = feedforward_dim;
        self.kl_weight = kl_weight;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ActTrainingLayerSettings {
    encoder_layers: usize,
    vae_encoder_layers: usize,
    decoder_layers: usize,
}

impl ActTrainingLayerSettings {
    pub(crate) fn new(
        encoder_layers: usize,
        vae_encoder_layers: usize,
        decoder_layers: usize,
    ) -> Self {
        Self {
            encoder_layers,
            vae_encoder_layers,
            decoder_layers,
        }
    }
}

impl ActTrainingSettings {
    pub(crate) fn new(
        input: ActTrainingInputSettings,
        model: ActTrainingModelSettings,
        layers: ActTrainingLayerSettings,
    ) -> Self {
        Self {
            input,
            model,
            layers,
        }
    }

    pub(crate) fn camera_streams(&self) -> &[String] {
        &self.input.camera_streams
    }
    pub(crate) fn image_width(&self) -> usize {
        self.input.image_width
    }
    pub(crate) fn image_height(&self) -> usize {
        self.input.image_height
    }
    pub(crate) fn video_decoder(&self) -> &str {
        &self.input.video_decoder
    }
    pub(crate) fn pretrained_backbone(&self) -> bool {
        self.input.pretrained_backbone
    }
    pub(crate) fn hidden_dim(&self) -> usize {
        self.model.hidden_dim
    }
    pub(crate) fn latent_dim(&self) -> usize {
        self.model.latent_dim
    }
    pub(crate) fn attention_heads(&self) -> usize {
        self.model.attention_heads
    }
    pub(crate) fn feedforward_dim(&self) -> usize {
        self.model.feedforward_dim
    }
    pub(crate) fn kl_weight(&self) -> f64 {
        self.model.kl_weight
    }
    pub(crate) fn encoder_layers(&self) -> usize {
        self.layers.encoder_layers
    }
    pub(crate) fn vae_encoder_layers(&self) -> usize {
        self.layers.vae_encoder_layers
    }
    pub(crate) fn decoder_layers(&self) -> usize {
        self.layers.decoder_layers
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrainingCommandIdentity {
    run_name: String,
    dataset_id: String,
    policy: TrainingPolicy,
}

impl TrainingCommandIdentity {
    pub(crate) fn new(
        run_name: impl Into<String>,
        dataset_id: impl Into<String>,
        policy: TrainingPolicy,
    ) -> Self {
        Self {
            run_name: run_name.into(),
            dataset_id: dataset_id.into(),
            policy,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrainingDataSettings {
    query: String,
    state_column: String,
    action_column: String,
}

impl TrainingDataSettings {
    pub(crate) fn new(
        query: impl Into<String>,
        state_column: impl Into<String>,
        action_column: impl Into<String>,
    ) -> Self {
        Self {
            query: query.into(),
            state_column: state_column.into(),
            action_column: action_column.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrainingProgressSettings {
    steps: u64,
    batch_size: usize,
    validation_split: f64,
}

impl TrainingProgressSettings {
    pub(crate) fn new(steps: u64, batch_size: usize, validation_split: f64) -> Self {
        Self {
            steps,
            batch_size,
            validation_split,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrainingOptimizerSettings {
    learning_rate: f64,
    seed: u64,
}

impl TrainingOptimizerSettings {
    pub(crate) fn new(learning_rate: f64, seed: u64) -> Self {
        Self {
            learning_rate,
            seed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TrainingCheckpointSettings {
    checkpoint_every: u64,
    chunk_size: usize,
}

impl TrainingCheckpointSettings {
    pub(crate) fn new(checkpoint_every: u64, chunk_size: usize) -> Self {
        Self {
            checkpoint_every,
            chunk_size,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrainingExecutionSettings {
    device: String,
    progress: TrainingProgressSettings,
    optimizer: TrainingOptimizerSettings,
    checkpoints: TrainingCheckpointSettings,
    act: ActTrainingSettings,
}

impl TrainingExecutionSettings {
    pub(crate) fn new(
        device: impl Into<String>,
        progress: TrainingProgressSettings,
        act: ActTrainingSettings,
    ) -> Self {
        Self {
            device: device.into(),
            progress,
            optimizer: TrainingOptimizerSettings::new(0.0, 0),
            checkpoints: TrainingCheckpointSettings::new(0, 0),
            act,
        }
    }

    pub(crate) fn with_schedule(
        mut self,
        optimizer: TrainingOptimizerSettings,
        checkpoints: TrainingCheckpointSettings,
    ) -> Self {
        self.optimizer = optimizer;
        self.checkpoints = checkpoints;
        self
    }
}

impl StartTrainingCommand {
    pub(crate) fn new(
        identity: TrainingCommandIdentity,
        data: TrainingDataSettings,
        execution: TrainingExecutionSettings,
    ) -> Self {
        Self {
            identity,
            data,
            execution,
        }
    }

    pub(crate) fn normalize_identifiers(&mut self) {
        self.identity.run_name = self.identity.run_name.trim().to_owned();
        self.identity.dataset_id = self.identity.dataset_id.trim().to_owned();
        self.data.state_column = self.data.state_column.trim().to_owned();
        self.data.action_column = self.data.action_column.trim().to_owned();
        self.execution.device = self.execution.device.trim().to_owned();
    }

    pub(crate) fn with_camera_streams(mut self, camera_streams: Vec<String>) -> Self {
        self.execution.act.input.camera_streams = camera_streams;
        self
    }

    pub(crate) fn run_name(&self) -> &str {
        &self.identity.run_name
    }
    pub(crate) fn dataset_id(&self) -> &str {
        &self.identity.dataset_id
    }
    pub(crate) fn policy(&self) -> TrainingPolicy {
        self.identity.policy
    }
    pub(crate) fn query(&self) -> &str {
        &self.data.query
    }
    pub(crate) fn state_column(&self) -> &str {
        &self.data.state_column
    }
    pub(crate) fn action_column(&self) -> &str {
        &self.data.action_column
    }
    pub(crate) fn device(&self) -> &str {
        &self.execution.device
    }
    pub(crate) fn steps(&self) -> u64 {
        self.execution.progress.steps
    }
    pub(crate) fn batch_size(&self) -> usize {
        self.execution.progress.batch_size
    }
    pub(crate) fn validation_split(&self) -> f64 {
        self.execution.progress.validation_split
    }
    pub(crate) fn learning_rate(&self) -> f64 {
        self.execution.optimizer.learning_rate
    }
    pub(crate) fn checkpoint_every(&self) -> u64 {
        self.execution.checkpoints.checkpoint_every
    }
    pub(crate) fn chunk_size(&self) -> usize {
        self.execution.checkpoints.chunk_size
    }
    pub(crate) fn seed(&self) -> u64 {
        self.execution.optimizer.seed
    }
    pub(crate) fn act(&self) -> &ActTrainingSettings {
        &self.execution.act
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TrainingCheckpointStatus {
    step: u64,
    inference_ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrainingRunStatus {
    active: bool,
    checkpoints: Vec<TrainingCheckpointStatus>,
}

/// Other live work that can still depend on a completed run's artifacts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrainingRunActivity {
    rollout: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeleteTrainingRunCommand {
    run_id: String,
}

impl TrainingCheckpointStatus {
    pub(crate) const fn new(step: u64, inference_ready: bool) -> Self {
        Self {
            step,
            inference_ready,
        }
    }
}

impl TrainingRunStatus {
    pub(crate) fn new(active: bool, checkpoints: Vec<TrainingCheckpointStatus>) -> Self {
        Self {
            active,
            checkpoints,
        }
    }
}

impl TrainingRunActivity {
    pub(crate) fn new(rollout: Option<String>) -> Self {
        Self { rollout }
    }
}

impl DeleteTrainingRunCommand {
    pub(crate) fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TrainingRunPhase {
    #[default]
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrainingMetric {
    progress: TrainingMetricProgress,
    throughput: TrainingMetricThroughput,
    resources: TrainingMetricResources,
    timings: TrainingMetricTimings,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrainingMetricProgress {
    step: u64,
    train_loss: f64,
    validation_loss: Option<f64>,
    learning_rate: f64,
    event: Option<String>,
}

impl TrainingMetricProgress {
    pub(crate) fn new(step: u64, train_loss: f64, validation_loss: Option<f64>) -> Self {
        Self {
            step,
            train_loss,
            validation_loss,
            learning_rate: 0.0,
            event: None,
        }
    }

    pub(crate) fn with_event(mut self, learning_rate: f64, event: Option<String>) -> Self {
        self.learning_rate = learning_rate;
        self.event = event;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TrainingMetricThroughput {
    transfer_bytes_per_second: f64,
    examples_per_second: f64,
}

impl TrainingMetricThroughput {
    pub(crate) fn new(transfer_bytes_per_second: f64, examples_per_second: f64) -> Self {
        Self {
            transfer_bytes_per_second,
            examples_per_second,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TrainingMetricResources {
    cpu_percent: Option<f64>,
    gpu_percent: Option<f64>,
    memory_bytes: Option<u64>,
}

impl TrainingMetricResources {
    pub(crate) fn new(
        cpu_percent: Option<f64>,
        gpu_percent: Option<f64>,
        memory_bytes: Option<u64>,
    ) -> Self {
        Self {
            cpu_percent,
            gpu_percent,
            memory_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TrainingMetricTimings {
    loader_ms: Option<f64>,
    batch_encode_ms: Option<f64>,
    upload_ms: Option<f64>,
    device_copy_ms: Option<f64>,
    model_ms: Option<f64>,
    elapsed_seconds: f64,
}

impl TrainingMetricTimings {
    pub(crate) fn new(
        loader_ms: Option<f64>,
        batch_encode_ms: Option<f64>,
        upload_ms: Option<f64>,
    ) -> Self {
        Self {
            loader_ms,
            batch_encode_ms,
            upload_ms,
            ..Self::default()
        }
    }

    pub(crate) fn with_execution(
        mut self,
        device_copy_ms: Option<f64>,
        model_ms: Option<f64>,
    ) -> Self {
        self.device_copy_ms = device_copy_ms;
        self.model_ms = model_ms;
        self
    }

    pub(crate) fn elapsed(mut self, elapsed_seconds: f64) -> Self {
        self.elapsed_seconds = elapsed_seconds;
        self
    }
}

impl TrainingMetric {
    pub(crate) fn new(
        progress: TrainingMetricProgress,
        throughput: TrainingMetricThroughput,
        resources: TrainingMetricResources,
    ) -> Self {
        Self {
            progress,
            throughput,
            resources,
            timings: TrainingMetricTimings::default(),
        }
    }

    pub(crate) fn with_timings(mut self, timings: TrainingMetricTimings) -> Self {
        self.timings = timings;
        self
    }
    pub(crate) fn step(&self) -> u64 {
        self.progress.step
    }
    pub(crate) fn train_loss(&self) -> f64 {
        self.progress.train_loss
    }
    pub(crate) fn validation_loss(&self) -> Option<f64> {
        self.progress.validation_loss
    }
    pub(crate) fn learning_rate(&self) -> f64 {
        self.progress.learning_rate
    }
    pub(crate) fn event(&self) -> Option<&str> {
        self.progress.event.as_deref()
    }
    pub(crate) fn transfer_bytes_per_second(&self) -> f64 {
        self.throughput.transfer_bytes_per_second
    }
    pub(crate) fn examples_per_second(&self) -> f64 {
        self.throughput.examples_per_second
    }
    pub(crate) fn cpu_percent(&self) -> Option<f64> {
        self.resources.cpu_percent
    }
    pub(crate) fn gpu_percent(&self) -> Option<f64> {
        self.resources.gpu_percent
    }
    pub(crate) fn memory_bytes(&self) -> Option<u64> {
        self.resources.memory_bytes
    }
    pub(crate) fn loader_ms(&self) -> Option<f64> {
        self.timings.loader_ms
    }
    pub(crate) fn batch_encode_ms(&self) -> Option<f64> {
        self.timings.batch_encode_ms
    }
    pub(crate) fn upload_ms(&self) -> Option<f64> {
        self.timings.upload_ms
    }
    pub(crate) fn device_copy_ms(&self) -> Option<f64> {
        self.timings.device_copy_ms
    }
    pub(crate) fn model_ms(&self) -> Option<f64> {
        self.timings.model_ms
    }
    pub(crate) fn elapsed_seconds(&self) -> f64 {
        self.timings.elapsed_seconds
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrainingCheckpoint {
    step: u64,
    created_ms: i64,
    validation_loss: Option<f64>,
    byte_size: u64,
    inference_ready: bool,
    recommended_action_steps: usize,
    sample_hz: f64,
}

impl TrainingCheckpoint {
    pub(crate) fn new(step: u64, created_ms: i64) -> Self {
        Self {
            step,
            created_ms,
            ..Self::default()
        }
    }
    pub(crate) fn with_validation_loss(mut self, validation_loss: Option<f64>) -> Self {
        self.validation_loss = validation_loss;
        self
    }
    pub(crate) fn with_artifact(mut self, byte_size: u64, inference_ready: bool) -> Self {
        self.byte_size = byte_size;
        self.inference_ready = inference_ready;
        self
    }
    pub(crate) fn with_rollout(mut self, recommended_action_steps: usize, sample_hz: f64) -> Self {
        self.recommended_action_steps = recommended_action_steps;
        self.sample_hz = sample_hz;
        self
    }
    pub(crate) const fn step(&self) -> u64 {
        self.step
    }
    pub(crate) const fn created_ms(&self) -> i64 {
        self.created_ms
    }
    pub(crate) const fn validation_loss(&self) -> Option<f64> {
        self.validation_loss
    }
    pub(crate) const fn byte_size(&self) -> u64 {
        self.byte_size
    }
    pub(crate) const fn inference_ready(&self) -> bool {
        self.inference_ready
    }
    pub(crate) const fn recommended_action_steps(&self) -> usize {
        self.recommended_action_steps
    }
    pub(crate) const fn sample_hz(&self) -> f64 {
        self.sample_hz
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrainingSource {
    dataset_id: String,
}

impl TrainingSource {
    pub(crate) fn new(dataset_id: impl Into<String>) -> Self {
        Self {
            dataset_id: dataset_id.into(),
        }
    }

    pub(crate) fn dataset_id(&self) -> &str {
        &self.dataset_id
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrainingReadiness {
    ready: bool,
    spiral_ready: bool,
    runtime: Option<TrainingRuntimeInfo>,
    summary: String,
}

impl TrainingReadiness {
    pub(crate) fn new(ready: bool, spiral_ready: bool, summary: impl Into<String>) -> Self {
        Self {
            ready,
            spiral_ready,
            runtime: None,
            summary: summary.into(),
        }
    }

    pub(crate) fn with_runtime(mut self, runtime: Option<TrainingRuntimeInfo>) -> Self {
        self.runtime = runtime;
        self
    }

    pub(crate) const fn ready(&self) -> bool {
        self.ready
    }

    pub(crate) const fn spiral_ready(&self) -> bool {
        self.spiral_ready
    }

    pub(crate) fn runtime(&self) -> Option<&TrainingRuntimeInfo> {
        self.runtime.as_ref()
    }

    pub(crate) fn summary(&self) -> &str {
        &self.summary
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrainingRuntimeInfo {
    id: String,
    display_name: String,
    runtime: String,
    devices: Vec<String>,
}

impl TrainingRuntimeInfo {
    pub(crate) fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        runtime: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            runtime: runtime.into(),
            devices: Vec::new(),
        }
    }

    pub(crate) fn with_devices(mut self, devices: Vec<String>) -> Self {
        self.devices = devices;
        self
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) fn runtime(&self) -> &str {
        &self.runtime
    }

    pub(crate) fn devices(&self) -> &[String] {
        &self.devices
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrainingRun {
    identity: TrainingRunIdentity,
    state: TrainingRunState,
    history: TrainingRunHistory,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrainingRunIdentity {
    id: String,
    config: StartTrainingCommand,
    source: TrainingSource,
}

impl TrainingRunIdentity {
    pub(crate) fn new(
        id: impl Into<String>,
        config: StartTrainingCommand,
        source: TrainingSource,
    ) -> Self {
        Self {
            id: id.into(),
            config,
            source,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrainingRunState {
    phase: TrainingRunPhase,
    current_step: u64,
    started_ms: i64,
    finished_ms: Option<i64>,
    message: Option<String>,
}

impl TrainingRunState {
    pub(crate) fn new(phase: TrainingRunPhase, current_step: u64, started_ms: i64) -> Self {
        Self {
            phase,
            current_step,
            started_ms,
            finished_ms: None,
            message: None,
        }
    }

    pub(crate) fn finished(mut self, finished_ms: Option<i64>, message: Option<String>) -> Self {
        self.finished_ms = finished_ms;
        self.message = message;
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrainingRunHistory {
    metrics: Vec<TrainingMetric>,
    checkpoints: Vec<TrainingCheckpoint>,
    logs: Vec<String>,
}

impl TrainingRunHistory {
    pub(crate) fn new(
        metrics: Vec<TrainingMetric>,
        checkpoints: Vec<TrainingCheckpoint>,
        logs: Vec<String>,
    ) -> Self {
        Self {
            metrics,
            checkpoints,
            logs,
        }
    }
}

impl TrainingRun {
    pub(crate) fn new(
        identity: TrainingRunIdentity,
        state: TrainingRunState,
        history: TrainingRunHistory,
    ) -> Self {
        Self {
            identity,
            state,
            history,
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.identity.id
    }
    pub(crate) fn config(&self) -> &StartTrainingCommand {
        &self.identity.config
    }
    pub(crate) fn source(&self) -> &TrainingSource {
        &self.identity.source
    }
    pub(crate) fn phase(&self) -> TrainingRunPhase {
        self.state.phase
    }
    pub(crate) fn current_step(&self) -> u64 {
        self.state.current_step
    }
    pub(crate) fn started_ms(&self) -> i64 {
        self.state.started_ms
    }
    pub(crate) fn finished_ms(&self) -> Option<i64> {
        self.state.finished_ms
    }
    pub(crate) fn message(&self) -> Option<&str> {
        self.state.message.as_deref()
    }
    pub(crate) fn metrics(&self) -> &[TrainingMetric] {
        &self.history.metrics
    }
    pub(crate) fn checkpoints(&self) -> &[TrainingCheckpoint] {
        &self.history.checkpoints
    }
    pub(crate) fn logs(&self) -> &[String] {
        &self.history.logs
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrainingSnapshot {
    readiness: TrainingReadiness,
    runs: Vec<TrainingRun>,
}

impl TrainingSnapshot {
    pub(crate) fn new(readiness: TrainingReadiness, runs: Vec<TrainingRun>) -> Self {
        Self { readiness, runs }
    }
    pub(crate) const fn readiness(&self) -> &TrainingReadiness {
        &self.readiness
    }
    pub(crate) fn runs(&self) -> &[TrainingRun] {
        &self.runs
    }
}

pub(crate) trait TrainingPort: Send + Sync {
    fn training_snapshot(&self) -> TrainingSnapshot;

    fn training_run_status(&self, run_id: &str) -> Option<TrainingRunStatus>;

    async fn start_training(
        &self,
        command: StartTrainingCommand,
    ) -> Result<TrainingRun, OperationError>;

    fn stop_training(&self, run_id: &str) -> Result<(), OperationError>;

    async fn delete_training(&self, run_id: String) -> Result<(), OperationError>;
}

pub(crate) fn list_training<P: TrainingPort + ?Sized>(port: &P) -> TrainingSnapshot {
    port.training_snapshot()
}

pub(crate) async fn start_training<P: TrainingPort + ?Sized>(
    port: &P,
    command: StartTrainingCommand,
) -> Result<TrainingRun, OperationError> {
    let command = command.validated()?;
    port.start_training(command).await
}

pub(crate) fn stop_training<P: TrainingPort + ?Sized>(
    port: &P,
    run_id: &str,
) -> Result<(), OperationError> {
    port.stop_training(run_id)
}

/// Delete one inactive run and every artifact owned by it.
///
/// Admission checks happen before crossing the deletion boundary so the
/// adapter never removes a checkpoint that is training or driving an arm.
pub(crate) async fn delete_training<P: TrainingPort + ?Sized>(
    port: &P,
    activity: TrainingRunActivity,
    command: DeleteTrainingRunCommand,
) -> Result<(), OperationError> {
    let run_id = validate_run_id(command.run_id)?;
    let Some(run) = port.training_run_status(&run_id) else {
        return Err(OperationError::NotFound(format!(
            "training run {run_id:?} is no longer available"
        )));
    };
    if run.active {
        return Err(OperationError::Conflict(format!(
            "training run {run_id:?} is still active; stop it before deleting it"
        )));
    }
    if activity.rollout.as_deref() == Some(run_id.as_str()) {
        return Err(OperationError::Conflict(format!(
            "training run {run_id:?} is in use by Rollout; stop Rollout before deleting it"
        )));
    }
    port.delete_training(run_id).await
}

pub(crate) fn admit_rollout<T>(
    training: &T,
    command: RolloutRequest,
) -> Result<RolloutRequest, OperationError>
where
    T: TrainingPort + ?Sized,
{
    let command = command.validated()?;
    let Some(run) = training.training_run_status(command.run_id()) else {
        return Err(OperationError::InvalidInput(
            "the selected training run is no longer available".to_owned(),
        ));
    };
    if run.active {
        return Err(OperationError::InvalidInput(
            "stop or finish training before starting a rollout".to_owned(),
        ));
    }
    if !run.checkpoints.iter().any(|checkpoint| {
        checkpoint.step == command.checkpoint_step() && checkpoint.inference_ready
    }) {
        return Err(OperationError::InvalidInput(
            "the selected checkpoint is no longer available".to_owned(),
        ));
    }
    Ok(command)
}

impl StartTrainingCommand {
    fn validated(mut self) -> Result<Self, OperationError> {
        let command = &mut self;
        command.normalize_identifiers();

        if command.run_name().is_empty()
            || command.run_name().len() > 80
            || !command
                .run_name()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return invalid("Run name must use 1–80 letters, numbers, '.', '_' or '-' characters.");
        }
        DatasetId::try_from(command.dataset_id()).map_err(|_| {
            OperationError::InvalidInput(
                "Dataset ID must use 1–96 letters, numbers, '.', '_' or '-' characters.".to_owned(),
            )
        })?;
        ReadOnlyQuery::new(command.query(), ReadOnlyQueryMode::Training)
            .authorize()
            .map_err(|error| OperationError::InvalidInput(error.to_string()))?;
        for (label, column) in [
            ("State column", command.state_column()),
            ("Action column", command.action_column()),
        ] {
            if UnquotedSqlIdentifier::try_from(column).is_err() {
                return invalid(format!("{label} must be a SQL identifier."));
            }
        }
        if command.state_column() == command.action_column() {
            return invalid("State and action columns must be different.");
        }
        if command.device().is_empty()
            || command.device().len() > 64
            || !command.device().bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/' | b'+')
            })
        {
            return invalid(
                "Training device must be a 1–64 character identifier advertised by the runtime.",
            );
        }
        if !(1..=10_000_000).contains(&command.steps()) {
            return invalid("Training steps must be between 1 and 10,000,000.");
        }
        if !(1..=512).contains(&command.batch_size()) {
            return invalid("Batch size must be between 1 and 512.");
        }
        if !command.validation_split().is_finite()
            || !(0.0..=0.5).contains(&command.validation_split())
        {
            return invalid("Validation split must be between 0% and 50%.");
        }
        if !(1..=256).contains(&command.chunk_size()) {
            return invalid("Action chunk size must be between 1 and 256.");
        }
        if command
            .batch_size()
            .checked_mul(command.chunk_size())
            .is_none_or(|rows| rows > 8_192)
        {
            return invalid("Batch size × action chunk size may not exceed 8,192 manifest rows.");
        }
        if command.checkpoint_every() == 0 || command.checkpoint_every() > command.steps() {
            return invalid("Checkpoint cadence must be between 1 and the run step count.");
        }
        if !command.learning_rate().is_finite() || command.learning_rate() <= 0.0 {
            return invalid("Learning rate must be a finite positive number.");
        }
        if command.act().camera_streams().len() > 8 {
            return invalid("ACT supports at most eight camera streams.");
        }
        let mut camera_streams = BTreeSet::new();
        for stream in command.act().camera_streams() {
            if stream.is_empty()
                || !stream
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            {
                return invalid(format!("Invalid ACT camera stream {stream:?}."));
            }
            if !camera_streams.insert(stream) {
                return invalid(format!("ACT camera stream {stream:?} is repeated."));
            }
        }
        let native_images = command.act().image_width() == 0 && command.act().image_height() == 0;
        if !native_images
            && (!(32..=1_024).contains(&command.act().image_width())
                || !(32..=1_024).contains(&command.act().image_height()))
        {
            return invalid(
                "ACT image width and height must both be zero (native) or between 32 and 1,024.",
            );
        }
        let runtime_image_bytes = if native_images {
            // The adapter repeats the exact check after resolving native dataset
            // dimensions. Budget the largest batch it is allowed to admit here.
            MAX_TRAINING_IMAGE_BATCH_BYTES
        } else {
            // An empty list means automatic selection, not zero cameras. Admission
            // must budget the maximum the use case permits; the adapter repeats the
            // check with the exact resolved dataset camera count.
            let camera_count = if command.act().camera_streams().is_empty() {
                8
            } else {
                command.act().camera_streams().len()
            };
            // Training transports one byte per channel, but the runtime expands
            // every image to f32 before model execution. Admission budgets that
            // expanded representation rather than the smaller HTTP request body.
            let image_bytes = command
                .batch_size()
                .checked_mul(camera_count)
                .and_then(|values| values.checked_mul(3))
                .and_then(|values| values.checked_mul(command.act().image_width()))
                .and_then(|values| values.checked_mul(command.act().image_height()))
                .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()));
            if image_bytes.is_none_or(|bytes| bytes > MAX_TRAINING_IMAGE_BATCH_BYTES) {
                return invalid(format!(
                    "ACT expanded f32 image batch may not exceed {MAX_TRAINING_IMAGE_BATCH_BYTES} bytes. Reduce batch size, camera count, or image dimensions."
                ));
            }
            image_bytes.expect("checked runtime image byte count")
        };
        if !matches!(
            command.act().video_decoder(),
            "auto" | "ffmpeg" | "videotoolbox" | "nvdec"
        ) {
            return invalid("ACT video decoder must be auto, ffmpeg, videotoolbox, or nvdec.");
        }
        if !(1..=16).contains(&command.act().attention_heads()) {
            return invalid("ACT attention heads must be between 1 and 16.");
        }
        if !(64..=1_024).contains(&command.act().hidden_dim())
            || !command.act().hidden_dim().is_multiple_of(4)
            || !command
                .act()
                .hidden_dim()
                .is_multiple_of(command.act().attention_heads())
        {
            return invalid(
                "ACT hidden size must be 64–1,024 and divisible by four and the attention-head count.",
            );
        }
        if !(1..=12).contains(&command.act().encoder_layers())
            || !(1..=12).contains(&command.act().vae_encoder_layers())
            || !(1..=12).contains(&command.act().decoder_layers())
            || !(1..=512).contains(&command.act().latent_dim())
            || command.act().feedforward_dim() < command.act().hidden_dim()
            || command.act().feedforward_dim() > 16_384
        {
            return invalid("ACT transformer configuration is outside the supported range.");
        }
        enforce_act_training_resource_budget(estimate_act_training_resource_bytes(
            &command,
            runtime_image_bytes,
        ))?;
        if !command.act().kl_weight().is_finite() || command.act().kl_weight() < 0.0 {
            return invalid("ACT KL weight must be finite and non-negative.");
        }
        Ok(self)
    }
}

fn validate_run_id(run_id: String) -> Result<String, OperationError> {
    let run_id = run_id.trim().to_owned();
    if run_id.is_empty()
        || run_id.len() > 128
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return invalid("Training run ID is invalid.");
    }
    Ok(run_id)
}

/// Conservative, checked estimate for memory owned while one ACT optimizer
/// step is live. The exact state/action projections are dataset-dependent, so
/// a fixed non-transformer reserve covers those, ResNet-18, CVAE projections,
/// and backend bookkeeping. Known transformer parameters are counted from the
/// command, including attention, feed-forward, bias, and norm terms.
fn estimate_act_training_resource_bytes(
    command: &StartTrainingCommand,
    runtime_image_bytes: usize,
) -> Option<usize> {
    let hidden = command.act().hidden_dim();
    let feedforward = command.act().feedforward_dim();
    let hidden_squared = hidden.checked_mul(hidden)?;
    let hidden_feedforward = hidden.checked_mul(feedforward)?;

    // Encoder/CVAE encoder: one self-attention, one feed-forward pair, two
    // norms. Decoder: self + cross attention, one feed-forward pair, 3 norms.
    let encoder_parameters = hidden_squared
        .checked_mul(4)?
        .checked_add(hidden_feedforward.checked_mul(2)?)?
        .checked_add(hidden.checked_mul(9)?)?
        .checked_add(feedforward)?;
    let decoder_parameters = hidden_squared
        .checked_mul(8)?
        .checked_add(hidden_feedforward.checked_mul(2)?)?
        .checked_add(hidden.checked_mul(15)?)?
        .checked_add(feedforward)?;
    let encoder_count = command
        .act()
        .encoder_layers()
        .checked_add(command.act().vae_encoder_layers())?;
    let transformer_parameters = encoder_parameters
        .checked_mul(encoder_count)?
        .checked_add(decoder_parameters.checked_mul(command.act().decoder_layers())?)?
        .checked_add(hidden.checked_mul(command.act().latent_dim())?)?
        .checked_add(hidden.checked_mul(command.chunk_size())?)?;
    let transformer_training_bytes =
        transformer_parameters.checked_mul(ACT_TRAINING_BYTES_PER_TRANSFORMER_PARAMETER)?;
    let image_runtime_bytes = runtime_image_bytes.checked_mul(ACT_IMAGE_RUNTIME_MULTIPLIER)?;
    transformer_training_bytes
        .checked_add(image_runtime_bytes)?
        .checked_add(ACT_NON_TRANSFORMER_RESOURCE_RESERVE_BYTES)
}

fn enforce_act_training_resource_budget(estimate: Option<usize>) -> Result<(), OperationError> {
    if estimate.is_none_or(|bytes| bytes > MAX_ACT_TRAINING_RESOURCE_BYTES) {
        return invalid(format!(
            "ACT model and optimizer may not exceed the {MAX_ACT_TRAINING_RESOURCE_BYTES}-byte remote compute-session budget. Reduce hidden size, feed-forward size, layer count, batch size, cameras, or image dimensions."
        ));
    }
    Ok(())
}

impl RolloutRequest {
    fn validated(self) -> Result<Self, OperationError> {
        let command = Self::new(
            self.run_id().trim().to_owned(),
            self.checkpoint_step(),
            self.robot().trim().to_owned(),
        )
        .with_duration(self.duration_seconds());
        if command.run_id().is_empty() || command.robot().is_empty() {
            return invalid("rollout requires a training run and target robot");
        }
        if command.checkpoint_step() == 0 {
            return invalid("checkpoint step must be greater than zero");
        }
        if RolloutDuration::new(command.duration_seconds()).is_none() {
            return invalid(format!(
                "rollout duration_seconds must be between {} and {}",
                spiral_robotics_core::operation::MIN_ROLLOUT_DURATION_SECONDS,
                spiral_robotics_core::operation::MAX_ROLLOUT_DURATION_SECONDS,
            ));
        }
        Ok(command)
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, OperationError> {
    Err(OperationError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct FakeTraining {
        run: Option<TrainingRunStatus>,
        starts: Mutex<Vec<StartTrainingCommand>>,
        stops: Mutex<Vec<String>>,
        deletes: Mutex<Vec<String>>,
    }

    impl TrainingPort for FakeTraining {
        fn training_snapshot(&self) -> TrainingSnapshot {
            TrainingSnapshot::default()
        }

        fn training_run_status(&self, _: &str) -> Option<TrainingRunStatus> {
            self.run.clone()
        }

        async fn start_training(
            &self,
            command: StartTrainingCommand,
        ) -> Result<TrainingRun, OperationError> {
            self.starts.lock().unwrap().push(command.clone());
            Ok(training_run(command))
        }

        fn stop_training(&self, run_id: &str) -> Result<(), OperationError> {
            self.stops.lock().unwrap().push(run_id.to_owned());
            Ok(())
        }

        async fn delete_training(&self, run_id: String) -> Result<(), OperationError> {
            self.deletes.lock().unwrap().push(run_id);
            Ok(())
        }
    }

    fn training(run: Option<TrainingRunStatus>) -> FakeTraining {
        FakeTraining {
            run,
            starts: Mutex::new(Vec::new()),
            stops: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
        }
    }

    fn rollout_command() -> RolloutRequest {
        RolloutRequest::new("run-1".to_owned(), 20_000, "follower".to_owned()).with_duration(30)
    }

    fn training_command() -> StartTrainingCommand {
        let act = ActTrainingSettings::new(
            ActTrainingInputSettings::new(vec!["wrist".to_owned()], (0, 0), "auto")
                .with_pretrained_backbone(true),
            ActTrainingModelSettings::new(256, 32, 8).with_feedforward(1_024, 10.0),
            ActTrainingLayerSettings::new(4, 4, 4),
        );
        StartTrainingCommand::new(
            TrainingCommandIdentity::new(" act-run ", " demo ", TrainingPolicy::Act),
            TrainingDataSettings::new("SELECT state, action FROM samples", " state ", " action "),
            TrainingExecutionSettings::new(
                " CPU ",
                TrainingProgressSettings::new(1_000, 8, 0.1),
                act,
            )
            .with_schedule(
                TrainingOptimizerSettings::new(1.0e-5, 7),
                TrainingCheckpointSettings::new(100, 16),
            ),
        )
    }

    fn training_run(config: StartTrainingCommand) -> TrainingRun {
        TrainingRun::new(
            TrainingRunIdentity::new("run", config, TrainingSource::default()),
            TrainingRunState::new(TrainingRunPhase::Starting, 0, 1),
            TrainingRunHistory::default(),
        )
    }

    #[tokio::test]
    async fn training_validation_is_pure_and_normalizes_identifiers_before_delegating() {
        let training = training(None);
        start_training(&training, training_command()).await.unwrap();
        let starts = training.starts.lock().unwrap();
        assert_eq!(starts[0].run_name(), "act-run");
        assert_eq!(starts[0].dataset_id(), "demo");
        assert_eq!(starts[0].state_column(), "state");
        assert_eq!(starts[0].action_column(), "action");
        assert_eq!(starts[0].device(), "CPU");
    }

    #[tokio::test]
    async fn training_accepts_runtime_advertised_cuda_device_identifiers() {
        let mut command = training_command();
        command.execution.device = " cuda:0 ".to_owned();

        let training = training(None);
        start_training(&training, command).await.unwrap();

        let starts = training.starts.lock().unwrap();
        assert_eq!(starts[0].device(), "cuda:0");
    }

    #[tokio::test]
    async fn invalid_training_never_reaches_the_runtime_adapter() {
        let mut commands = Vec::new();
        let mut mutating = training_command();
        mutating.data.query = "DELETE FROM samples".to_owned();
        commands.push(mutating);
        let mut duplicate_columns = training_command();
        duplicate_columns.data.action_column = "state".to_owned();
        commands.push(duplicate_columns);
        let mut invalid_vae = training_command();
        invalid_vae.execution.act.layers.vae_encoder_layers = 0;
        commands.push(invalid_vae);
        let mut non_finite = training_command();
        non_finite.execution.optimizer.learning_rate = f64::NAN;
        commands.push(non_finite);
        let mut invalid_validation_split = training_command();
        invalid_validation_split.execution.progress.validation_split = 0.75;
        commands.push(invalid_validation_split);

        let training = training(None);
        for command in commands {
            assert!(matches!(
                start_training(&training, command).await,
                Err(OperationError::InvalidInput(_))
            ));
        }
        assert!(training.starts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn expanded_image_batch_budget_is_checked_at_the_exact_boundary() {
        let mut boundary = training_command();
        boundary.execution.progress.batch_size = 85;
        boundary.execution.checkpoints.chunk_size = 1;
        boundary.execution.act.input.camera_streams = vec!["wrist".to_owned()];
        boundary.execution.act.input.image_width = 512;
        boundary.execution.act.input.image_height = 512;
        // 85 × 1 × 3 × 512 × 512 × 4 = 255 MiB, under the 256 MiB cap.
        let at_boundary = training(None);
        start_training(&at_boundary, boundary.clone())
            .await
            .unwrap();

        boundary.execution.progress.batch_size = 86;
        let over_budget = training(None);
        let error = start_training(&over_budget, boundary).await.unwrap_err();
        assert!(error.message().contains("expanded f32 image batch"));
        assert!(over_budget.starts.lock().unwrap().is_empty());

        let mut automatic = training_command();
        automatic.execution.progress.batch_size = 11;
        automatic.execution.checkpoints.chunk_size = 1;
        automatic.execution.act.input.camera_streams.clear();
        automatic.execution.act.input.image_width = 512;
        automatic.execution.act.input.image_height = 512;
        let training = training(None);
        let error = start_training(&training, automatic).await.unwrap_err();
        assert!(error.message().contains("expanded f32 image batch"));
    }

    #[tokio::test]
    async fn aggregate_act_budget_rejects_individually_valid_dimensions_before_runtime() {
        let mut command = training_command();
        command.execution.progress.batch_size = 1;
        command.execution.checkpoints.chunk_size = 1;
        command.execution.act.model.hidden_dim = 1_024;
        command.execution.act.model.latent_dim = 512;
        command.execution.act.layers.encoder_layers = 12;
        command.execution.act.layers.vae_encoder_layers = 12;
        command.execution.act.layers.decoder_layers = 12;
        command.execution.act.model.attention_heads = 16;
        command.execution.act.model.feedforward_dim = 16_384;

        let training = training(None);
        let error = start_training(&training, command).await.unwrap_err();
        assert!(error.message().contains("model and optimizer"));
        assert!(training.starts.lock().unwrap().is_empty());
    }

    #[test]
    fn aggregate_act_budget_is_inclusive_and_overflow_is_fail_closed() {
        assert!(
            enforce_act_training_resource_budget(Some(MAX_ACT_TRAINING_RESOURCE_BYTES)).is_ok()
        );
        assert!(
            enforce_act_training_resource_budget(MAX_ACT_TRAINING_RESOURCE_BYTES.checked_add(1))
                .is_err()
        );
        assert!(enforce_act_training_resource_budget(None).is_err());

        let mut overflow = training_command();
        overflow.execution.act.model.hidden_dim = usize::MAX;
        overflow.execution.act.model.feedforward_dim = usize::MAX;
        overflow.execution.act.layers.encoder_layers = usize::MAX;
        assert!(
            estimate_act_training_resource_bytes(&overflow, usize::MAX).is_none(),
            "checked resource arithmetic must never wrap to an admitted estimate"
        );
    }

    #[tokio::test]
    async fn malformed_rollout_is_rejected_before_training_or_runtime_lookup() {
        let training = training(run(false, 20_000, true));
        for command in [
            RolloutRequest::new(String::new(), 20_000, "follower".to_owned()).with_duration(30),
            RolloutRequest::new("run-1".to_owned(), 0, "follower".to_owned()).with_duration(30),
            RolloutRequest::new("run-1".to_owned(), 20_000, "follower".to_owned())
                .with_duration(spiral_robotics_core::operation::MAX_ROLLOUT_DURATION_SECONDS + 1),
        ] {
            assert!(matches!(
                admit_rollout(&training, command),
                Err(OperationError::InvalidInput(_))
            ));
        }
    }

    fn run(active: bool, step: u64, inference_ready: bool) -> Option<TrainingRunStatus> {
        Some(TrainingRunStatus::new(
            active,
            vec![TrainingCheckpointStatus::new(step, inference_ready)],
        ))
    }

    #[tokio::test]
    async fn rollout_rejects_missing_active_or_unready_training_without_delegating() {
        for (run, expected) in [
            (None, "no longer available"),
            (run(true, 20_000, true), "stop or finish"),
            (
                run(false, 10_000, true),
                "checkpoint is no longer available",
            ),
            (
                run(false, 20_000, false),
                "checkpoint is no longer available",
            ),
        ] {
            let training = training(run);
            let error = admit_rollout(&training, rollout_command()).unwrap_err();
            assert!(error.message().contains(expected));
        }
    }

    #[tokio::test]
    async fn ready_rollout_returns_the_exact_admitted_semantic_command() {
        let training = training(run(false, 20_000, true));
        let command = rollout_command();

        assert_eq!(admit_rollout(&training, command.clone()), Ok(command));
    }

    #[tokio::test]
    async fn deleting_a_run_rejects_training_and_rollout_activity() {
        let command = || DeleteTrainingRunCommand::new("run-1");
        let active_training = training(run(true, 20_000, true));
        let error = delete_training(&active_training, TrainingRunActivity::default(), command())
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::Conflict(_)));
        assert!(active_training.deletes.lock().unwrap().is_empty());

        let active_rollout = training(run(false, 20_000, true));
        let error = delete_training(
            &active_rollout,
            TrainingRunActivity::new(Some("run-1".to_owned())),
            command(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, OperationError::Conflict(_)));
        assert!(active_rollout.deletes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deleting_a_missing_run_is_not_found_and_an_idle_run_delegates_once() {
        let missing = training(None);
        let error = delete_training(
            &missing,
            TrainingRunActivity::default(),
            DeleteTrainingRunCommand::new("run-1"),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, OperationError::NotFound(_)));

        let idle = training(run(false, 20_000, true));
        delete_training(
            &idle,
            TrainingRunActivity::default(),
            DeleteTrainingRunCommand::new("run-1"),
        )
        .await
        .unwrap();
        assert_eq!(idle.deletes.lock().unwrap().as_slice(), ["run-1"]);
    }
}
