use std::sync::Arc;

use crate::{InferenceError, InferenceResult};

const MAX_MODEL_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_MODEL_WEIGHTS_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Copy)]
enum ModelBytes {
    Configuration,
    Weights,
}

impl ModelBytes {
    const fn label(self) -> &'static str {
        match self {
            Self::Configuration => "model config",
            Self::Weights => "model weights",
        }
    }

    const fn maximum_bytes(self) -> usize {
        match self {
            Self::Configuration => MAX_MODEL_CONFIG_BYTES,
            Self::Weights => MAX_MODEL_WEIGHTS_BYTES,
        }
    }

    fn validate_length(self, byte_count: usize) -> InferenceResult<()> {
        let maximum_bytes = self.maximum_bytes();
        if byte_count > maximum_bytes {
            return Err(InferenceError::Artifact(format!(
                "{} is {byte_count} bytes, exceeding the {maximum_bytes}-byte limit",
                self.label()
            )));
        }
        Ok(())
    }
}

/// Owned model bytes admitted through bounded artifact validation.
#[derive(Clone)]
pub struct ModelArtifact {
    config: Arc<[u8]>,
    weights: Arc<[u8]>,
}

impl std::fmt::Debug for ModelArtifact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelArtifact")
            .field("config_bytes", &self.config.len())
            .field("weights_bytes", &self.weights.len())
            .finish()
    }
}

impl ModelArtifact {
    pub fn from_bytes(config: Vec<u8>, weights: Vec<u8>) -> InferenceResult<Self> {
        ModelBytes::Configuration.validate_length(config.len())?;
        ModelBytes::Weights.validate_length(weights.len())?;

        Ok(Self {
            config: config.into(),
            weights: weights.into(),
        })
    }

    pub fn config(&self) -> &[u8] {
        &self.config
    }

    pub fn weights(&self) -> &[u8] {
        &self.weights
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_MODEL_CONFIG_BYTES, ModelArtifact, ModelBytes};

    #[test]
    fn in_memory_artifacts_own_the_admitted_bytes() {
        let artifact = ModelArtifact::from_bytes(b"{}".to_vec(), b"weights".to_vec()).unwrap();
        assert_eq!(artifact.config(), b"{}");
        assert_eq!(artifact.weights(), b"weights");
    }

    #[test]
    fn model_byte_limits_are_inclusive() {
        assert!(
            ModelBytes::Configuration
                .validate_length(MAX_MODEL_CONFIG_BYTES)
                .is_ok()
        );
        assert!(
            ModelBytes::Configuration
                .validate_length(MAX_MODEL_CONFIG_BYTES + 1)
                .is_err()
        );
    }
}
