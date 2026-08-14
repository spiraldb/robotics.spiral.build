//! Backend-neutral model-training boundary.
//!
//! Studio owns datasets, run lifecycle, metrics, and artifact publication. A
//! backend owns model state and optimizer execution. The contract uses owned,
//! serializable values so an implementation may execute locally or proxy each
//! operation to a remote service without exposing framework types to Studio.

use serde::{Deserialize, Serialize};

pub const TRAINING_BACKEND_API_VERSION: u32 = 1;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingBatch {
    pub batch_size: usize,
    pub states: Vec<f32>,
    pub images: Vec<f32>,
    pub actions: Vec<f32>,
    pub action_is_pad: Vec<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingLoss {
    pub total: f64,
    pub l1: f64,
    pub kl: f64,
    pub pad: f64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingExecution {
    LocalCpu,
    LocalAccelerator,
    Remote,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrainingSessionInfo {
    pub backend: TrainingBackendInfo,
    pub device: String,
    pub execution: TrainingExecution,
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

    pub fn message(&self) -> &str {
        &self.message
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
}

/// One stateful optimizer session. No framework tensor type crosses this seam.
pub trait TrainingSession: Send {
    fn info(&self) -> &TrainingSessionInfo;

    fn learning_rate(&self, step: u64) -> f64;

    fn train_step(
        &mut self,
        batch: TrainingBatch,
        step: u64,
    ) -> Result<TrainingLoss, TrainingBackendError>;

    fn validation_step(
        &mut self,
        batch: TrainingBatch,
    ) -> Result<TrainingLoss, TrainingBackendError>;

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
    fn wire_contract_round_trips_without_framework_types() {
        let batch = TrainingBatch {
            batch_size: 1,
            states: vec![0.25],
            images: vec![0.5],
            actions: vec![0.75],
            action_is_pad: vec![false],
        };
        let encoded = serde_json::to_vec(&batch).unwrap();
        let decoded: TrainingBatch = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, batch);
    }
}
