//! Backend-neutral model-training boundary.
//!
//! Studio owns orchestration and the visible run history. A job backend owns
//! SpiralDB reads, model execution, telemetry, and artifact publication. The
//! older session/batch types remain for rollout compatibility and isolated
//! runtime tests; no framework tensor type crosses this seam.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

pub const TRAINING_BACKEND_API_VERSION: u32 = 4;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingBackendInfo {
    pub api_version: u32,
    pub id: String,
    pub display_name: String,
    pub runtime: String,
    pub devices: Vec<String>,
}

impl TrainingBackendInfo {
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · {}",
            self.display_name,
            self.runtime,
            self.devices.join(" + ")
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum TrainingModelConfig {
    Act(ActTrainingConfig),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActTrainingConfig {
    pub state_dimensions: usize,
    pub action_dimensions: usize,
    pub camera_streams: Vec<String>,
    pub image_width: usize,
    pub image_height: usize,
    pub chunk_size: usize,
    pub hidden_dimensions: usize,
    pub latent_dimensions: usize,
    pub attention_heads: usize,
    pub feedforward_dimensions: usize,
    pub encoder_layers: usize,
    pub vae_encoder_layers: usize,
    pub decoder_layers: usize,
    pub kl_weight: f64,
    pub learning_rate: f64,
    pub total_steps: u64,
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingInitialization {
    Random,
    PretrainedBackbone,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingSessionRequest {
    pub model: TrainingModelConfig,
    pub device: String,
    pub initialization: TrainingInitialization,
}

/// One immutable connection selection sent only to the training worker. The
/// client secret is write-only control-plane material and must never appear in
/// job snapshots, logs, or telemetry.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SpiralJobConnection {
    pub api_url: String,
    pub spfs_url: String,
    pub project_id: String,
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl fmt::Debug for SpiralJobConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpiralJobConnection")
            .field("api_url", &self.api_url)
            .field("spfs_url", &self.spfs_url)
            .field("project_id", &self.project_id)
            .field("namespace", &self.namespace)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActTrainingJobConfig {
    pub camera_streams: Vec<String>,
    pub image_width: usize,
    pub image_height: usize,
    pub video_decoder: String,
    pub hidden_dimensions: usize,
    pub latent_dimensions: usize,
    pub attention_heads: usize,
    pub feedforward_dimensions: usize,
    pub encoder_layers: usize,
    pub vae_encoder_layers: usize,
    pub decoder_layers: usize,
    pub kl_weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobRequest {
    pub job_id: String,
    pub run_name: String,
    pub spiral: SpiralJobConnection,
    pub sql: String,
    pub state_column: String,
    pub action_column: String,
    pub device: String,
    pub initialization: TrainingInitialization,
    pub steps: u64,
    pub batch_size: usize,
    pub validation_split: f64,
    pub learning_rate: f64,
    pub checkpoint_every: u64,
    pub chunk_size: usize,
    pub seed: u64,
    pub act: ActTrainingJobConfig,
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
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Stopped | Self::Failed)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobResolved {
    pub camera_streams: Vec<String>,
    pub state_dimensions: usize,
    pub action_dimensions: usize,
    pub image_width: usize,
    pub image_height: usize,
    pub sample_hz: f64,
    pub robot_type: String,
    pub joint_names: Vec<String>,
    pub parameter_count: usize,
    pub device: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobMetric {
    pub step: u64,
    pub train_loss: f64,
    pub validation_loss: Option<f64>,
    pub learning_rate: f64,
    pub examples_per_second: f64,
    pub cpu_percent: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub loader_ms: Option<f64>,
    pub device_copy_ms: Option<f64>,
    pub model_ms: Option<f64>,
    pub elapsed_seconds: f64,
    pub event: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobCheckpoint {
    pub step: u64,
    pub created_ms: i64,
    pub validation_loss: Option<f64>,
    pub byte_size: u64,
    pub recommended_action_steps: usize,
    pub sample_hz: f64,
}

/// Complete, replayable worker state. Collections are append-only for the
/// lifetime of a job, allowing Studio to reattach after a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJobSnapshot {
    pub job_id: String,
    pub phase: TrainingJobPhase,
    pub current_step: u64,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
    pub resolved: Option<TrainingJobResolved>,
    pub metrics: Vec<TrainingJobMetric>,
    pub checkpoints: Vec<TrainingJobCheckpoint>,
    pub logs: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct TrainingBatch<'a> {
    pub batch_size: usize,
    pub states: Cow<'a, [f32]>,
    /// Interleaved HWC RGB pixels. The runtime owns device-side conversion,
    /// CHW permutation, float32 casting, and model normalization.
    pub images: Cow<'a, [u8]>,
    pub actions: Cow<'a, [f32]>,
    /// One byte per action timestep: `0` for data and `1` for padding.
    pub action_is_pad: Cow<'a, [u8]>,
}

impl TrainingBatch<'static> {
    pub fn owned(
        batch_size: usize,
        states: Vec<f32>,
        images: Vec<u8>,
        actions: Vec<f32>,
        action_is_pad: Vec<u8>,
    ) -> Self {
        Self {
            batch_size,
            states: Cow::Owned(states),
            images: Cow::Owned(images),
            actions: Cow::Owned(actions),
            action_is_pad: Cow::Owned(action_is_pad),
        }
    }
}

impl<'a> TrainingBatch<'a> {
    pub fn borrowed(
        batch_size: usize,
        states: &'a [f32],
        images: &'a [u8],
        actions: &'a [f32],
        action_is_pad: &'a [u8],
    ) -> Self {
        Self {
            batch_size,
            states: Cow::Borrowed(states),
            images: Cow::Borrowed(images),
            actions: Cow::Borrowed(actions),
            action_is_pad: Cow::Borrowed(action_is_pad),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingLoss {
    pub total: f64,
    pub l1: f64,
    pub kl: f64,
    pub pad: f64,
}

/// Latest process and accelerator evidence reported by a training runtime.
///
/// Every field is optional because not every framework or device exposes the
/// same counters. A remote adapter forwards these values from the process that
/// actually owns the optimizer rather than sampling the Studio process.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingTelemetry {
    pub cpu_percent: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    /// Studio-side Arrow IPC materialization for the most recent batch.
    #[serde(default)]
    pub arrow_encode_ms: Option<f64>,
    /// HTTP request/response time outside the remote runtime handler. For a
    /// remote endpoint this is dominated by request-body upload.
    #[serde(default)]
    pub upload_ms: Option<f64>,
    /// Runtime-side conversion and host-to-device transfer.
    #[serde(default)]
    pub device_copy_ms: Option<f64>,
    /// Runtime-side optimizer or validation compute after inputs reach the device.
    #[serde(default)]
    pub model_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrainingStepOutput {
    pub loss: TrainingLoss,
    pub learning_rate: f64,
    pub telemetry: TrainingTelemetry,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingCheckpointRequest {
    pub manifest_json: String,
    pub state_mean: Vec<f32>,
    pub state_std: Vec<f32>,
    pub action_mean: Vec<f32>,
    pub action_std: Vec<f32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingCheckpointArtifact {
    pub format: String,
    pub extension: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingSessionInfo {
    pub backend: TrainingBackendInfo,
    pub device: String,
    pub parameter_count: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct TrainingBackendError {
    message: String,
}

impl TrainingBackendError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Factory and health boundary owned by Studio as `Arc<dyn TrainingBackend>`.
///
/// Calls are made from worker threads, so a remote implementation may perform
/// blocking transport I/O without blocking Studio's async control loop.
pub trait TrainingBackend: Send + Sync {
    fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError>;

    fn create_session(
        &self,
        request: TrainingSessionRequest,
    ) -> Result<Box<dyn TrainingSession>, TrainingBackendError>;

    fn create_job(
        &self,
        _request: TrainingJobRequest,
    ) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        Err(TrainingBackendError::new(
            "this training backend does not support worker-owned jobs",
        ))
    }

    fn job(&self, _job_id: &str) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        Err(TrainingBackendError::new(
            "this training backend does not support worker-owned jobs",
        ))
    }

    fn stop_job(&self, _job_id: &str) -> Result<TrainingJobSnapshot, TrainingBackendError> {
        Err(TrainingBackendError::new(
            "this training backend does not support worker-owned jobs",
        ))
    }
}

/// A remotely executing optimizer step. Starting a step releases the session
/// handle so Studio can fill its bounded upload buffer while compute runs.
pub trait PendingTrainingStep: Send {
    fn finish(self: Box<Self>) -> Result<TrainingStepOutput, TrainingBackendError>;
}

/// One stateful optimizer session. No framework tensor type crosses this seam.
pub trait TrainingSession: Send {
    fn info(&self) -> &TrainingSessionInfo;

    /// Upload and retain exactly one batch for a future optimizer step.
    fn buffer_training_batch(
        &mut self,
        batch: TrainingBatch<'static>,
        step: u64,
    ) -> Result<(), TrainingBackendError>;

    /// Consume the buffered batch and begin the optimizer step asynchronously.
    fn begin_training_step(
        &mut self,
        step: u64,
    ) -> Result<Box<dyn PendingTrainingStep>, TrainingBackendError>;

    fn validation_step(
        &mut self,
        batch: TrainingBatch<'static>,
        after_step: u64,
    ) -> Result<TrainingStepOutput, TrainingBackendError>;

    fn checkpoint(
        &self,
        request: TrainingCheckpointRequest,
    ) -> Result<TrainingCheckpointArtifact, TrainingBackendError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RemoteBackend;

    impl TrainingBackend for RemoteBackend {
        fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
            Ok(TrainingBackendInfo {
                api_version: TRAINING_BACKEND_API_VERSION,
                id: "remote.test".to_owned(),
                display_name: "Remote test backend".to_owned(),
                runtime: "HTTP".to_owned(),
                devices: vec!["remote".to_owned()],
            })
        }

        fn create_session(
            &self,
            _request: TrainingSessionRequest,
        ) -> Result<Box<dyn TrainingSession>, TrainingBackendError> {
            Err(TrainingBackendError::new("not connected"))
        }
    }

    #[test]
    fn boundary_is_object_safe_and_remote_implementable() {
        let backend: std::sync::Arc<dyn TrainingBackend> = std::sync::Arc::new(RemoteBackend);
        assert_eq!(backend.health().unwrap().id, "remote.test");
    }

    #[test]
    fn job_connection_debug_output_redacts_the_client_secret() {
        let connection = SpiralJobConnection {
            api_url: "https://api.spiraldb.dev".to_owned(),
            spfs_url: "https://spfs.spiraldb.dev".to_owned(),
            project_id: "project".to_owned(),
            namespace: "spiral_robotics".to_owned(),
            client_id: Some("client-id".to_owned()),
            client_secret: Some("do-not-log-this".to_owned()),
        };
        let debug = format!("{connection:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("do-not-log-this"));
    }

    #[test]
    fn batches_can_own_or_borrow_framework_neutral_buffers() {
        let owned = TrainingBatch::owned(1, vec![0.25], vec![128], vec![0.75], vec![0]);
        let borrowed = TrainingBatch::borrowed(
            1,
            owned.states.as_ref(),
            owned.images.as_ref(),
            owned.actions.as_ref(),
            owned.action_is_pad.as_ref(),
        );
        assert_eq!(borrowed, owned);
    }
}
