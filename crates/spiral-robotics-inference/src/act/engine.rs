use std::time::Instant;

use burn_core::tensor::backend::Backend;
use burn_core::tensor::{Tensor, TensorData};

use crate::{
    ActionChunk, Error, ImageFormat, ImageView, InferenceInput, InferenceStats, ModelArtifact,
    ModelFiles, ModelIdentity, Policy, PolicySpecification, Result,
};

use super::config::ActConfig;
use super::model::ActModel;
use super::weights::Weights;

const NORMALIZATION_EPSILON: f32 = 1.0e-8;

#[cfg(feature = "cpu")]
pub type CpuActPolicy = ActPolicy<burn_ndarray::NdArray<f32>>;

#[cfg(feature = "metal")]
pub type MetalActPolicy = ActPolicy<burn_wgpu::Metal>;

/// Probe Metal without initializing Burn's global compute runtime. WGPU's
/// higher-level Burn setup panics when adapter discovery returns nothing, so
/// callers use this fallible boundary before selecting the backend.
#[cfg(feature = "metal")]
pub fn metal_available() -> bool {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    !futures_lite::future::block_on(instance.enumerate_adapters(wgpu::Backends::METAL)).is_empty()
}

#[cfg(feature = "metal")]
fn ensure_metal_available() -> Result<()> {
    metal_available()
        .then_some(())
        .ok_or_else(|| Error::Inference("no Metal compute adapter is available".to_owned()))
}

/// Load ACT with the portable CPU runtime.
#[cfg(feature = "cpu")]
pub fn load_cpu(files: &ModelFiles) -> Result<CpuActPolicy> {
    ActPolicy::load(files, Default::default())
}

#[cfg(feature = "cpu")]
pub fn load_cpu_artifact(artifact: ModelArtifact) -> Result<CpuActPolicy> {
    ActPolicy::from_artifact(artifact, Default::default())
}

/// Load ACT with Burn's WGPU backend restricted to Metal by the crate feature.
#[cfg(feature = "metal")]
pub fn load_metal(files: &ModelFiles) -> Result<MetalActPolicy> {
    ensure_metal_available()?;
    ActPolicy::load(files, Default::default())
}

#[cfg(feature = "metal")]
pub fn load_metal_artifact(artifact: ModelArtifact) -> Result<MetalActPolicy> {
    ensure_metal_available()?;
    ActPolicy::from_artifact(artifact, Default::default())
}

/// Stateless ACT chunk predictor. The generic backend is deliberately kept
/// behind convenience constructors for normal callers, while tests and future
/// deployments can select another Burn backend explicitly.
pub struct ActPolicy<B: Backend> {
    identity: ModelIdentity,
    specification: PolicySpecification,
    model: ActModel<B>,
    normalization: Normalization,
    device: B::Device,
}

impl<B: Backend> ActPolicy<B> {
    pub fn load(files: &ModelFiles, device: B::Device) -> Result<Self> {
        Self::from_artifact(files.load()?, device)
    }

    pub fn from_artifact(artifact: ModelArtifact, device: B::Device) -> Result<Self> {
        let (config_bytes, weights_bytes, weights_label, identity) = artifact.into_parts();
        let config = ActConfig::from_slice(&config_bytes)?;
        let specification = config.specification();
        let weights = Weights::from_bytes(weights_label, weights_bytes)?;
        let normalization = Normalization::load(&config, &weights)?;
        let model = ActModel::load(&config, &weights, &device)?;
        Ok(Self {
            identity,
            specification,
            model,
            normalization,
            device,
        })
    }

    fn prepare(&self, input: InferenceInput<'_>) -> Result<(Tensor<B, 2>, Vec<Tensor<B, 4>>)> {
        if input.state.len() != self.specification.state_dimensions {
            return Err(Error::Input(format!(
                "state has {} values, model expects {}",
                input.state.len(),
                self.specification.state_dimensions
            )));
        }
        if input.state.iter().any(|value| !value.is_finite()) {
            return Err(Error::Input("state contains a non-finite value".to_owned()));
        }
        if input.images.len() != self.specification.image_inputs.len() {
            return Err(Error::Input(format!(
                "received {} images, model expects {}",
                input.images.len(),
                self.specification.image_inputs.len()
            )));
        }

        let state = input
            .state
            .iter()
            .zip(&self.normalization.state_mean)
            .zip(&self.normalization.state_std)
            .map(|((&value, &mean), &std)| normalize(value, mean, std))
            .collect::<Vec<_>>();
        let state = Tensor::from_data(
            TensorData::new(state, [1, self.specification.state_dimensions]),
            &self.device,
        );

        let mut prepared_images = Vec::with_capacity(self.specification.image_inputs.len());
        for (index, image_spec) in self.specification.image_inputs.iter().enumerate() {
            let image = required_image(input.images, &image_spec.name)?;
            validate_image(image, image_spec.width, image_spec.height)?;
            let plane = image.width * image.height;
            let mut pixels = vec![0.0; plane * 3];
            for y in 0..image.height {
                let row = &image.data[y * image.row_stride..y * image.row_stride + image.width * 3];
                for x in 0..image.width {
                    for channel in 0..3 {
                        let value = row[x * 3 + channel] as f32 / 255.0;
                        pixels[channel * plane + y * image.width + x] = normalize(
                            value,
                            self.normalization.image_mean[index][channel],
                            self.normalization.image_std[index][channel],
                        );
                    }
                }
            }
            prepared_images.push(Tensor::from_data(
                TensorData::new(pixels, [1, 3, image.height, image.width]),
                &self.device,
            ));
        }
        Ok((state, prepared_images))
    }
}

