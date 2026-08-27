use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::InferenceResult;

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

/// Width and height of one image plane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageDimensions {
    width: usize,
    height: usize,
}

impl ImageDimensions {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height }
    }

    pub fn width(self) -> usize {
        self.width
    }

    pub fn height(self) -> usize {
        self.height
    }
}

/// A borrowed image plane. `row_stride` allows callers to pass camera buffers
/// without repacking them first.
#[derive(Clone, Copy, Debug)]
pub struct ImageFrame<'a> {
    name: &'a str,
    dimensions: ImageDimensions,
    row_stride: usize,
    format: ImageFormat,
    data: &'a [u8],
}

impl<'a> ImageFrame<'a> {
    pub fn packed_rgb8(
        name: &'a str,
        dimensions: ImageDimensions,
        data: &'a [u8],
    ) -> InferenceResult<Self> {
        let row_stride = dimensions.width.checked_mul(3).ok_or_else(|| {
            crate::InferenceError::Input("packed RGB row stride overflows usize".to_owned())
        })?;
        let expected_byte_count = row_stride.checked_mul(dimensions.height).ok_or_else(|| {
            crate::InferenceError::Input("packed RGB image size overflows usize".to_owned())
        })?;
        if data.len() != expected_byte_count {
            return Err(crate::InferenceError::Input(format!(
                "packed RGB image contains {} bytes, expected {expected_byte_count}",
                data.len()
            )));
        }
        Ok(Self {
            name,
            dimensions,
            row_stride,
            format: ImageFormat::Rgb8,
            data,
        })
    }

    pub fn name(self) -> &'a str {
        self.name
    }

    pub fn dimensions(self) -> ImageDimensions {
        self.dimensions
    }

    pub fn row_stride(self) -> usize {
        self.row_stride
    }

    pub fn format(self) -> ImageFormat {
        self.format
    }

    pub fn data(self) -> &'a [u8] {
        self.data
    }
}

/// One synchronous policy observation.
#[derive(Clone, Copy, Debug)]
pub struct InferenceInput<'a> {
    state: &'a [f32],
    images: &'a [ImageFrame<'a>],
}

impl<'a> InferenceInput<'a> {
    pub fn new(state: &'a [f32], images: &'a [ImageFrame<'a>]) -> Self {
        Self { state, images }
    }

    pub fn state(self) -> &'a [f32] {
        self.state
    }

    pub fn images(self) -> &'a [ImageFrame<'a>] {
        self.images
    }
}

/// Stable model contract that the integration layer can validate before any
/// physical rollout starts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PolicySpecification {
    kind: PolicyKind,
    state_dimensions: usize,
    image_inputs: Vec<ImageInputSpecification>,
    action_dimensions: usize,
    action_chunk_size: usize,
    /// Checkpoint-recommended prefix to consume before requesting a new chunk.
    recommended_action_steps: usize,
}

impl PolicySpecification {
    pub fn state_dimensions(&self) -> usize {
        self.state_dimensions
    }

    pub fn image_inputs(&self) -> &[ImageInputSpecification] {
        &self.image_inputs
    }

    pub fn action_dimensions(&self) -> usize {
        self.action_dimensions
    }

    pub fn action_chunk_size(&self) -> usize {
        self.action_chunk_size
    }

    pub fn recommended_action_steps(&self) -> usize {
        self.recommended_action_steps
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        kind: PolicyKind,
        observation: ObservationSpecification,
        actions: ActionChunkSpecification,
    ) -> Self {
        Self {
            kind,
            state_dimensions: observation.state_dimensions,
            image_inputs: observation.image_inputs,
            action_dimensions: actions.action_dimensions,
            action_chunk_size: actions.chunk_size,
            recommended_action_steps: actions.recommended_steps,
        }
    }
}

/// Observation shape accepted by a policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationSpecification {
    state_dimensions: usize,
    image_inputs: Vec<ImageInputSpecification>,
}

/// Action shape and consumption recommendation produced by a policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionChunkSpecification {
    action_dimensions: usize,
    chunk_size: usize,
    recommended_steps: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageInputSpecification {
    name: String,
    channels: usize,
    height: usize,
    width: usize,
}

impl ImageInputSpecification {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn dimensions(&self) -> ImageDimensions {
        ImageDimensions::new(self.width, self.height)
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }
}

/// Runtime measurements for one completed forward pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct InferenceStats {
    preprocessing: Duration,
    /// Backend execution including synchronization and device-to-host transfer.
    model: Duration,
    postprocessing: Duration,
    total: Duration,
}

impl InferenceStats {
    pub fn with_preprocessing(mut self, preprocessing: Duration) -> Self {
        self.preprocessing = preprocessing;
        self
    }

    pub fn with_model(mut self, model: Duration) -> Self {
        self.model = model;
        self
    }

