use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingInitialization {
    Random,
    PretrainedBackbone,
}

/// The two services required to read one SpiralDB project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpiralServiceEndpoints {
    api_url: String,
    spfs_url: String,
}

impl SpiralServiceEndpoints {
    pub fn new(api_url: impl Into<String>, spfs_url: impl Into<String>) -> Self {
        let endpoints = Self {
            api_url: api_url.into(),
            spfs_url: spfs_url.into(),
        };
        assert!(
            !endpoints.api_url.trim().is_empty(),
            "Spiral API URL must not be empty"
        );
        assert!(
            !endpoints.spfs_url.trim().is_empty(),
            "SPFS URL must not be empty"
        );
        endpoints
    }
}

/// One immutable connection selection sent only to the training worker. The
/// client secret is write-only control-plane material and must never appear in
/// job snapshots, logs, or telemetry.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SpiralJobConnection {
    api_url: String,
    spfs_url: String,
    project_id: String,
    table_prefix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
}

impl SpiralJobConnection {
    pub fn new(
        endpoints: SpiralServiceEndpoints,
        project_id: impl Into<String>,
        table_prefix: impl Into<String>,
    ) -> Self {
        let connection = Self {
            api_url: endpoints.api_url,
            spfs_url: endpoints.spfs_url,
            project_id: project_id.into(),
            table_prefix: table_prefix.into(),
            client_id: None,
            client_secret: None,
        };
        assert!(
            !connection.project_id.trim().is_empty(),
            "Spiral project ID must not be empty"
        );
        assert!(
            !connection.table_prefix.trim().is_empty(),
            "Studio table prefix must not be empty"
        );
        connection
    }

    pub fn with_credentials(
        mut self,
        client_id: Option<String>,
        client_secret: Option<String>,
    ) -> Self {
        assert_eq!(
            client_id.is_some(),
            client_secret.is_some(),
            "Spiral client ID and secret must be supplied together"
        );
        self.client_id = client_id;
        self.client_secret = client_secret;
        self
    }
}

impl fmt::Debug for SpiralJobConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpiralJobConnection")
            .field("api_url", &self.api_url)
            .field("spfs_url", &self.spfs_url)
            .field("project_id", &self.project_id)
            .field("table_prefix", &self.table_prefix)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingImageSize {
    image_width: usize,
    image_height: usize,
}