impl<B: Backend> Policy for ActPolicy<B> {
    fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    fn specification(&self) -> &PolicySpecification {
        &self.specification
    }

    fn warm_up(&mut self) -> Result<InferenceStats> {
        let state = vec![0.0; self.specification.state_dimensions];
        let image_inputs = self.specification.image_inputs.clone();
        let pixels = image_inputs
            .iter()
            .map(|image| vec![0; image.width * image.height * image.channels])
            .collect::<Vec<_>>();
        let image_views = image_inputs
            .iter()
            .zip(&pixels)
            .map(|(image, pixels)| {
                ImageView::packed_rgb8(&image.name, image.width, image.height, pixels)
            })
            .collect::<Result<Vec<_>>>()?;
        self.infer(InferenceInput {
            state: &state,
            images: &image_views,
        })
        .map(|chunk| chunk.stats)
    }

    fn infer(&mut self, input: InferenceInput<'_>) -> Result<ActionChunk> {
        let started = Instant::now();
        let (state, images) = self.prepare(input)?;
        let preprocessing = started.elapsed();
        let model_started = Instant::now();
        let output = self.model.forward(state, images);
        let output_shape = output.dims();
        let expected_shape = [
            1,
            self.specification.action_chunk_size,
            self.specification.action_dimensions,
        ];
        if output_shape != expected_shape {
            return Err(Error::Inference(format!(
                "model returned shape {output_shape:?}, expected {expected_shape:?}"
            )));
        }
        let mut values = output
            .try_into_data()
            .map_err(|error| Error::Inference(error.to_string()))?
            .to_vec::<f32>()
            .map_err(|error| Error::Inference(error.to_string()))?;
        let model = model_started.elapsed();
        let postprocessing_started = Instant::now();
        let action_dimensions = self.specification.action_dimensions;
        for (index, value) in values.iter_mut().enumerate() {
            let dimension = index % action_dimensions;
            *value = *value * self.normalization.action_std[dimension]
                + self.normalization.action_mean[dimension];
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(Error::Inference(
                "model produced a non-finite action".to_owned(),
            ));
        }
        let postprocessing = postprocessing_started.elapsed();
        ActionChunk::try_new(
            values,
            self.specification.action_chunk_size,
            action_dimensions,
            InferenceStats {
                preprocessing,
                model,
                postprocessing,
                total: started.elapsed(),
            },
        )
    }
}

struct Normalization {
    state_mean: Vec<f32>,
    state_std: Vec<f32>,
    image_mean: Vec<Vec<f32>>,
    image_std: Vec<Vec<f32>>,
    action_mean: Vec<f32>,
    action_std: Vec<f32>,
}

impl Normalization {
    fn load(config: &ActConfig, weights: &Weights) -> Result<Self> {
        let state_name = config.state_name();
        let action_name = config.action_name();
        let state_prefix = normalization_prefix("normalize_inputs", state_name);
        let action_prefix = normalization_prefix("unnormalize_outputs", action_name);
        let mut image_mean = Vec::with_capacity(config.specification().image_inputs.len());
        let mut image_std = Vec::with_capacity(config.specification().image_inputs.len());
        for image in config.specification().image_inputs {
            let image_prefix = normalization_prefix("normalize_inputs", &image.name);
            image_mean.push(weights.values_exact(&format!("{image_prefix}.mean"), &[3, 1, 1])?);
            image_std.push(weights.values_exact(&format!("{image_prefix}.std"), &[3, 1, 1])?);
        }
        let normalization = Self {
            state_mean: weights
                .vector_exact(&format!("{state_prefix}.mean"), config.state_dimensions())?,
            state_std: weights
                .vector_exact(&format!("{state_prefix}.std"), config.state_dimensions())?,
            image_mean,
            image_std,
            action_mean: weights
                .vector_exact(&format!("{action_prefix}.mean"), config.action_dimensions())?,
            action_std: weights
                .vector_exact(&format!("{action_prefix}.std"), config.action_dimensions())?,
        };
        normalization.validate(config)?;
        Ok(normalization)
    }

