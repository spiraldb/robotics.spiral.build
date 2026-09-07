//! Native policy-training contracts and bounded run history.

use serde::{Deserialize, Serialize};

/// The policy family executed by the native training runtime.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActTrainingConfig {
    #[serde(flatten)]
    input: ActInputConfig,
    #[serde(flatten)]
    architecture: ActArchitectureConfig,
    #[serde(flatten)]
    attention: ActAttentionConfig,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActInputConfig {
    /// Camera stream IDs that must be present for every sampled anchor.
    /// An empty list discovers every camera stream shared by the selected
    /// dataset episodes, matching LeRobot's feature-driven policy inputs.
    #[serde(default = "ActInputConfig::default_camera_streams")]
    camera_streams: Vec<String>,
    /// Requested image width. Zero preserves the recorded camera width.
    #[serde(default = "ActInputConfig::default_image_size")]
    image_width: usize,
    /// Requested image height. Zero preserves the recorded camera height.
    #[serde(default = "ActInputConfig::default_image_size")]
    image_height: usize,
    /// Spiral video decoder implementation: auto, ffmpeg, videotoolbox, or nvdec.
    #[serde(default = "ActInputConfig::default_video_decoder")]
    video_decoder: String,
    /// Use the ImageNet initialization expected by vision ACT.
    #[serde(default = "ActInputConfig::default_pretrained_backbone")]
    pretrained_backbone: bool,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActArchitectureConfig {
    #[serde(default = "ActArchitectureConfig::default_hidden_dim")]
    hidden_dim: usize,
    #[serde(default = "ActArchitectureConfig::default_latent_dim")]
    latent_dim: usize,
    #[serde(default = "ActArchitectureConfig::default_encoder_layers")]
    encoder_layers: usize,
    #[serde(default = "ActArchitectureConfig::default_vae_encoder_layers")]
    vae_encoder_layers: usize,
    #[serde(default = "ActArchitectureConfig::default_decoder_layers")]
    decoder_layers: usize,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActLayerCounts {
    encoder: usize,
    variational_encoder: usize,
    decoder: usize,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActAttentionConfig {
    #[serde(default = "ActAttentionConfig::default_heads")]
    attention_heads: usize,
    #[serde(default = "ActAttentionConfig::default_feedforward_dim")]
    feedforward_dim: usize,
    #[serde(default = "ActAttentionConfig::default_kl_weight")]
    kl_weight: f64,
}

impl ActInputConfig {
    fn default_camera_streams() -> Vec<String> {
        Vec::new()
    }

    const fn default_image_size() -> usize {
        0
    }

    fn default_video_decoder() -> String {
        "auto".to_owned()
    }

    const fn default_pretrained_backbone() -> bool {
        true
    }
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

    pub const fn with_pretrained_backbone(mut self, pretrained_backbone: bool) -> Self {
        self.pretrained_backbone = pretrained_backbone;
        self
    }
}

impl ActArchitectureConfig {
    const fn default_hidden_dim() -> usize {
        512
    }

    const fn default_latent_dim() -> usize {
        32
    }

    const fn default_encoder_layers() -> usize {
        4
    }

    const fn default_vae_encoder_layers() -> usize {
        4
    }

    const fn default_decoder_layers() -> usize {
        // The original ACT requested seven decoder layers but returned only the
        // first layer's output. LeRobot preserves that effective architecture.
        1
    }
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
    const fn default_heads() -> usize {
        8
    }

    const fn default_feedforward_dim() -> usize {
        3_200
    }

    const fn default_kl_weight() -> f64 {
        10.0
    }
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
                camera_streams: ActInputConfig::default_camera_streams(),
                image_width: ActInputConfig::default_image_size(),
                image_height: ActInputConfig::default_image_size(),
                video_decoder: ActInputConfig::default_video_decoder(),
                pretrained_backbone: ActInputConfig::default_pretrained_backbone(),
            },
            architecture: ActArchitectureConfig {
                hidden_dim: ActArchitectureConfig::default_hidden_dim(),
                latent_dim: ActArchitectureConfig::default_latent_dim(),
                encoder_layers: ActArchitectureConfig::default_encoder_layers(),
                vae_encoder_layers: ActArchitectureConfig::default_vae_encoder_layers(),
                decoder_layers: ActArchitectureConfig::default_decoder_layers(),
            },
            attention: ActAttentionConfig {
                attention_heads: ActAttentionConfig::default_heads(),
                feedforward_dim: ActAttentionConfig::default_feedforward_dim(),
                kl_weight: ActAttentionConfig::default_kl_weight(),
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
    pub const fn pretrained_backbone(&self) -> bool {
        self.input.pretrained_backbone
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

/// Authoritative lifecycle of one backend-owned native training run.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// Everything needed to reproduce one run. Spiral is the only sample source.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrainingConfig {
    #[serde(flatten)]
    selection: TrainingDataSelection,
    #[serde(flatten)]
    optimization: TrainingOptimizationConfig,
    #[serde(default)]
    act: ActTrainingConfig,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TrainingDataSelection {
    run_name: String,
    /// Relational dataset selected by the operator. The configured Spiral
    /// namespace remains a storage location; this ID is the training set.
    #[serde(default = "TrainingDataSelection::default_dataset_id")]
    dataset_id: String,
    #[serde(default)]
    policy: TrainingPolicy,
    sql: String,
    #[serde(default = "TrainingDataSelection::default_state_column")]
    state_column: String,
    #[serde(default = "TrainingDataSelection::default_action_column")]
    action_column: String,
    #[serde(default = "TrainingDataSelection::default_device")]
    device: String,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct TrainingOptimizationConfig {
    #[serde(default = "TrainingOptimizationConfig::default_steps")]
    steps: u64,
    #[serde(default = "TrainingOptimizationConfig::default_batch_size")]
    batch_size: usize,
    /// Fraction of covered episodes held out from optimizer updates.
    ///
    /// Missing values deserialize to zero so historical run records keep their
    /// original no-validation semantics. Newly created configurations default to 10%.
    #[serde(default)]
    validation_split: f64,
    #[serde(default = "TrainingOptimizationConfig::default_learning_rate")]
    learning_rate: f64,
    #[serde(default = "TrainingOptimizationConfig::default_checkpoint_every")]
    checkpoint_every: u64,
    #[serde(default = "TrainingOptimizationConfig::default_chunk_size")]
    chunk_size: usize,
    #[serde(default = "TrainingOptimizationConfig::default_seed")]
    seed: u64,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            selection: TrainingDataSelection {
                run_name: "act-run".to_owned(),
                dataset_id: TrainingDataSelection::default_dataset_id(),
                policy: TrainingPolicy::Act,
                sql: String::new(),
                state_column: TrainingDataSelection::default_state_column(),
                action_column: TrainingDataSelection::default_action_column(),
                device: TrainingDataSelection::default_device(),
            },
            optimization: TrainingOptimizationConfig {
                steps: TrainingOptimizationConfig::default_steps(),
                batch_size: TrainingOptimizationConfig::default_batch_size(),
                validation_split: TrainingOptimizationConfig::default_validation_split(),
                learning_rate: TrainingOptimizationConfig::default_learning_rate(),
                checkpoint_every: TrainingOptimizationConfig::default_checkpoint_every(),
                chunk_size: TrainingOptimizationConfig::default_chunk_size(),
                seed: TrainingOptimizationConfig::default_seed(),
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
    pub const fn seed(&self) -> u64 {
        self.optimization.seed
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
    pub const fn set_seed(&mut self, seed: u64) {
        self.optimization.seed = seed;
    }
    pub const fn set_chunk_size(&mut self, chunk_size: usize) {
        self.optimization.chunk_size = chunk_size;
    }
}

impl TrainingDataSelection {
    fn default_dataset_id() -> String {
        "default".to_owned()
    }

    fn default_state_column() -> String {
        "state".to_owned()
    }

    fn default_action_column() -> String {
        "action".to_owned()
    }

    fn default_device() -> String {
        "auto".to_owned()
    }

    pub(crate) fn new(
        run_name: impl Into<String>,
        dataset_id: impl Into<String>,
        sql: impl Into<String>,
    ) -> Self {
        Self {
            run_name: run_name.into(),
            dataset_id: dataset_id.into(),
            policy: TrainingPolicy::default(),
            sql: sql.into(),
            state_column: Self::default_state_column(),
            action_column: Self::default_action_column(),
            device: Self::default_device(),
        }
    }
}

impl TrainingOptimizationConfig {
    const fn default_steps() -> u64 {
        100_000
    }

    const fn default_batch_size() -> usize {
        8
    }

    const fn default_validation_split() -> f64 {
        0.1
    }

    const fn default_learning_rate() -> f64 {
        1.0e-5
    }

    const fn default_checkpoint_every() -> u64 {
        20_000
    }

    const fn default_chunk_size() -> usize {
        100
    }

    const fn default_seed() -> u64 {
        1_000
    }
}

/// One sample on the shared training-step axis used by every Studio plot.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingMetricPoint {
    #[serde(flatten)]
    progress: TrainingMetricProgress,
    #[serde(flatten)]
    resources: TrainingResourceMetrics,
    #[serde(flatten)]
    timings: TrainingStageTimings,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingMetricProgress {
    step: u64,
    train_loss: f64,
    validation_loss: Option<f64>,
    learning_rate: f64,
    elapsed_seconds: f64,
    #[serde(default)]
    event: Option<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingResourceMetrics {
    /// Latest requested video payload bytes divided by total sample preparation time.
    transfer_bytes_per_second: f64,
    examples_per_second: f64,
    /// Studio process CPU usage; 100% represents one fully occupied core.
    #[serde(default)]
    cpu_percent: Option<f64>,
    /// Whole-device GPU utilization when the active backend exposes it.
    #[serde(default)]
    gpu_percent: Option<f64>,
    /// Current resident memory used by the Studio process.
    #[serde(default)]
    memory_bytes: Option<u64>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingStageTimings {
    /// Spiral query, frame decode, normalization, and batch assembly time.
    #[serde(default)]
    loader_ms: Option<f64>,
    /// Studio-side Arrow IPC materialization time.
    #[serde(default)]
    arrow_encode_ms: Option<f64>,
    /// HTTP request/response time outside the remote runtime handler.
    #[serde(default)]
    upload_ms: Option<f64>,
    /// Runtime-side conversion and host-to-device transfer time.
    #[serde(default)]
    device_copy_ms: Option<f64>,
    /// Runtime-side optimizer or validation compute time.
    #[serde(default)]
    model_ms: Option<f64>,
}

impl TrainingMetricPoint {
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
                transfer_bytes_per_second: 0.0,
                examples_per_second: 0.0,
                cpu_percent: None,
                gpu_percent: None,
                memory_bytes: None,
            },
            timings: TrainingStageTimings {
                loader_ms: None,
                arrow_encode_ms: None,
                upload_ms: None,
                device_copy_ms: None,
                model_ms: None,
            },
        }
    }

    pub const fn with_learning_rate(mut self, learning_rate: f64) -> Self {
        self.progress.learning_rate = learning_rate;
        self
    }

    pub const fn with_validation_loss(mut self, validation_loss: Option<f64>) -> Self {
        self.progress.validation_loss = validation_loss;
        self
    }

    pub fn with_timeline(mut self, elapsed_seconds: f64, event: Option<String>) -> Self {
        self.progress.elapsed_seconds = elapsed_seconds;
        self.progress.event = event;
        self
    }

    pub const fn with_throughput(
        mut self,
        transfer_bytes_per_second: f64,
        examples_per_second: f64,
    ) -> Self {
        self.resources.transfer_bytes_per_second = transfer_bytes_per_second;
        self.resources.examples_per_second = examples_per_second;
        self
    }

    pub const fn with_utilization(
        mut self,
        cpu_percent: Option<f64>,
        gpu_percent: Option<f64>,
    ) -> Self {
        self.resources.cpu_percent = cpu_percent;
        self.resources.gpu_percent = gpu_percent;
        self
    }

    pub const fn with_memory_bytes(mut self, memory_bytes: Option<u64>) -> Self {
        self.resources.memory_bytes = memory_bytes;
        self
    }

    pub const fn with_loader_ms(mut self, loader_ms: Option<f64>) -> Self {
        self.timings.loader_ms = loader_ms;
        self
    }

    pub const fn with_transport_timings(
        mut self,
        arrow_encode_ms: Option<f64>,
        upload_ms: Option<f64>,
    ) -> Self {
        self.timings.arrow_encode_ms = arrow_encode_ms;
        self.timings.upload_ms = upload_ms;
        self
    }

    pub const fn with_runtime_timings(
        mut self,
        device_copy_ms: Option<f64>,
        model_ms: Option<f64>,
    ) -> Self {
        self.timings.device_copy_ms = device_copy_ms;
        self.timings.model_ms = model_ms;
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
    pub const fn elapsed_seconds(&self) -> f64 {
        self.progress.elapsed_seconds
    }
    pub const fn transfer_bytes_per_second(&self) -> f64 {
        self.resources.transfer_bytes_per_second
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
    pub const fn arrow_encode_ms(&self) -> Option<f64> {
        self.timings.arrow_encode_ms
    }
    pub const fn upload_ms(&self) -> Option<f64> {
        self.timings.upload_ms
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

/// An inference checkpoint durably ingested into Spiral.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingCheckpoint {
    step: u64,
    created_ms: i64,
    validation_loss: Option<f64>,
    byte_size: u64,
    /// Old run-history entries default to false and cannot be selected for rollout.
    #[serde(default)]
    inference_ready: bool,
    /// Number of actions to execute before refreshing the observation.
    #[serde(default)]
    recommended_action_steps: usize,
    /// Control frequency inherited from the recorded dataset.
    #[serde(default)]
    sample_hz: f64,
}

impl TrainingCheckpoint {
    pub const fn new(step: u64, created_ms: i64) -> Self {
        Self {
            step,
            created_ms,
            validation_loss: None,
            byte_size: 0,
            inference_ready: false,
            recommended_action_steps: 0,
            sample_hz: 0.0,
        }
    }
    pub const fn with_validation_loss(mut self, validation_loss: Option<f64>) -> Self {
        self.validation_loss = validation_loss;
        self
    }
    pub const fn with_artifact(mut self, byte_size: u64, inference_ready: bool) -> Self {
        self.byte_size = byte_size;
        self.inference_ready = inference_ready;
        self
    }
    pub const fn with_rollout(mut self, recommended_action_steps: usize, sample_hz: f64) -> Self {
        self.recommended_action_steps = recommended_action_steps;
        self.sample_hz = sample_hz;
        self
    }
    pub const fn step(&self) -> u64 {
        self.step
    }
    pub const fn created_ms(&self) -> i64 {
        self.created_ms
    }
    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }
    pub const fn validation_loss(&self) -> Option<f64> {
        self.validation_loss
    }
    pub const fn inference_ready(&self) -> bool {
        self.inference_ready
    }
    pub const fn recommended_action_steps(&self) -> usize {
        self.recommended_action_steps
    }
    pub const fn sample_hz(&self) -> f64 {
        self.sample_hz
    }
}

/// Dataset selection captured when a run starts. Project and namespace come
/// from Studio's one process-owned Spiral destination.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingSource {
    /// Relational dataset row whose episodes supplied this run.
    #[serde(default = "TrainingSource::default_dataset_id")]
    dataset_id: String,
}

impl TrainingSource {
    fn default_dataset_id() -> String {
        "default".to_owned()
    }

    pub fn new(dataset_id: impl Into<String>) -> Self {
        Self {
            dataset_id: dataset_id.into(),
        }
    }

    pub fn dataset_id(&self) -> &str {
        &self.dataset_id
    }
}

/// Result of checking every dependency needed before a native run can start.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingReadiness {
    ready: bool,
    spiral_ready: bool,
    #[serde(default)]
    runtime: Option<TrainingRuntimeInfo>,
    summary: String,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrainingRuntimeInfo {
    id: String,
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
    pub fn new(
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

/// Browser-safe run state. Checkpoint bytes and task handles never cross the
/// wire; only their durable Spiral identity does.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingRun {
    #[serde(flatten)]
    identity: TrainingRunIdentity,
    #[serde(flatten)]
    lifecycle: TrainingRunLifecycle,
    #[serde(flatten)]
    history: TrainingRunHistory,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingRunIdentity {
    id: String,
    config: TrainingConfig,
    #[serde(default)]
    source: TrainingSource,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingRunLifecycle {
    phase: TrainingRunPhase,
    current_step: u64,
    started_ms: i64,
    finished_ms: Option<i64>,
    message: Option<String>,
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct TrainingRunHistory {
    metrics: Vec<TrainingMetricPoint>,
    checkpoints: Vec<TrainingCheckpoint>,
    logs: Vec<String>,
}

impl TrainingRun {
    pub fn starting(id: impl Into<String>, config: TrainingConfig) -> Self {
        Self {
            identity: TrainingRunIdentity {
                id: id.into(),
                config,
                source: TrainingSource::default(),
            },
            lifecycle: TrainingRunLifecycle {
                phase: TrainingRunPhase::Starting,
                current_step: 0,
                started_ms: 0,
                finished_ms: None,
                message: None,
            },
            history: TrainingRunHistory::default(),
        }
    }

    pub fn with_source(mut self, source: TrainingSource) -> Self {
        self.identity.source = source;
        self
    }

    pub const fn with_phase(mut self, phase: TrainingRunPhase, current_step: u64) -> Self {
        self.lifecycle.phase = phase;
        self.lifecycle.current_step = current_step;
        self
    }

    pub const fn with_started_ms(mut self, started_ms: i64) -> Self {
        self.lifecycle.started_ms = started_ms;
        self
    }

    pub fn with_completion(mut self, finished_ms: Option<i64>, message: Option<String>) -> Self {
        self.lifecycle.finished_ms = finished_ms;
        self.lifecycle.message = message;
        self
    }

    pub fn with_metrics(mut self, metrics: Vec<TrainingMetricPoint>) -> Self {
        self.history.metrics = metrics;
        self
    }

    pub fn with_checkpoints(mut self, checkpoints: Vec<TrainingCheckpoint>) -> Self {
        self.history.checkpoints = checkpoints;
        self
    }

    pub fn with_logs(mut self, logs: Vec<String>) -> Self {
        self.history.logs = logs;
        self
    }

    pub fn id(&self) -> &str {
        &self.identity.id
    }
    pub const fn config(&self) -> &TrainingConfig {
        &self.identity.config
    }
    pub const fn phase(&self) -> TrainingRunPhase {
        self.lifecycle.phase
    }
    pub const fn current_step(&self) -> u64 {
        self.lifecycle.current_step
    }
    pub fn message(&self) -> Option<&str> {
        self.lifecycle.message.as_deref()
    }
    pub fn metrics(&self) -> &[TrainingMetricPoint] {
        &self.history.metrics
    }
    pub fn checkpoints(&self) -> &[TrainingCheckpoint] {
        &self.history.checkpoints
    }
    pub fn logs(&self) -> &[String] {
        &self.history.logs
    }
    pub const fn started_ms(&self) -> i64 {
        self.lifecycle.started_ms
    }
    pub const fn finished_ms(&self) -> Option<i64> {
        self.lifecycle.finished_ms
    }
    pub const fn source(&self) -> &TrainingSource {
        &self.identity.source
    }
}

/// Backend training state and its query projection.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TrainingSnapshot {
    #[serde(default)]
    readiness: TrainingReadiness,
    runs: Vec<TrainingRun>,
    /// Present on query projections, identifying the run with complete details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detailed_run_id: Option<String>,
}

impl TrainingSnapshot {
    pub fn new(readiness: TrainingReadiness, runs: Vec<TrainingRun>) -> Self {
        Self {
            readiness,
            runs,
            detailed_run_id: None,
        }
    }
    pub const fn readiness(&self) -> &TrainingReadiness {
        &self.readiness
    }
    /// The run catalog includes the latest metric and checkpoints; only the
    /// selected run carries chart history and logs. Never clone every history
    /// just to answer a polling request.
    pub fn for_display(&self, selected: Option<&str>) -> Self {
        let mut result = Self::new(
            self.readiness.clone(),
            self.runs
                .iter()
                .map(|run| {
                    if Some(run.id()) == selected {
                        return run.clone();
                    }
                    TrainingRun {
                        identity: run.identity.clone(),
                        lifecycle: run.lifecycle.clone(),
                        history: TrainingRunHistory {
                            metrics: run.metrics().last().cloned().into_iter().collect(),
                            checkpoints: run.checkpoints().to_vec(),
                            logs: Vec::new(),
                        },
                    }
                })
                .collect(),
        );
        result.detailed_run_id = selected
            .filter(|id| self.runs.iter().any(|run| run.id() == *id))
            .map(str::to_owned);
        result
    }
    pub fn detailed_run_id(&self) -> Option<&str> {
        self.detailed_run_id.as_deref()
    }

    pub fn runs(&self) -> &[TrainingRun] {
        &self.runs
    }
}

pub(crate) const MAX_TRAINING_METRIC_POINTS: usize = 2_400;
const TRAINING_METRIC_COMPACTION_TARGET: usize = MAX_TRAINING_METRIC_POINTS / 2;
const TRAINING_METRIC_SERIES_COUNT: usize = 12;
pub(crate) const MAX_TRAINING_LOG_LINES: usize = 120;
const MAX_TRAINING_CHECKPOINTS: usize = 1_000;
pub const MAX_TRAINING_HISTORY_RUNS: usize = 40;

/// Internal training state transition.
///
/// The backend folds these transitions into its retained run history.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
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
        config: Box<TrainingConfig>,
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
        self.runs.iter().find(|run| run.lifecycle.phase.active())
    }

    pub fn apply(&mut self, update: TrainingUpdate) {
        match update {
            TrainingUpdate::Snapshot { snapshot } => *self = *snapshot,
            TrainingUpdate::Readiness { readiness } => self.readiness = readiness,
            TrainingUpdate::RunAdded { run } => {
                self.runs
                    .retain(|existing| existing.identity.id != run.identity.id);
                self.runs.insert(0, *run);
                self.runs.truncate(MAX_TRAINING_HISTORY_RUNS);
            }
            TrainingUpdate::RunRemoved { run_id } => {
                self.runs.retain(|run| run.identity.id != run_id);
            }
            TrainingUpdate::RunConfigured { run_id, config } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.identity.id == run_id) {
                    run.identity.config = *config;
                }
            }
            TrainingUpdate::RunState {
                run_id,
                phase,
                current_step,
                finished_ms,
                message,
            } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.identity.id == run_id) {
                    run.lifecycle.phase = phase;
                    run.lifecycle.current_step = current_step;
                    run.lifecycle.finished_ms = finished_ms;
                    run.lifecycle.message = message;
                }
            }
            TrainingUpdate::Metric { run_id, point } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.identity.id == run_id) {
                    run.insert_metric(*point);
                }
            }
            TrainingUpdate::Checkpoint { run_id, checkpoint } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.identity.id == run_id) {
                    run.lifecycle.current_step = run.lifecycle.current_step.max(checkpoint.step);
                    run.history.checkpoints.push(checkpoint);
                    let excess = run
                        .history
                        .checkpoints
                        .len()
                        .saturating_sub(MAX_TRAINING_CHECKPOINTS);
                    if excess > 0 {
                        run.history.checkpoints.drain(..excess);
                    }
                }
            }
            TrainingUpdate::Log { run_id, line } => {
                if let Some(run) = self.runs.iter_mut().find(|run| run.identity.id == run_id) {
                    run.history.logs.push(line);
                    let excess = run
                        .history
                        .logs
                        .len()
                        .saturating_sub(MAX_TRAINING_LOG_LINES);
                    if excess > 0 {
                        run.history.logs.drain(..excess);
                    }
                }
            }
        }
    }
}

impl TrainingRun {
    fn insert_metric(&mut self, point: TrainingMetricPoint) {
        self.lifecycle.current_step = self.lifecycle.current_step.max(point.progress.step);
        match self
            .history
            .metrics
            .binary_search_by_key(&point.progress.step, |existing| existing.progress.step)
        {
            Ok(index) => self.history.metrics[index] = point,
            Err(index) => self.history.metrics.insert(index, point),
        }
        if self.history.metrics.len() > MAX_TRAINING_METRIC_POINTS {
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

        if self.history.metrics.len() <= TRAINING_METRIC_COMPACTION_TARGET {
            return;
        }

        let source = std::mem::take(&mut self.history.metrics);
        let mut keep = BTreeSet::from([0, source.len() - 1]);
        let event_indices = source
            .iter()
            .enumerate()
            .filter_map(|(index, point)| point.progress.event.is_some().then_some(index))
            .collect::<Vec<_>>();
        Self::retain_evenly(
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
        Self::retain_evenly(&mut keep, &all_indices, remaining);

        self.history.metrics = source
            .into_iter()
            .enumerate()
            .filter_map(|(index, point)| keep.contains(&index).then_some(point))
            .collect();
    }
    fn retain_evenly(
        keep: &mut std::collections::BTreeSet<usize>,
        candidates: &[usize],
        limit: usize,
    ) {
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
}

impl TrainingMetricPoint {
    fn series_value(&self, series_index: usize) -> Option<f64> {
        match series_index {
            0 => Some(self.progress.train_loss),
            1 => self.progress.validation_loss,
            2 => Some(self.resources.transfer_bytes_per_second),
            3 => Some(self.resources.examples_per_second),
            4 => self.resources.cpu_percent,
            5 => self.resources.gpu_percent,
            6 => self
                .resources
                .memory_bytes
                .map(|memory_bytes| memory_bytes as f64),
            7 => self.timings.loader_ms,
            8 => self.timings.arrow_encode_ms,
            9 => self.timings.upload_ms,
            10 => self.timings.device_copy_ms,
            11 => self.timings.model_ms,
            _ => None,
        }
    }
}

impl ActLayerCounts {
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

    struct TrainingSnapshotTest;

    impl TrainingSnapshotTest {
        fn run(id: &str) -> TrainingRun {
            TrainingRun::starting(id, TrainingConfig::default())
                .with_source(TrainingSource::default())
        }

        fn metric(step: u64, train_loss: f64) -> TrainingMetricPoint {
            TrainingMetricPoint::new(step, train_loss)
        }
    }

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
        assert_eq!(config.seed(), 1_000);
        assert_eq!(config.validation_split(), 0.1);
    }

    #[test]
    fn historical_training_configs_keep_no_validation_semantics() {
        let mut serialized_config = serde_json::to_value(TrainingConfig::default()).unwrap();
        serialized_config
            .as_object_mut()
            .unwrap()
            .remove("validation_split");
        let config: TrainingConfig = serde_json::from_value(serialized_config).unwrap();
        assert_eq!(config.validation_split(), 0.0);
    }

    #[test]
    fn incremental_training_updates_stay_bounded_without_dropping_early_history() {
        let mut snapshot = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![TrainingSnapshotTest::run("run-a")],
        );
        for step in 0..=12_000 {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(
                    TrainingMetricPoint::new(step, if step == 123 { 10_000.0 } else { 1.0 })
                        .with_validation_loss((step == 6_000).then_some(4.0))
                        .with_utilization((step == 777).then_some(9_000.0), None),
                ),
            });
        }
        for line in 0..=MAX_TRAINING_LOG_LINES {
            snapshot.apply(TrainingUpdate::Log {
                run_id: "run-a".to_owned(),
                line: format!("line {line}"),
            });
        }

        let run = &snapshot.runs()[0];
        assert!(run.metrics().len() <= MAX_TRAINING_METRIC_POINTS);
        assert_eq!(
            run.metrics().first().map(TrainingMetricPoint::step),
            Some(0)
        );
        assert_eq!(
            run.metrics().last().map(TrainingMetricPoint::step),
            Some(12_000)
        );
        assert!(run.metrics().iter().any(|point| point.step() == 123));
        assert!(run.metrics().iter().any(|point| point.step() == 777));
        assert!(run.metrics().iter().any(|point| point.step() == 6_000));
        assert_eq!(run.logs().len(), MAX_TRAINING_LOG_LINES);
        assert_eq!(run.logs()[0], "line 1");
        assert_eq!(run.current_step(), 12_000);
    }

    #[test]
    fn repeated_or_late_metric_updates_replace_the_step_without_regressing_progress() {
        let mut snapshot = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![TrainingSnapshotTest::run("run-a")],
        );
        for (step, loss) in [(2, 2.0), (1, 1.0), (2, 0.5)] {
            snapshot.apply(TrainingUpdate::Metric {
                run_id: "run-a".to_owned(),
                point: Box::new(TrainingSnapshotTest::metric(step, loss)),
            });
        }

        let run = &snapshot.runs()[0];
        assert_eq!(run.current_step(), 2);
        assert_eq!(run.metrics().len(), 2);
        assert_eq!(run.metrics()[0].step(), 1);
        assert_eq!(run.metrics()[1].step(), 2);
        assert_eq!(run.metrics()[1].train_loss(), 0.5);
    }

    #[test]
    fn resolved_training_configuration_updates_the_run_snapshot() {
        let mut snapshot = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![TrainingSnapshotTest::run("run-a")],
        );
        let mut config = TrainingConfig::default();
        config.act_mut().set_camera_streams(vec![
            "follower.camera.gripper".to_owned(),
            "follower.camera.top".to_owned(),
        ]);
        snapshot.apply(TrainingUpdate::RunConfigured {
            run_id: "run-a".to_owned(),
            config: Box::new(config.clone()),
        });
        assert_eq!(snapshot.runs()[0].config(), &config);
    }

    #[test]
    fn removing_a_training_run_removes_its_complete_snapshot_record() {
        let mut snapshot = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![
                TrainingRun::starting("run-a", TrainingConfig::default())
                    .with_source(TrainingSource::default())
                    .with_metrics(vec![TrainingMetricPoint::default()])
                    .with_checkpoints(vec![TrainingCheckpoint::default()])
                    .with_logs(vec!["retained only inside the run".to_owned()]),
                TrainingSnapshotTest::run("run-b"),
            ],
        );

        snapshot.apply(TrainingUpdate::RunRemoved {
            run_id: "run-a".to_owned(),
        });

        assert_eq!(snapshot.runs().len(), 1);
        assert_eq!(snapshot.runs()[0].id(), "run-b");
    }

    #[test]
    fn bootstrap_training_snapshot_replaces_the_placeholder_atomically() {
        let mut snapshot = TrainingSnapshot::default();
        let restored = TrainingSnapshot::new(
            TrainingReadiness::default(),
            vec![TrainingSnapshotTest::run("restored-run")],
        );

        snapshot.apply(TrainingUpdate::Snapshot {
            snapshot: Box::new(restored.clone()),
        });

        assert_eq!(snapshot, restored);
    }
    #[test]
    fn polling_sends_full_history_only_for_the_selected_run() {
        let runs = ["a", "b"]
            .into_iter()
            .map(|id| {
                TrainingRun::starting(id, TrainingConfig::default())
                    .with_metrics(vec![
                        TrainingMetricPoint::new(1, 2.0),
                        TrainingMetricPoint::new(2, 1.0),
                    ])
                    .with_logs(vec!["retained log".to_owned()])
                    .with_checkpoints(vec![TrainingCheckpoint::new(2, 1)])
            })
            .collect();
        let history = TrainingSnapshot::new(TrainingReadiness::default(), runs);
        let display = history.for_display(Some("b"));
        assert_eq!(display.runs()[0].metrics().len(), 1);
        assert!(display.runs()[0].logs().is_empty());
        assert_eq!(
            display.runs()[0].checkpoints(),
            history.runs()[0].checkpoints()
        );
        assert_eq!(display.runs()[1], history.runs()[1]);
        assert_eq!(history.runs()[0].metrics().len(), 2);
    }
}

/// Selects the one run whose bounded full history should be returned.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TrainingSelection {
    selected: Option<String>,
}
impl TrainingSelection {
    pub fn new(selected: Option<String>) -> Self {
        Self { selected }
    }
    pub fn into_selected(self) -> Option<String> {
        self.selected
    }
}
