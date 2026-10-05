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

    /// Decode the single-body form [`Self::envelope`] writes.
    pub fn from_envelope(envelope: &[u8]) -> InferenceResult<Self> {
        let malformed = || InferenceError::Artifact("policy envelope is truncated".to_owned());
        let (length, rest) = envelope.split_first_chunk::<8>().ok_or_else(malformed)?;
        let config_len = usize::try_from(u64::from_le_bytes(*length)).map_err(|_| malformed())?;
        if config_len > rest.len() {
            return Err(malformed());
        }
        let (config, weights) = rest.split_at(config_len);
        Self::from_bytes(config.to_vec(), weights.to_vec())
    }

    /// The single body a policy travels in between Studio, the robot
    /// connector, and the policy runtime: the config's length as a
    /// little-endian `u64`, the config, then the weights.
    pub fn envelope(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(8 + self.config.len() + self.weights.len());
        body.extend_from_slice(&(self.config.len() as u64).to_le_bytes());
        body.extend_from_slice(&self.config);
        body.extend_from_slice(&self.weights);
        body
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
    fn envelopes_round_trip_and_refuse_truncation() {
        let artifact = ModelArtifact::from_bytes(b"{}".to_vec(), b"weights".to_vec()).unwrap();
        let envelope = artifact.envelope();
        let decoded = ModelArtifact::from_envelope(&envelope).unwrap();
        assert_eq!(decoded.config(), b"{}");
        assert_eq!(decoded.weights(), b"weights");
        assert!(ModelArtifact::from_envelope(&envelope[..4]).is_err());
        assert!(ModelArtifact::from_envelope(&[9, 0, 0, 0, 0, 0, 0, 0, b'{']).is_err());
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