    fn validate(&self, config: &ActConfig) -> Result<()> {
        let expected = [
            (
                "state mean",
                self.state_mean.len(),
                config.state_dimensions(),
            ),
            ("state std", self.state_std.len(), config.state_dimensions()),
            (
                "image mean sets",
                self.image_mean.len(),
                config.specification().image_inputs.len(),
            ),
            (
                "image std sets",
                self.image_std.len(),
                config.specification().image_inputs.len(),
            ),
            (
                "action mean",
                self.action_mean.len(),
                config.action_dimensions(),
            ),
            (
                "action std",
                self.action_std.len(),
                config.action_dimensions(),
            ),
        ];
        for (name, actual, expected) in expected {
            if actual != expected {
                return Err(Error::Checkpoint(format!(
                    "{name} has {actual} values, expected {expected}"
                )));
            }
        }
        if self
            .image_mean
            .iter()
            .chain(&self.image_std)
            .any(|values| values.len() != 3)
        {
            return Err(Error::Checkpoint(
                "every image normalization tensor must contain three channels".to_owned(),
            ));
        }
        if self
            .state_mean
            .iter()
            .chain(self.image_mean.iter().flatten())
            .chain(&self.action_mean)
            .any(|value| !value.is_finite())
        {
            return Err(Error::Checkpoint(
                "normalization means must be finite".to_owned(),
            ));
        }
        if self
            .state_std
            .iter()
            .chain(self.image_std.iter().flatten())
            .chain(&self.action_std)
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(Error::Checkpoint(
                "normalization standard deviations must be finite and nonnegative".to_owned(),
            ));
        }
        Ok(())
    }
}

fn normalization_prefix(operation: &str, feature: &str) -> String {
    format!("{operation}.buffer_{}", feature.replace(['.', '/'], "_"))
}

fn normalize(value: f32, mean: f32, standard_deviation: f32) -> f32 {
    (value - mean) / (standard_deviation + NORMALIZATION_EPSILON)
}

fn required_image<'a>(images: &'a [ImageView<'a>], name: &str) -> Result<&'a ImageView<'a>> {
    let mut matches = images.iter().filter(|image| image.name == name);
    let image = matches
        .next()
        .ok_or_else(|| Error::Input(format!("missing required image {name:?}")))?;
    if matches.next().is_some() {
        return Err(Error::Input(format!(
            "image {name:?} was supplied more than once"
        )));
    }
    Ok(image)
}

fn validate_image(image: &ImageView<'_>, width: usize, height: usize) -> Result<()> {
    if image.format != ImageFormat::Rgb8 {
        return Err(Error::Input(format!(
            "image {:?} must use RGB8 pixels",
            image.name
        )));
    }
    if image.width != width || image.height != height {
        return Err(Error::Input(format!(
            "image {:?} is {}x{}, model expects {width}x{height}",
            image.name, image.width, image.height
        )));
    }
    let packed = image
        .width
        .checked_mul(3)
        .ok_or_else(|| Error::Input(format!("image {:?} dimensions overflow", image.name)))?;
    if image.row_stride < packed {
        return Err(Error::Input(format!(
            "image {:?} row stride {} is smaller than {packed}",
            image.name, image.row_stride
        )));
    }
    let final_row = image
        .height
        .checked_sub(1)
        .and_then(|row| row.checked_mul(image.row_stride))
        .ok_or_else(|| Error::Input(format!("image {:?} buffer length overflows", image.name)))?;
    let required = final_row
        .checked_add(packed)
        .ok_or_else(|| Error::Input(format!("image {:?} buffer length overflows", image.name)))?;
    if image.data.len() < required {
        return Err(Error::Input(format!(
            "image {:?} has {} bytes, expected at least {required}",
            image.name,
            image.data.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{ImageFormat, ImageView};

    use super::{normalization_prefix, normalize, validate_image};

    #[test]
    fn normalization_keys_match_lerobot_state_dict() {
        assert_eq!(
            normalization_prefix("normalize_inputs", "observation.images.top"),
            "normalize_inputs.buffer_observation_images_top"
        );
    }

    #[test]
    fn zero_variance_is_stabilized_by_the_reference_epsilon() {
        assert_eq!(normalize(2.0, 2.0, 0.0), 0.0);
        assert!(normalize(2.0 + 1.0e-7, 2.0, 0.0).is_finite());
    }

    #[test]
    fn final_strided_row_does_not_require_trailing_padding() {
        let data = [0_u8; 14];
        let image = ImageView {
            name: "camera",
            width: 2,
            height: 2,
            row_stride: 8,
            format: ImageFormat::Rgb8,
            data: &data,
        };
        assert!(validate_image(&image, 2, 2).is_ok());
    }
}
