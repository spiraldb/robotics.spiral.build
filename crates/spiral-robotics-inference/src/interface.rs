use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Result;

/// Policy family implemented by an engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    Act,
}

/// Pixel encoding accepted at the framework-independent API boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageFormat {
    /// Interleaved red, green, and blue bytes.
    Rgb8,
}

/// A borrowed image plane. `row_stride` allows callers to pass camera buffers
/// without repacking them first.
#[derive(Clone, Copy, Debug)]
pub struct ImageFrame<'a> {
    pub name: &'a str,
    pub width: usize,
    pub height: usize,
    pub row_stride: usize,
    pub format: ImageFormat,
    pub data: &'a [u8],
}

impl<'a> ImageFrame<'a> {
    pub fn packed_rgb8(name: &'a str, width: usize, height: usize, data: &'a [u8]) -> Result<Self> {
        let row_stride = width.checked_mul(3).ok_or_else(|| {
            crate::Error::Input("packed RGB row stride overflows usize".to_owned())
        })?;
        Ok(Self {
            name,
            width,
            height,
            row_stride,
            format: ImageFormat::Rgb8,
            data,
        })
    }
}

/// One synchronous policy observation.
#[derive(Clone, Copy, Debug)]
pub struct InferenceInput<'a> {
    pub state: &'a [f32],
    pub images: &'a [ImageFrame<'a>],
}

/// Stable model contract that the integration layer can validate before any
/// physical rollout starts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PolicySpecification {
    pub kind: PolicyKind,
    pub state_dimensions: usize,
    pub image_inputs: Vec<ImageInputSpecification>,
    pub action_dimensions: usize,
    pub action_chunk_size: usize,
    /// Checkpoint-recommended prefix to consume before requesting a new chunk.
    pub recommended_action_steps: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageInputSpecification {
    pub name: String,
    pub channels: usize,
    pub height: usize,
    pub width: usize,
}

/// Runtime measurements for one completed forward pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct InferenceStats {
    pub preprocessing: Duration,
    /// Backend execution including synchronization and device-to-host transfer.
    pub model: Duration,
    pub postprocessing: Duration,
    pub total: Duration,
}

/// Row-major `[step, action_dimension]` action predictions.
#[derive(Clone, Debug)]
pub struct ActionChunk {
    values: Vec<f32>,
    steps: usize,
    action_dimensions: usize,
    pub stats: InferenceStats,
}

impl ActionChunk {
    /// Build a row-major action chunk returned by a custom [`Policy`].
    ///
    /// Keeping this constructor fallible lets runtime integrations implement
    /// the public policy interface without being able to publish a chunk whose
    /// storage disagrees with its declared shape.
    pub fn try_new(
        values: Vec<f32>,
        steps: usize,
        action_dimensions: usize,
        stats: InferenceStats,
    ) -> Result<Self> {
        let expected = steps.checked_mul(action_dimensions).ok_or_else(|| {
            crate::Error::Inference("action chunk dimensions overflow usize".to_owned())
        })?;
        if values.len() != expected {
            return Err(crate::Error::Inference(format!(
                "model returned {} action values, expected {steps} × {action_dimensions} = {expected}",
                values.len()
            )));
        }
        Ok(Self {
            values,
            steps,
            action_dimensions,
            stats,
        })
    }

    pub fn steps(&self) -> usize {
        self.steps
    }

    pub fn action_dimensions(&self) -> usize {
        self.action_dimensions
    }

    pub fn action(&self, step: usize) -> Option<&[f32]> {
        let start = step.checked_mul(self.action_dimensions)?;
        let end = start.checked_add(self.action_dimensions)?;
        self.values.get(start..end)
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.values
    }

    pub fn into_values(self) -> Vec<f32> {
        self.values
    }
}

/// Framework-neutral interface implemented by every policy family.
pub trait Policy: Send {
    fn identity(&self) -> &crate::ModelIdentity;
    fn specification(&self) -> &PolicySpecification;

    /// Materialize backend kernels and validate one representative forward
    /// pass before the policy is admitted to a deadline-sensitive control path.
    fn warm_up(&mut self) -> Result<InferenceStats>;

    /// Produce a full action chunk. Scheduling, chunk consumption, temporal
    /// ensembling, and safety validation remain responsibilities of the caller.
    fn infer(&mut self, input: InferenceInput<'_>) -> Result<ActionChunk>;

    /// Clear policy-local episode state. Stateless chunk predictors need no
    /// work here, while future ensemble/streaming policies can override it.
    fn reset(&mut self) {}
}

/// Materializes one policy inside the process that owns the tensor framework.
///
/// HTTP transports depend only on this framework-neutral boundary. Studio can
/// therefore use a remote policy without linking the loader implementation.
pub trait PolicyLoader: Send + Sync {
    fn load(&self, artifact: crate::ModelArtifact) -> Result<Box<dyn Policy>>;
}

#[cfg(test)]
mod tests {
    use super::{ActionChunk, ImageFrame, InferenceStats};

    #[test]
    fn action_chunks_cannot_misrepresent_their_storage() {
        assert!(ActionChunk::try_new(vec![0.0; 13], 1, 14, InferenceStats::default()).is_err());
        assert!(ActionChunk::try_new(vec![0.0; 14], 1, 14, InferenceStats::default()).is_ok());
    }

    #[test]
    fn packed_images_report_stride_overflow() {
        assert!(ImageFrame::packed_rgb8("camera", usize::MAX, 1, &[]).is_err());
    }
}
