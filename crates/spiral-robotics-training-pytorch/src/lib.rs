//! Direct LibTorch implementation of the backend-neutral training contract.

use spiral_robotics_inference::act::{
    NativeActTrainer, VisionActBatch, VisionActCheckpoint, VisionActTrainingConfig,
    imagenet_resnet18_weights,
};
use spiral_robotics_training::{
    ActTrainingConfig, TRAINING_BACKEND_API_VERSION, TrainingBackend, TrainingBackendError,
    TrainingBackendInfo, TrainingBatch, TrainingCheckpointArtifact, TrainingCheckpointRequest,
    TrainingExecution, TrainingInitialization, TrainingLoss, TrainingModelConfig, TrainingSession,
    TrainingSessionInfo, TrainingSessionRequest,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct PyTorchTrainingBackend;

impl PyTorchTrainingBackend {
    pub const fn new() -> Self {
        Self
    }
}

impl TrainingBackend for PyTorchTrainingBackend {
    fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
        let mut devices = vec!["CPU".to_owned()];
        #[cfg(target_os = "macos")]
        if spiral_robotics_inference::act::metal_available() {
            devices.insert(0, "MPS".to_owned());
        }
        Ok(TrainingBackendInfo {
            api_version: TRAINING_BACKEND_API_VERSION,
            id: "pytorch.libtorch".to_owned(),
            display_name: "PyTorch".to_owned(),
            runtime: "LibTorch 2.9".to_owned(),
            devices,
        })
    }

    fn create_session(
        &self,
        request: TrainingSessionRequest,
    ) -> Result<Box<dyn TrainingSession>, TrainingBackendError> {
        let TrainingModelConfig::Act(config) = request.model;
        let pretrained = match request.initialization {
            TrainingInitialization::Random => None,
            TrainingInitialization::PretrainedBackbone => Some(
                imagenet_resnet18_weights()
                    .map_err(|error| backend_error("loading pretrained ResNet18", error))?,
            ),
        };
        let trainer =
            NativeActTrainer::new(act_config(config), &request.device, pretrained.as_deref())
                .map_err(|error| backend_error("creating ACT session", error))?;
        let device = trainer.backend_name().to_owned();
        let execution = if device == "MPS" {
            TrainingExecution::LocalAccelerator
        } else {
            TrainingExecution::LocalCpu
        };
        let info = TrainingSessionInfo {
            backend: self.health()?,
            device,
            execution,
            parameter_count: trainer.parameter_count(),
        };
        Ok(Box::new(PyTorchTrainingSession { trainer, info }))
    }
}

struct PyTorchTrainingSession {
    trainer: NativeActTrainer,
    info: TrainingSessionInfo,
}

impl TrainingSession for PyTorchTrainingSession {
    fn info(&self) -> &TrainingSessionInfo {
        &self.info
    }

    fn learning_rate(&self, step: u64) -> f64 {
        self.trainer.learning_rate(step)
    }

    fn train_step(
        &mut self,
        batch: TrainingBatch,
        step: u64,
    ) -> Result<TrainingLoss, TrainingBackendError> {
        self.trainer
            .train_step(vision_batch(batch), step)
            .map(training_loss)
            .map_err(|error| backend_error("running ACT optimizer step", error))
    }

    fn validation_step(
        &mut self,
        batch: TrainingBatch,
    ) -> Result<TrainingLoss, TrainingBackendError> {
        self.trainer
            .validation_step(vision_batch(batch))
            .map(training_loss)
            .map_err(|error| backend_error("running ACT validation", error))
    }

    fn checkpoint(
        &self,
        request: TrainingCheckpointRequest,
    ) -> Result<TrainingCheckpointArtifact, TrainingBackendError> {
        let payload = self
            .trainer
            .checkpoint(VisionActCheckpoint {
                manifest_json: &request.manifest_json,
                state_mean: &request.state_mean,
                state_std: &request.state_std,
                action_mean: &request.action_mean,
                action_std: &request.action_std,
            })
            .map_err(|error| backend_error("creating ACT checkpoint", error))?;
        Ok(TrainingCheckpointArtifact {
            format: "safetensors".to_owned(),
            extension: "safetensors".to_owned(),
            payload,
        })
    }
}