    pub fn with_postprocessing(mut self, postprocessing: Duration) -> Self {
        self.postprocessing = postprocessing;
        self
    }

    pub fn with_total(mut self, total: Duration) -> Self {
        self.total = total;
        self
    }

    pub fn model(self) -> Duration {
        self.model
    }

    pub fn total(self) -> Duration {
        self.total
    }
}

/// Validated row-major shape of an [`ActionChunk`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionChunkShape {
    steps: usize,
    action_dimensions: usize,
    value_count: usize,
}

impl ActionChunkShape {
    pub fn try_new(steps: usize, action_dimensions: usize) -> InferenceResult<Self> {
        let value_count = steps.checked_mul(action_dimensions).ok_or_else(|| {
            crate::InferenceError::Inference("action chunk dimensions overflow usize".to_owned())
        })?;
        Ok(Self {
            steps,
            action_dimensions,
            value_count,
        })
    }
}

/// Row-major `[step, action_dimension]` action predictions.
#[derive(Clone, Debug)]
pub struct ActionChunk {
    values: Vec<f32>,
    shape: ActionChunkShape,
    stats: InferenceStats,
}

impl ActionChunk {
    /// Build a row-major action chunk returned by a custom [`Policy`].
    ///
    /// Keeping this constructor fallible lets runtime integrations implement
    /// the public policy interface without being able to publish a chunk whose
    /// storage disagrees with its declared shape.
    pub fn try_new(
        values: Vec<f32>,
        shape: ActionChunkShape,
        stats: InferenceStats,
    ) -> InferenceResult<Self> {
        if values.len() != shape.value_count {
            return Err(crate::InferenceError::Inference(format!(
                "model returned {} action values, expected {} × {} = {}",
                values.len(),
                shape.steps,
                shape.action_dimensions,
                shape.value_count,
            )));
        }
        Ok(Self {
            values,
            shape,
            stats,
        })
    }

    pub fn steps(&self) -> usize {
        self.shape.steps
    }

    pub fn action_dimensions(&self) -> usize {
        self.shape.action_dimensions
    }

    pub fn action(&self, step: usize) -> Option<&[f32]> {
        let start = step.checked_mul(self.shape.action_dimensions)?;
        let end = start.checked_add(self.shape.action_dimensions)?;
        self.values.get(start..end)
    }

    pub fn stats(&self) -> InferenceStats {
        self.stats
    }
}

/// Framework-neutral interface implemented by every policy family.
pub trait Policy: Send {
    fn specification(&self) -> &PolicySpecification;

    /// Materialize backend kernels and validate one representative forward
    /// pass before the policy is admitted to a deadline-sensitive control path.
    fn warm_up(&mut self) -> InferenceResult<InferenceStats>;

    /// Produce a full action chunk. Scheduling, chunk consumption, temporal
    /// ensembling, and safety validation remain responsibilities of the caller.
    fn infer(&mut self, input: InferenceInput<'_>) -> InferenceResult<ActionChunk>;
}

/// Materializes one policy inside the process that owns the tensor framework.
///
/// HTTP transports depend only on this framework-neutral boundary. Studio can
/// therefore use a remote policy without linking the loader implementation.
pub trait PolicyLoader: Send + Sync {
    fn load(&self, artifact: crate::ModelArtifact) -> InferenceResult<Box<dyn Policy>>;
}

impl ActionChunkSpecification {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(action_dimensions: usize, chunk_size: usize, recommended_steps: usize) -> Self {
        Self {
            action_dimensions,
            chunk_size,
            recommended_steps,
        }
    }
}

impl ObservationSpecification {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(state_dimensions: usize, image_inputs: Vec<ImageInputSpecification>) -> Self {
        Self {
            state_dimensions,
            image_inputs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionChunk, ActionChunkShape, ImageDimensions, ImageFrame, InferenceStats};

    #[test]
    fn action_chunks_cannot_misrepresent_their_storage() {
        let shape = ActionChunkShape::try_new(1, 14).unwrap();
        assert!(ActionChunk::try_new(vec![0.0; 13], shape, InferenceStats::default()).is_err());
        assert!(ActionChunk::try_new(vec![0.0; 14], shape, InferenceStats::default()).is_ok());
    }

    #[test]
    fn packed_images_report_stride_overflow() {
        assert!(
            ImageFrame::packed_rgb8("camera", ImageDimensions::new(usize::MAX, 1), &[]).is_err()
        );
    }

    #[test]
    fn packed_images_require_exact_storage() {
        let dimensions = ImageDimensions::new(2, 2);
        assert!(ImageFrame::packed_rgb8("camera", dimensions, &[0; 11]).is_err());
        assert!(ImageFrame::packed_rgb8("camera", dimensions, &[0; 12]).is_ok());
    }
}