impl TrainingImageSize {
    pub fn new(image_width: usize, image_height: usize) -> Self {
        assert!(
            (image_width == 0 && image_height == 0) || (image_width > 0 && image_height > 0),
            "training image dimensions must both be zero for native resolution or both be positive"
        );
        Self {
            image_width,
            image_height,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActTrainingInput {
    camera_streams: Vec<String>,
    #[serde(flatten)]
    image_size: TrainingImageSize,
    video_decoder: String,
}

impl ActTrainingInput {
    pub fn new(
        camera_streams: Vec<String>,
        image_size: TrainingImageSize,
        video_decoder: impl Into<String>,
    ) -> Self {
        let input = Self {
            camera_streams,
            image_size,
            video_decoder: video_decoder.into(),
        };
        assert!(
            input
                .camera_streams
                .iter()
                .all(|stream| !stream.trim().is_empty()),
            "camera stream names must not be empty"
        );
        assert!(
            !input.video_decoder.trim().is_empty(),
            "video decoder must not be empty"
        );
        input
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActModelDimensions {
    hidden_dimensions: usize,
    latent_dimensions: usize,
    feedforward_dimensions: usize,
}

impl ActModelDimensions {
    pub fn new(
        hidden_dimensions: usize,
        latent_dimensions: usize,
        feedforward_dimensions: usize,
    ) -> Self {
        assert!(
            hidden_dimensions > 0,
            "ACT hidden dimensions must be positive"
        );
        assert!(
            latent_dimensions > 0,
            "ACT latent dimensions must be positive"
        );
        assert!(
            feedforward_dimensions > 0,
            "ACT feedforward dimensions must be positive"
        );
        Self {
            hidden_dimensions,
            latent_dimensions,
            feedforward_dimensions,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActLayerCounts {
    encoder_layers: usize,
    vae_encoder_layers: usize,
    decoder_layers: usize,
}

impl ActLayerCounts {
    pub fn new(encoder_layers: usize, vae_encoder_layers: usize, decoder_layers: usize) -> Self {
        assert!(
            encoder_layers > 0,
            "ACT encoder layer count must be positive"
        );
        assert!(
            vae_encoder_layers > 0,
            "ACT VAE encoder layer count must be positive"
        );
        assert!(
            decoder_layers > 0,
            "ACT decoder layer count must be positive"
        );
        Self {
            encoder_layers,
            vae_encoder_layers,
            decoder_layers,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActTrainingArchitecture {
    #[serde(flatten)]
    dimensions: ActModelDimensions,
    attention_heads: usize,
    #[serde(flatten)]
    layers: ActLayerCounts,
}

impl ActTrainingArchitecture {
    pub fn new(
        dimensions: ActModelDimensions,
        attention_heads: usize,
        layers: ActLayerCounts,
    ) -> Self {
        assert!(
            attention_heads > 0,
            "ACT attention head count must be positive"
        );
        assert!(
            dimensions.hidden_dimensions.is_multiple_of(attention_heads),
            "ACT hidden dimensions must be divisible by the attention head count"
        );
        Self {
            dimensions,
            attention_heads,
            layers,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActTrainingJobConfig {
    #[serde(flatten)]
    input: ActTrainingInput,
    #[serde(flatten)]
    architecture: ActTrainingArchitecture,
    kl_weight: f64,
}

impl ActTrainingJobConfig {
    pub fn new(
        input: ActTrainingInput,
        architecture: ActTrainingArchitecture,
        kl_weight: f64,
    ) -> Self {
        assert!(
            kl_weight.is_finite() && kl_weight >= 0.0,
            "ACT KL weight must be finite and non-negative"
        );
        Self {
            input,
            architecture,
            kl_weight,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobIdentity {
    job_id: String,
    run_name: String,
}

impl TrainingJobIdentity {
    pub fn new(job_id: impl Into<String>, run_name: impl Into<String>) -> Self {
        let identity = Self {
            job_id: job_id.into(),
            run_name: run_name.into(),
        };
        assert!(
            !identity.job_id.trim().is_empty(),
            "job ID must not be empty"
        );
        assert!(
            !identity.run_name.trim().is_empty(),
            "run name must not be empty"
        );
        identity
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingDataColumns {
    state_column: String,
    action_column: String,
}

impl TrainingDataColumns {
    pub fn new(state_column: impl Into<String>, action_column: impl Into<String>) -> Self {
        let columns = Self {
            state_column: state_column.into(),
            action_column: action_column.into(),
        };
        assert!(
            !columns.state_column.trim().is_empty(),
            "state column must not be empty"
        );
        assert!(
            !columns.action_column.trim().is_empty(),
            "action column must not be empty"
        );
        columns
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingDataQuery {
    sql: String,
    #[serde(flatten)]
    columns: TrainingDataColumns,
    spiral_schema_version: u32,
}

impl TrainingDataQuery {
    pub fn new(
        sql: impl Into<String>,
        columns: TrainingDataColumns,
        spiral_schema_version: u32,
    ) -> Self {
        let query = Self {
            sql: sql.into(),
            columns,
            spiral_schema_version,
        };
        assert!(
            !query.sql.trim().is_empty(),
            "training SQL must not be empty"
        );
        assert!(
            query.spiral_schema_version > 0,
            "Spiral schema version must be positive"
        );
        query
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingDataSelection {
    spiral: SpiralJobConnection,
    #[serde(flatten)]
    query: TrainingDataQuery,
}

impl TrainingDataSelection {
    pub fn new(spiral: SpiralJobConnection, query: TrainingDataQuery) -> Self {
        Self { spiral, query }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingRuntimeSelection {
    device: String,
    initialization: TrainingInitialization,
}

impl TrainingRuntimeSelection {
    pub fn new(device: impl Into<String>, initialization: TrainingInitialization) -> Self {
        let selection = Self {
            device: device.into(),
            initialization,
        };
        assert!(
            !selection.device.trim().is_empty(),
            "training device must not be empty"
        );
        selection
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingProgress {
    steps: u64,
    batch_size: usize,
    validation_split: f64,
}

impl TrainingProgress {
    pub fn new(steps: u64, batch_size: usize, validation_split: f64) -> Self {
        assert!(steps > 0, "training step count must be positive");
        assert!(batch_size > 0, "training batch size must be positive");
        assert!(
            validation_split.is_finite() && (0.0..1.0).contains(&validation_split),
            "validation split must be finite and in [0, 1)"
        );
        Self {
            steps,
            batch_size,
            validation_split,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingOptimizer {
    learning_rate: f64,
    seed: u64,
}

impl TrainingOptimizer {
    pub fn new(learning_rate: f64, seed: u64) -> Self {
        assert!(
            learning_rate.is_finite() && learning_rate > 0.0,
            "learning rate must be finite and positive"
        );
        Self {
            learning_rate,
            seed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingCheckpointSchedule {
    checkpoint_every: u64,
    chunk_size: usize,
}

impl TrainingCheckpointSchedule {
    pub fn new(checkpoint_every: u64, chunk_size: usize) -> Self {
        assert!(checkpoint_every > 0, "checkpoint interval must be positive");
        assert!(chunk_size > 0, "action chunk size must be positive");
        Self {
            checkpoint_every,
            chunk_size,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingSchedule {
    #[serde(flatten)]
    progress: TrainingProgress,
    #[serde(flatten)]
    optimizer: TrainingOptimizer,
    #[serde(flatten)]
    checkpoints: TrainingCheckpointSchedule,
}

impl TrainingSchedule {
    pub fn new(
        progress: TrainingProgress,
        optimizer: TrainingOptimizer,
        checkpoints: TrainingCheckpointSchedule,
    ) -> Self {
        assert!(
            checkpoints.checkpoint_every <= progress.steps,
            "checkpoint interval must not exceed the training step count"
        );
        Self {
            progress,
            optimizer,
            checkpoints,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobPlan {
    #[serde(flatten)]
    runtime: TrainingRuntimeSelection,
    #[serde(flatten)]
    schedule: TrainingSchedule,
    act: ActTrainingJobConfig,
}

impl TrainingJobPlan {
    pub fn new(
        runtime: TrainingRuntimeSelection,
        schedule: TrainingSchedule,
        act: ActTrainingJobConfig,
    ) -> Self {
        Self {
            runtime,
            schedule,
            act,
        }
    }
}

/// An immutable, validated job request ready for serialization to a worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobRequest {
    #[serde(flatten)]
    identity: TrainingJobIdentity,
    #[serde(flatten)]
    data: TrainingDataSelection,
    #[serde(flatten)]
    plan: TrainingJobPlan,
}

impl TrainingJobRequest {
    pub fn new(
        identity: TrainingJobIdentity,
        data: TrainingDataSelection,
        plan: TrainingJobPlan,
    ) -> Self {
        Self {
            identity,
            data,
            plan,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingJobPhase {
    Starting,
    Running,
    Stopping,
    Completed,
    Stopped,
    Failed,
}

impl TrainingJobPhase {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Stopped | Self::Failed)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ResolvedObservation {
    camera_streams: Vec<String>,
    state_dimensions: usize,
    image_width: usize,
    image_height: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ResolvedAction {
    action_dimensions: usize,
    sample_hz: f64,
    robot_type: String,
    joint_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ResolvedRuntime {
    parameter_count: usize,
    device: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobResolved {
    #[serde(flatten)]
    observation: ResolvedObservation,
    #[serde(flatten)]
    action: ResolvedAction,
    #[serde(flatten)]
    runtime: ResolvedRuntime,
}

impl TrainingJobResolved {
    pub fn camera_streams(&self) -> &[String] {
        &self.observation.camera_streams
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TrainingMetricProgress {
    step: u64,
    train_loss: f64,
    validation_loss: Option<f64>,
    learning_rate: f64,
    examples_per_second: f64,
    event: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TrainingResourceMetrics {
    cpu_percent: Option<f64>,
    gpu_percent: Option<f64>,
    memory_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TrainingTimingMetrics {
    loader_ms: Option<f64>,
    device_copy_ms: Option<f64>,
    model_ms: Option<f64>,
    elapsed_seconds: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobMetric {
    #[serde(flatten)]
    progress: TrainingMetricProgress,
    #[serde(flatten)]
    resources: TrainingResourceMetrics,
    #[serde(flatten)]
    timings: TrainingTimingMetrics,
}

impl TrainingJobMetric {
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
        self.progress.examples_per_second
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

    pub const fn elapsed_seconds(&self) -> f64 {
        self.timings.elapsed_seconds
    }

    pub fn event(&self) -> Option<&str> {
        self.progress.event.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobCheckpoint {
    step: u64,
    created_ms: i64,
    validation_loss: Option<f64>,
    byte_size: u64,
    recommended_action_steps: usize,
    sample_hz: f64,
}

impl TrainingJobCheckpoint {
    pub const fn step(&self) -> u64 {
        self.step
    }

    pub const fn created_ms(&self) -> i64 {
        self.created_ms
    }

    pub const fn validation_loss(&self) -> Option<f64> {
        self.validation_loss
    }

    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }

    pub const fn recommended_action_steps(&self) -> usize {
        self.recommended_action_steps
    }

    pub const fn sample_hz(&self) -> f64 {
        self.sample_hz
    }
}

/// Append-only worker history position. Callers send the last observed cursor
/// so polling transfers only new state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobCursor {
    metric_step: u64,
    checkpoint_step: u64,
    log_offset: u64,
}

impl TrainingJobCursor {
    pub const fn metric_step(self) -> u64 {
        self.metric_step
    }

    pub const fn checkpoint_step(self) -> u64 {
        self.checkpoint_step
    }

    pub const fn log_offset(self) -> u64 {
        self.log_offset
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobLog {
    offset: u64,
    line: String,
}

impl TrainingJobLog {
    pub fn line(&self) -> &str {
        &self.line
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TrainingSnapshotState {
    job_id: String,
    phase: TrainingJobPhase,
    current_step: u64,
    finished_ms: Option<i64>,
    message: Option<String>,
    resolved: Option<TrainingJobResolved>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TrainingSnapshotReplay {
    metrics: Vec<TrainingJobMetric>,
    checkpoints: Vec<TrainingJobCheckpoint>,
    logs: Vec<TrainingJobLog>,
    /// Cursor after all deltas in this response.
    #[serde(default)]
    cursor: TrainingJobCursor,
    /// Present when the requested cursor predates bounded worker retention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    history_gap: Option<String>,
}

/// Current worker state plus replay deltas after the requested cursor.
///
/// Collections are append-only but retained within worker-side bounds. A
/// caller that falls behind those bounds receives `history_gap` alongside the
/// oldest still-available deltas and can continue from `cursor`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobSnapshot {
    #[serde(flatten)]
    state: TrainingSnapshotState,
    #[serde(flatten)]
    replay: TrainingSnapshotReplay,
}

impl TrainingJobSnapshot {
    pub fn job_id(&self) -> &str {
        &self.state.job_id
    }

    pub const fn phase(&self) -> TrainingJobPhase {
        self.state.phase
    }

    pub const fn current_step(&self) -> u64 {
        self.state.current_step
    }

    pub fn message(&self) -> Option<&str> {
        self.state.message.as_deref()
    }

    pub const fn resolved(&self) -> Option<&TrainingJobResolved> {
        self.state.resolved.as_ref()
    }

    pub fn metrics(&self) -> &[TrainingJobMetric] {
        &self.replay.metrics
    }

    pub fn checkpoints(&self) -> &[TrainingJobCheckpoint] {
        &self.replay.checkpoints
    }

    pub fn logs(&self) -> &[TrainingJobLog] {
        &self.replay.logs
    }

    pub const fn cursor(&self) -> TrainingJobCursor {
        self.replay.cursor
    }

    pub fn history_gap(&self) -> Option<&str> {
        self.replay.history_gap.as_deref()
    }
}