fn act_config(config: ActTrainingConfig) -> VisionActTrainingConfig {
    VisionActTrainingConfig {
        state_dimensions: config.state_dimensions,
        action_dimensions: config.action_dimensions,
        camera_streams: config.camera_streams,
        image_width: config.image_width,
        image_height: config.image_height,
        chunk_size: config.chunk_size,
        hidden_dimensions: config.hidden_dimensions,
        latent_dimensions: config.latent_dimensions,
        attention_heads: config.attention_heads,
        feedforward_dimensions: config.feedforward_dimensions,
        encoder_layers: config.encoder_layers,
        vae_encoder_layers: config.vae_encoder_layers,
        decoder_layers: config.decoder_layers,
        kl_weight: config.kl_weight,
        learning_rate: config.learning_rate,
        total_steps: config.total_steps,
        seed: config.seed,
    }
}

fn vision_batch(batch: TrainingBatch) -> VisionActBatch {
    VisionActBatch {
        batch_size: batch.batch_size,
        states: batch.states,
        images: batch.images,
        actions: batch.actions,
        action_is_pad: batch.action_is_pad,
    }
}

fn training_loss(loss: spiral_robotics_inference::act::VisionActLoss) -> TrainingLoss {
    TrainingLoss {
        total: loss.total,
        l1: loss.l1,
        kl: loss.kl,
        pad: loss.pad,
    }
}

fn backend_error(context: &str, error: impl std::fmt::Display) -> TrainingBackendError {
    TrainingBackendError::new(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pytorch_adapter_satisfies_the_object_safe_boundary() {
        let backend: std::sync::Arc<dyn TrainingBackend> =
            std::sync::Arc::new(PyTorchTrainingBackend::new());
        let health = backend.health().unwrap();
        assert_eq!(health.id, "pytorch.libtorch");
        assert!(health.devices.iter().any(|device| device == "CPU"));
    }

    #[test]
    fn adapter_executes_a_complete_backend_neutral_step_and_checkpoint() {
        let backend = PyTorchTrainingBackend::new();
        let mut session = backend
            .create_session(TrainingSessionRequest {
                model: TrainingModelConfig::Act(ActTrainingConfig {
                    state_dimensions: 2,
                    action_dimensions: 2,
                    camera_streams: vec!["wrist".to_owned()],
                    image_width: 32,
                    image_height: 32,
                    chunk_size: 2,
                    hidden_dimensions: 64,
                    latent_dimensions: 4,
                    attention_heads: 8,
                    feedforward_dimensions: 128,
                    encoder_layers: 1,
                    vae_encoder_layers: 1,
                    decoder_layers: 1,
                    kl_weight: 1.0,
                    learning_rate: 1.0e-5,
                    total_steps: 1,
                    seed: 7,
                }),
                device: "cpu".to_owned(),
                initialization: TrainingInitialization::Random,
            })
            .unwrap();
        let loss = session
            .train_step(
                TrainingBatch {
                    batch_size: 1,
                    states: vec![0.0; 2],
                    images: vec![0.0; 3 * 32 * 32],
                    actions: vec![0.0; 2 * 2],
                    action_is_pad: vec![false; 2],
                },
                1,
            )
            .unwrap();
        assert!(loss.total.is_finite());
        let artifact = session
            .checkpoint(TrainingCheckpointRequest {
                manifest_json: "{}".to_owned(),
                state_mean: vec![0.0; 2],
                state_std: vec![1.0; 2],
                action_mean: vec![0.0; 2],
                action_std: vec![1.0; 2],
            })
            .unwrap();
        assert_eq!(artifact.format, "safetensors");
        assert!(!artifact.payload.is_empty());
    }
}
