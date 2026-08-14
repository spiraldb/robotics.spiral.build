//! Direct LibTorch vision ACT training.
//!
//! The public boundary accepts owned host buffers so Spiral's Arrow data path
//! remains framework-neutral. Tensor upload, autograd, AdamW, validation, and
//! checkpoint export are executed by PyTorch through `tch`.

use std::collections::HashMap;

use safetensors::tensor::{Dtype, TensorView, serialize};
use tch::nn::{Module, OptimizerConfig};
use tch::{Device, Kind, Tensor, nn};

use crate::{Error, Result};

use super::layers::EncoderLayer;
use super::model::ActModel;
use super::weights::Weights;

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];
const GLOBAL_GRADIENT_NORM_LIMIT: f64 = 10.0;

#[derive(Clone, Debug)]
pub struct VisionActTrainingConfig {
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

impl VisionActTrainingConfig {
    fn validate(&self) -> Result<()> {
        if self.state_dimensions == 0
            || self.action_dimensions == 0
            || self.camera_streams.is_empty()
            || self.image_width < 32
            || self.image_height < 32
            || self.chunk_size == 0
            || self.hidden_dimensions == 0
            || self.latent_dimensions == 0
            || self.attention_heads == 0
            || !self.hidden_dimensions.is_multiple_of(self.attention_heads)
            || self.feedforward_dimensions < self.hidden_dimensions
            || self.encoder_layers == 0
            || self.vae_encoder_layers == 0
            || self.decoder_layers == 0
            || self.total_steps == 0
            || !self.kl_weight.is_finite()
            || self.kl_weight < 0.0
            || !self.learning_rate.is_finite()
            || self.learning_rate <= 0.0
        {
            return Err(Error::Configuration(
                "invalid PyTorch vision ACT training dimensions or optimizer settings".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct VisionActBatch {
    pub batch_size: usize,
    pub states: Vec<f32>,
    /// `[batch, camera, 3, height, width]`, already ImageNet-normalized by Spiral.
    pub images: Vec<f32>,
    pub actions: Vec<f32>,
    pub action_is_pad: Vec<bool>,
}

#[derive(Clone, Copy, Debug)]
pub struct VisionActLoss {
    pub total: f64,
    pub l1: f64,
    pub kl: f64,
    pub pad: f64,
}

#[derive(Clone, Debug)]
pub struct VisionActCheckpoint<'a> {
    pub manifest_json: &'a str,
    pub state_mean: &'a [f32],
    pub state_std: &'a [f32],
    pub action_mean: &'a [f32],
    pub action_std: &'a [f32],
}

/// Runtime-selected direct PyTorch trainer. No framework types cross this boundary.
pub struct NativeActTrainer {
    trainer: Trainer,
    backend_name: &'static str,
    parameter_count: usize,
}

impl NativeActTrainer {
    pub fn new(
        config: VisionActTrainingConfig,
        requested_device: &str,
        pretrained_weights: Option<&[u8]>,
    ) -> Result<Self> {
        config.validate()?;
        let requested = requested_device.trim().to_ascii_lowercase();
        let device = match requested.as_str() {
            "auto" if tch::utils::has_mps() => Device::Mps,
            "auto" | "cpu" => Device::Cpu,
            "mps" | "metal" if tch::utils::has_mps() => Device::Mps,
            "mps" | "metal" => {
                return Err(Error::Configuration(
                    "training requested MPS, but this LibTorch build has no available MPS backend"
                        .to_owned(),
                ));
            }
            _ => {
                return Err(Error::Configuration(format!(
                    "training device {requested_device:?} is unsupported; choose auto, cpu, or mps"
                )));
            }
        };
        let trainer = Trainer::new(config, pretrained_weights, device)?;
        let parameter_count = trainer.parameter_count();
        Ok(Self {
            trainer,
            backend_name: if device == Device::Mps { "MPS" } else { "CPU" },
            parameter_count,
        })
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend_name
    }

    pub fn parameter_count(&self) -> usize {
        self.parameter_count
    }

    pub fn learning_rate(&self, step: u64) -> f64 {
        let _ = step;
        self.trainer.config.learning_rate
    }

    pub fn train_step(&mut self, batch: VisionActBatch, step: u64) -> Result<VisionActLoss> {
        self.trainer.train_step(batch, step)
    }

    pub fn validation_step(&self, batch: VisionActBatch) -> Result<VisionActLoss> {
        self.trainer.validation_step(batch)
    }

    pub fn checkpoint(&self, checkpoint: VisionActCheckpoint<'_>) -> Result<Vec<u8>> {
        self.trainer.checkpoint(checkpoint)
    }
}

#[derive(Debug)]
struct FullActModel {
    model: ActModel,
    cvae_encoder: Vec<EncoderLayer>,
    cvae_cls: nn::Embedding,
    cvae_state: nn::Linear,
    cvae_action: nn::Linear,
    latent_stats: nn::Linear,
    cvae_position: Tensor,
    latent_dimensions: i64,
}

impl FullActModel {
    fn new(
        config: &VisionActTrainingConfig,
        variables: &nn::VarStore,
        pretrained: Option<&Weights>,
    ) -> Result<Self> {
        let path = variables.root() / "model";
        let feature_size = resnet18_feature_size(config.image_height, config.image_width);
        let feature_sizes = vec![feature_size; config.camera_streams.len()];
        let model = ActModel::new(
            &path,
            config.state_dimensions,
            config.action_dimensions,
            &feature_sizes,
            config.chunk_size,
            config.hidden_dimensions,
            config.latent_dimensions,
            config.attention_heads,
            config.feedforward_dimensions,
            config.encoder_layers,
            config.decoder_layers,
            0.1,
            false,
        )?;
        let hidden = as_i64("hidden", config.hidden_dimensions)?;
        let heads = as_i64("attention heads", config.attention_heads)?;
        let feedforward = as_i64("feedforward", config.feedforward_dimensions)?;
        let latent = as_i64("latent", config.latent_dimensions)?;
        let state = as_i64("state", config.state_dimensions)?;
        let action = as_i64("action", config.action_dimensions)?;
        let cvae_encoder = (0..config.vae_encoder_layers)
            .map(|layer| {
                EncoderLayer::new(
                    &(&path / "vae_encoder" / "layers" / layer),
                    hidden,
                    heads,
                    feedforward,
                    0.1,
                    false,
                )
            })
            .collect();
        let cvae_cls = nn::embedding(
            &path / "vae_encoder_cls_embed",
            1,
            hidden,
            Default::default(),
        );
        let cvae_state = nn::linear(
            &path / "vae_encoder_robot_state_input_proj",
            state,
            hidden,
            Default::default(),
        );
        let cvae_action = nn::linear(
            &path / "vae_encoder_action_input_proj",
            action,
            hidden,
            Default::default(),
        );
        let latent_stats = nn::linear(
            &path / "vae_encoder_latent_output_proj",
            hidden,
            latent * 2,
            Default::default(),
        );
        let position =
            sinusoidal_position_embedding_1d(config.chunk_size + 2, config.hidden_dimensions)?
                .unsqueeze(0)
                .to_device(variables.device());
        let cvae_position = path.add("vae_encoder_pos_enc", position, false);
        if let Some(pretrained) = pretrained {
            let prefix = if pretrained.contains("model.encoder.layers.0.self_attn.in_proj_weight") {
                "model."
            } else {
                "model.backbone."
            };
            pretrained.copy_prefix_into(variables, prefix)?;
        }
        Ok(Self {
            model,
            cvae_encoder,
            cvae_cls,
            cvae_state,
            cvae_action,
            latent_stats,
            cvae_position,
            latent_dimensions: latent,
        })
    }

    fn forward_training(
        &self,
        state: &Tensor,
        images: &[Tensor],
        actions: &Tensor,
        action_is_pad: &Tensor,
        train: bool,
    ) -> ModelOutput {
        let batch = state.size()[0];
        let cls = self.cvae_cls.ws.unsqueeze(0).repeat([batch, 1, 1]);
        let state_token = self.cvae_state.forward(state).unsqueeze(1);
        let action_tokens = self.cvae_action.forward(actions);
        let mut tokens = Tensor::cat(&[cls, state_token, action_tokens], 1).transpose(0, 1);
        let position = self.cvae_position.transpose(0, 1);
        let prefix_mask = Tensor::zeros([batch, 2], (Kind::Bool, state.device()));
        let padding = Tensor::cat(&[prefix_mask, action_is_pad.shallow_clone()], 1);
        for layer in &self.cvae_encoder {
            tokens = layer.forward(&tokens, &position, Some(&padding), train);
        }
        let encoded = tokens.get(0);
        let stats = self.latent_stats.forward(&encoded);
        let mean = stats.narrow(1, 0, self.latent_dimensions);
        let log_variance = stats.narrow(1, self.latent_dimensions, self.latent_dimensions);
        let noise = Tensor::randn_like(&mean);
        let latent = &mean + (&log_variance * 0.5).exp() * noise;
        let actions = self
            .model
            .forward_with_latent(state, images, &latent, train);
        ModelOutput {
            actions,
            latent_distribution: Some((mean, log_variance)),
        }
    }

    fn forward_inference(&self, state: &Tensor, images: &[Tensor]) -> ModelOutput {
        ModelOutput {
            actions: self.model.forward(state, images),
            latent_distribution: None,
        }
    }
}

struct ModelOutput {
    actions: Tensor,
    latent_distribution: Option<(Tensor, Tensor)>,
}

struct LossOutput {
    total: Tensor,
    l1: Tensor,
    kl: Tensor,
}

struct DeviceBatch {
    states: Tensor,
    images: Vec<Tensor>,
    actions: Tensor,
    action_is_pad: Tensor,
}

struct Trainer {
    config: VisionActTrainingConfig,
    model: FullActModel,
    optimizer: nn::Optimizer,
    variables: nn::VarStore,
    device: Device,
}

impl Trainer {
    fn new(
        config: VisionActTrainingConfig,
        pretrained_weights: Option<&[u8]>,
        device: Device,
    ) -> Result<Self> {
        let seed = i64::try_from(config.seed)
            .map_err(|_| Error::Configuration("ACT seed exceeds i64".to_owned()))?;
        tch::manual_seed(seed);
        let variables = nn::VarStore::new(device);
        let pretrained = pretrained_weights
            .map(|bytes| Weights::from_bytes("pretrained ACT weights", bytes.to_vec()))
            .transpose()?;
        let full_checkpoint = pretrained.as_ref().is_some_and(|weights| {
            weights.contains("model.encoder.layers.0.self_attn.in_proj_weight")
        });
        let model = FullActModel::new(&config, &variables, pretrained.as_ref())?;
        // A complete checkpoint fixes initialization. Resetting here gives
        // callers a reproducible PyTorch RNG stream for dropout and VAE noise,
        // independent of the temporary module initialization used before load.
        if full_checkpoint {
            tch::manual_seed(seed);
        }
        let optimizer = nn::AdamW::default()
            .wd(1.0e-4)
            .build(&variables, config.learning_rate)
            .map_err(|error| Error::Inference(format!("creating PyTorch AdamW: {error}")))?;
        Ok(Self {
            config,
            model,
            optimizer,
            variables,
            device,
        })
    }

    fn parameter_count(&self) -> usize {
        self.variables
            .trainable_variables()
            .iter()
            .map(Tensor::numel)
            .sum()
    }

    fn batch(&self, batch: VisionActBatch) -> Result<DeviceBatch> {
        validate_batch(&self.config, &batch)?;
        let VisionActBatch {
            batch_size,
            states,
            images,
            actions,
            action_is_pad,
        } = batch;
        let batch_size = as_i64("batch", batch_size)?;
        let state_dimensions = as_i64("state", self.config.state_dimensions)?;
        let cameras = as_i64("camera", self.config.camera_streams.len())?;
        let height = as_i64("image height", self.config.image_height)?;
        let width = as_i64("image width", self.config.image_width)?;
        let chunk = as_i64("chunk", self.config.chunk_size)?;
        let action_dimensions = as_i64("action", self.config.action_dimensions)?;
        let states = Tensor::from_slice(&states)
            .view([batch_size, state_dimensions])
            .to_device(self.device);
        let images = Tensor::from_slice(&images)
            .view([batch_size, cameras, 3, height, width])
            .to_device(self.device)
            .unbind(1);
        let actions = Tensor::from_slice(&actions)
            .view([batch_size, chunk, action_dimensions])
            .to_device(self.device);
        let action_is_pad = Tensor::from_slice(&action_is_pad)
            .view([batch_size, chunk])
            .to_device(self.device);
        Ok(DeviceBatch {
            states,
            images,
            actions,
            action_is_pad,
        })
    }

    fn train_step(&mut self, batch: VisionActBatch, step: u64) -> Result<VisionActLoss> {
        let batch = self.batch(batch)?;
        let output = self.model.forward_training(
            &batch.states,
            &batch.images,
            &batch.actions,
            &batch.action_is_pad,
            true,
        );
        let loss = act_loss(
            output,
            &batch.actions,
            &batch.action_is_pad,
            self.config.kl_weight,
        );
        let metrics = Tensor::stack(
            &[
                loss.total.shallow_clone(),
                loss.l1.shallow_clone(),
                loss.kl.shallow_clone(),
            ],
            0,
        );
        self.optimizer
            .backward_step_clip_norm(&loss.total, GLOBAL_GRADIENT_NORM_LIMIT);
        let metrics = read_loss(metrics)?;
        if !metrics.total.is_finite() {
            return Err(Error::Inference(format!(
                "ACT loss became non-finite at step {step}"
            )));
        }
        Ok(metrics)
    }

    fn validation_step(&self, batch: VisionActBatch) -> Result<VisionActLoss> {
        let batch = self.batch(batch)?;
        tch::no_grad(|| {
            let output = self.model.forward_inference(&batch.states, &batch.images);
            let loss = act_loss(
                output,
                &batch.actions,
                &batch.action_is_pad,
                self.config.kl_weight,
            );
            read_loss(Tensor::stack(&[loss.total, loss.l1, loss.kl], 0))
        })
    }

    fn checkpoint(&self, checkpoint: VisionActCheckpoint<'_>) -> Result<Vec<u8>> {
        validate_checkpoint(&self.config, &checkpoint)?;
        let mut tensors = self
            .variables
            .variables()
            .into_iter()
            .filter(|(name, _)| name.starts_with("model."))
            .map(|(name, tensor)| owned_tensor(name, tensor))
            .collect::<Result<Vec<_>>>()?;
        add_normalization_tensors(&self.config, &checkpoint, &mut tensors);
        tensors.sort_by(|left, right| left.name.cmp(&right.name));
        let views = tensors
            .iter()
            .map(|tensor| {
                TensorView::new(tensor.dtype, tensor.shape.clone(), &tensor.bytes)
                    .map(|view| (tensor.name.as_str(), view))
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| Error::Checkpoint(format!("building ACT checkpoint: {error}")))?;
        let metadata = HashMap::from([
            ("format".to_owned(), "spiral-act".to_owned()),
            (
                "spiral_robotics".to_owned(),
                checkpoint.manifest_json.to_owned(),
            ),
        ]);
        serialize(views, Some(metadata))
            .map_err(|error| Error::Checkpoint(format!("serializing ACT checkpoint: {error}")))
    }
}

fn validate_batch(config: &VisionActTrainingConfig, batch: &VisionActBatch) -> Result<()> {
    let expected = [
        (
            "states",
            batch.states.len(),
            batch.batch_size * config.state_dimensions,
        ),
        (
            "images",
            batch.images.len(),
            batch.batch_size
                * config.camera_streams.len()
                * 3
                * config.image_height
                * config.image_width,
        ),
        (
            "actions",
            batch.actions.len(),
            batch.batch_size * config.chunk_size * config.action_dimensions,
        ),
        (
            "action padding",
            batch.action_is_pad.len(),
            batch.batch_size * config.chunk_size,
        ),
    ];
    if batch.batch_size == 0 {
        return Err(Error::Input("ACT batch cannot be empty".to_owned()));
    }
    for (name, actual, expected) in expected {
        if actual != expected {
            return Err(Error::Input(format!(
                "ACT {name} buffer has {actual} values, expected {expected}"
            )));
        }
    }
    if batch
        .states
        .iter()
        .chain(&batch.images)
        .chain(&batch.actions)
        .any(|value| !value.is_finite())
    {
        return Err(Error::Input(
            "ACT batch contains a non-finite numeric value".to_owned(),
        ));
    }
    Ok(())
}

fn act_loss(
    output: ModelOutput,
    target: &Tensor,
    target_pad: &Tensor,
    kl_weight: f64,
) -> LossOutput {
    let keep = target_pad.logical_not().to_kind(Kind::Float).unsqueeze(2);
    let action_values = (keep.sum(Kind::Float) * target.size()[2]).clamp_min(1.0);
    let l1 = ((output.actions - target).abs() * keep).sum(Kind::Float) / action_values;
    let kl = match output.latent_distribution {
        Some((mean, log_variance)) => {
            let kl = (&log_variance + 1.0 - mean.square() - log_variance.exp()) * -0.5;
            kl.sum_dim_intlist([1].as_slice(), false, Kind::Float)
                .mean(Kind::Float)
        }
        None => Tensor::zeros([], (Kind::Float, target.device())),
    };
    let total = &l1 + &kl * kl_weight;
    LossOutput { total, l1, kl }
}

fn read_loss(metrics: Tensor) -> Result<VisionActLoss> {
    let metrics = metrics
        .to_device(Device::Cpu)
        .to_kind(Kind::Float)
        .view([-1]);
    let values = Vec::<f32>::try_from(&metrics)
        .map_err(|error| Error::Inference(format!("reading PyTorch loss metrics: {error}")))?;
    let [total, l1, kl] = values.as_slice() else {
        return Err(Error::Inference(format!(
            "PyTorch returned {} loss metrics, expected 3",
            values.len()
        )));
    };
    Ok(VisionActLoss {
        total: f64::from(*total),
        l1: f64::from(*l1),
        kl: f64::from(*kl),
        pad: 0.0,
    })
}

fn sinusoidal_position_embedding_1d(positions: usize, dimensions: usize) -> Result<Tensor> {
    let mut values = vec![0.0; positions * dimensions];
    for position in 0..positions {
        for dimension in 0..dimensions {
            let divisor = 10_000.0_f32.powf(2.0 * ((dimension / 2) as f32) / dimensions as f32);
            let angle = position as f32 / divisor;
            values[position * dimensions + dimension] = if dimension % 2 == 0 {
                angle.sin()
            } else {
                angle.cos()
            };
        }
    }
    Ok(Tensor::from_slice(&values).view([
        as_i64("position count", positions)?,
        as_i64("position width", dimensions)?,
    ]))
}

fn resnet18_feature_size(height: usize, width: usize) -> (usize, usize) {
    fn conv(value: usize, kernel: usize, stride: usize, padding: usize) -> usize {
        (value + 2 * padding - kernel) / stride + 1
    }
    let mut height = conv(height, 7, 2, 3);
    let mut width = conv(width, 7, 2, 3);
    height = conv(height, 3, 2, 1);
    width = conv(width, 3, 2, 1);
    for _ in 0..3 {
        height = conv(height, 3, 2, 1);
        width = conv(width, 3, 2, 1);
    }
    (height, width)
}

fn validate_checkpoint(
    config: &VisionActTrainingConfig,
    checkpoint: &VisionActCheckpoint<'_>,
) -> Result<()> {
    if checkpoint.state_mean.len() != config.state_dimensions
        || checkpoint.state_std.len() != config.state_dimensions
        || checkpoint.action_mean.len() != config.action_dimensions
        || checkpoint.action_std.len() != config.action_dimensions
    {
        return Err(Error::Checkpoint(
            "normalization widths do not match the ACT model".to_owned(),
        ));
    }
    if checkpoint
        .state_mean
        .iter()
        .chain(checkpoint.state_std)
        .chain(checkpoint.action_mean)
        .chain(checkpoint.action_std)
        .any(|value| !value.is_finite())
    {
        return Err(Error::Checkpoint(
            "normalization contains a non-finite value".to_owned(),
        ));
    }
    Ok(())
}

struct OwnedTensor {
    name: String,
    dtype: Dtype,
    shape: Vec<usize>,
    bytes: Vec<u8>,
}

fn owned_tensor(name: String, tensor: Tensor) -> Result<OwnedTensor> {
    if tensor.kind() != Kind::Float {
        return Err(Error::Checkpoint(format!(
            "ACT tensor {name:?} must be float32, found {:?}",
            tensor.kind()
        )));
    }
    let shape = tensor
        .size()
        .into_iter()
        .map(usize::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Error::Checkpoint(format!("ACT tensor {name:?} has an invalid shape")))?;
    let tensor = tensor.to_device(Device::Cpu).contiguous().view([-1]);
    let values = Vec::<f32>::try_from(&tensor)
        .map_err(|error| Error::Checkpoint(format!("reading ACT tensor {name:?}: {error}")))?;
    Ok(OwnedTensor {
        name,
        dtype: Dtype::F32,
        shape,
        bytes: values.into_iter().flat_map(f32::to_le_bytes).collect(),
    })
}

fn add_normalization_tensors(
    config: &VisionActTrainingConfig,
    checkpoint: &VisionActCheckpoint<'_>,
    tensors: &mut Vec<OwnedTensor>,
) {
    push_f32(
        tensors,
        "normalize_inputs.buffer_observation_state.mean",
        vec![config.state_dimensions],
        checkpoint.state_mean,
    );
    push_f32(
        tensors,
        "normalize_inputs.buffer_observation_state.std",
        vec![config.state_dimensions],
        checkpoint.state_std,
    );
    for camera in &config.camera_streams {
        let key = camera.replace(['.', '/'], "_");
        push_f32(
            tensors,
            &format!("normalize_inputs.buffer_{key}.mean"),
            vec![3, 1, 1],
            &IMAGENET_MEAN,
        );
        push_f32(
            tensors,
            &format!("normalize_inputs.buffer_{key}.std"),
            vec![3, 1, 1],
            &IMAGENET_STD,
        );
    }
    push_f32(
        tensors,
        "unnormalize_outputs.buffer_action.mean",
        vec![config.action_dimensions],
        checkpoint.action_mean,
    );
    push_f32(
        tensors,
        "unnormalize_outputs.buffer_action.std",
        vec![config.action_dimensions],
        checkpoint.action_std,
    );
}

fn push_f32(owned: &mut Vec<OwnedTensor>, name: &str, shape: Vec<usize>, values: &[f32]) {
    owned.push(OwnedTensor {
        name: name.to_owned(),
        dtype: Dtype::F32,
        shape,
        bytes: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    });
}

fn as_i64(name: &str, value: usize) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| Error::Configuration(format!("ACT {name} dimension exceeds i64")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_config() -> VisionActTrainingConfig {
        VisionActTrainingConfig {
            state_dimensions: 2,
            action_dimensions: 2,
            camera_streams: vec!["wrist".to_owned()],
            image_width: 32,
            image_height: 32,
            chunk_size: 2,
            hidden_dimensions: 16,
            latent_dimensions: 4,
            attention_heads: 4,
            feedforward_dimensions: 32,
            encoder_layers: 1,
            vae_encoder_layers: 1,
            decoder_layers: 1,
            kl_weight: 1.0,
            learning_rate: 1.0e-5,
            total_steps: 100,
            seed: 42,
        }
    }

    fn small_batch() -> VisionActBatch {
        VisionActBatch {
            batch_size: 1,
            states: vec![0.0; 2],
            images: vec![0.0; 3 * 32 * 32],
            actions: vec![0.0; 2 * 2],
            action_is_pad: vec![false; 2],
        }
    }

    #[test]
    fn resnet_shape_matches_the_runtime_feature_contract() {
        assert_eq!(resnet18_feature_size(224, 224), (7, 7));
        assert_eq!(resnet18_feature_size(64, 64), (2, 2));
    }

    #[test]
    fn act_learning_rate_has_no_scheduler() {
        let trainer = NativeActTrainer::new(small_config(), "cpu", None).unwrap();
        assert_eq!(trainer.learning_rate(0), 1.0e-5);
        assert_eq!(trainer.learning_rate(50), 1.0e-5);
        assert_eq!(trainer.learning_rate(100), 1.0e-5);
    }

    #[test]
    fn every_training_step_returns_its_own_metrics() {
        let mut config = small_config();
        config.total_steps = 2;
        let mut trainer = NativeActTrainer::new(config, "cpu", None).unwrap();
        let first = trainer.train_step(small_batch(), 1).unwrap();
        let second = trainer.train_step(small_batch(), 2).unwrap();
        assert!(first.total.is_finite());
        assert!(second.total.is_finite());
    }

    #[test]
    fn gradients_are_clipped_by_one_global_model_norm() {
        let variables = nn::VarStore::new(Device::Cpu);
        let model = nn::linear(variables.root(), 2, 1, Default::default());
        let optimizer = nn::AdamW::default().build(&variables, 1.0e-5).unwrap();
        let input = Tensor::from_slice(&[100.0_f32, 100.0]).view([1, 2]);
        let loss = model.forward(&input).sum(Kind::Float);
        loss.backward();
        let before = global_gradient_norm(&variables);
        assert!(before > 100.0);
        optimizer.clip_grad_norm(10.0);
        let after = global_gradient_norm(&variables);
        assert!((after - 10.0).abs() < 1.0e-4, "global norm was {after}");
    }

    fn global_gradient_norm(variables: &nn::VarStore) -> f64 {
        let norms = variables
            .trainable_variables()
            .into_iter()
            .filter_map(|tensor| tensor.grad().defined().then(|| tensor.grad().norm()))
            .collect::<Vec<_>>();
        f64::try_from(Tensor::stack(&norms, 0).norm()).unwrap()
    }

    #[test]
    fn vae_positions_match_the_fixed_lerobot_sinusoid() {
        let positions =
            Vec::<f32>::try_from(&sinusoidal_position_embedding_1d(3, 8).unwrap().view([-1]))
                .unwrap();
        assert_eq!(&positions[..8], &[0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
        assert!((positions[8] - 1.0_f32.sin()).abs() < 1.0e-6);
        assert!((positions[9] - 1.0_f32.cos()).abs() < 1.0e-6);
        assert!((positions[10] - 0.1_f32.sin()).abs() < 1.0e-6);
        assert!((positions[11] - 0.1_f32.cos()).abs() < 1.0e-6);
    }

    #[test]
    fn vae_depth_is_independent_from_the_main_encoder_depth() {
        let mut config = small_config();
        config.vae_encoder_layers = 3;
        let variables = nn::VarStore::new(Device::Cpu);
        let model = FullActModel::new(&config, &variables, None).unwrap();
        assert_eq!(model.model.encoder_layer_count(), 1);
        assert_eq!(model.cvae_encoder.len(), 3);
    }

    #[test]
    fn torch_checkpoint_loads_through_the_native_inference_boundary() {
        use crate::{ModelArtifact, Policy};

        let camera = "wrist".to_owned();
        let mut config = small_config();
        config.state_dimensions = 6;
        config.action_dimensions = 6;
        config.camera_streams = vec![camera.clone()];
        config.chunk_size = 4;
        config.hidden_dimensions = 64;
        config.latent_dimensions = 8;
        config.attention_heads = 8;
        config.feedforward_dimensions = 128;
        config.vae_encoder_layers = 2;
        config.total_steps = 1;
        let mut trainer = NativeActTrainer::new(config, "cpu", None).unwrap();
        let loss = trainer
            .train_step(
                VisionActBatch {
                    batch_size: 1,
                    states: vec![0.0; 6],
                    images: vec![0.0; 3 * 32 * 32],
                    actions: vec![0.0; 4 * 6],
                    action_is_pad: vec![false; 4],
                },
                1,
            )
            .unwrap();
        assert!(loss.total.is_finite());
        let zeros = [0.0; 6];
        let ones = [1.0; 6];
        let weights = trainer
            .checkpoint(VisionActCheckpoint {
                manifest_json: "{}",
                state_mean: &zeros,
                state_std: &ones,
                action_mean: &zeros,
                action_std: &ones,
            })
            .unwrap();
        let tensors = safetensors::SafeTensors::deserialize(&weights).unwrap();
        assert!(
            tensors
                .tensor("model.encoder.layers.0.self_attn.in_proj_weight")
                .is_ok()
        );
        assert!(
            tensors
                .tensor("model.vae_encoder.layers.0.self_attn.in_proj_weight")
                .is_ok()
        );
        let model_config = serde_json::json!({
            "type": "act",
            "n_obs_steps": 1,
            "normalization_mapping": {
                "VISUAL": "MEAN_STD", "STATE": "MEAN_STD", "ACTION": "MEAN_STD"
            },
            "input_features": {
                "observation.state": {"type": "STATE", "shape": [6]},
                camera: {"type": "VISUAL", "shape": [3, 32, 32]}
            },
            "output_features": {"action": {"type": "ACTION", "shape": [6]}},
            "chunk_size": 4,
            "n_action_steps": 4,
            "vision_backbone": "resnet18",
            "replace_final_stride_with_dilation": false,
            "pre_norm": false,
            "dim_model": 64,
            "n_heads": 8,
            "dim_feedforward": 128,
            "feedforward_activation": "relu",
            "n_encoder_layers": 1,
            "n_decoder_layers": 1,
            "use_vae": true,
            "latent_dim": 8,
            "spiral_camera_embedding": false,
            "spiral_camera_order": ["wrist"]
        });
        let artifact =
            ModelArtifact::from_bytes(serde_json::to_vec(&model_config).unwrap(), weights).unwrap();
        let policy = super::super::engine::load_cpu_artifact(artifact).unwrap();
        assert_eq!(policy.specification().action_chunk_size, 4);
        assert_eq!(policy.specification().image_inputs[0].name, "wrist");
    }

    #[test]
    #[ignore = "requires an independently generated PyTorch one-step reference"]
    fn one_step_matches_pytorch_from_identical_weights_and_rng() {
        let root = std::env::var_os("SPIRAL_ACT_TRAINING_REFERENCE_DIR")
            .expect("SPIRAL_ACT_TRAINING_REFERENCE_DIR must name the parity fixture");
        let root = std::path::PathBuf::from(root);
        let initial = std::fs::read(root.join("initial.safetensors")).unwrap();
        let expected_metrics: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("expected.json")).unwrap()).unwrap();
        let expected_checkpoint = std::fs::read(root.join("updated.safetensors")).unwrap();
        let config = VisionActTrainingConfig {
            state_dimensions: 2,
            action_dimensions: 2,
            camera_streams: vec!["wrist".to_owned()],
            image_width: 32,
            image_height: 32,
            chunk_size: 2,
            hidden_dimensions: 64,
            latent_dimensions: 4,
            attention_heads: 4,
            feedforward_dimensions: 128,
            encoder_layers: 1,
            vae_encoder_layers: 1,
            decoder_layers: 1,
            kl_weight: 1.0,
            learning_rate: 1.0e-5,
            total_steps: 1,
            seed: 777,
        };
        let mut trainer = NativeActTrainer::new(config, "cpu", Some(&initial)).unwrap();
        let metrics = trainer
            .train_step(
                VisionActBatch {
                    batch_size: 1,
                    states: vec![0.0; 2],
                    images: vec![0.0; 3 * 32 * 32],
                    actions: vec![0.25, -0.5, 0.75, -1.0],
                    action_is_pad: vec![false; 2],
                },
                1,
            )
            .unwrap();
        for (name, actual) in [
            ("total", metrics.total),
            ("l1", metrics.l1),
            ("kl", metrics.kl),
        ] {
            let expected = expected_metrics[name].as_f64().unwrap();
            assert!(
                (actual - expected).abs() <= 1.0e-5,
                "{name} mismatch: expected {expected}, got {actual}"
            );
        }

        let zeros = [0.0; 2];
        let ones = [1.0; 2];
        let actual_checkpoint = trainer
            .checkpoint(VisionActCheckpoint {
                manifest_json: "{}",
                state_mean: &zeros,
                state_std: &ones,
                action_mean: &zeros,
                action_std: &ones,
            })
            .unwrap();
        let expected = safetensors::SafeTensors::deserialize(&expected_checkpoint).unwrap();
        let actual = safetensors::SafeTensors::deserialize(&actual_checkpoint).unwrap();
        let mut maximum_error = 0.0_f32;
        for name in expected.names() {
            let left = expected.tensor(name).unwrap();
            let right = actual.tensor(name).unwrap();
            assert_eq!(left.shape(), right.shape(), "shape mismatch for {name}");
            for (left, right) in left
                .data()
                .chunks_exact(4)
                .zip(right.data().chunks_exact(4))
            {
                let left = f32::from_le_bytes(left.try_into().unwrap());
                let right = f32::from_le_bytes(right.try_into().unwrap());
                maximum_error = maximum_error.max((left - right).abs());
            }
        }
        assert!(
            maximum_error <= 2.0e-6,
            "one-step checkpoint maximum error was {maximum_error}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mps_repeats_training_sync_segments_and_checkpoints() {
        if !tch::utils::has_mps() {
            return;
        }
        let mut config = small_config();
        config.total_steps = 12;
        let mut trainer = NativeActTrainer::new(config, "mps", None).unwrap();
        for step in 1..=12 {
            assert!(
                trainer
                    .train_step(small_batch(), step)
                    .unwrap()
                    .total
                    .is_finite()
            );
        }
        assert!(
            trainer
                .validation_step(small_batch())
                .unwrap()
                .total
                .is_finite()
        );
        let zeros = [0.0; 2];
        let ones = [1.0; 2];
        let checkpoint = trainer
            .checkpoint(VisionActCheckpoint {
                manifest_json: "{}",
                state_mean: &zeros,
                state_std: &ones,
                action_mean: &zeros,
                action_std: &ones,
            })
            .unwrap();
        assert!(!checkpoint.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "full 51.7M-parameter ACT MPS throughput benchmark"]
    fn benchmark_full_act_mps_training() {
        use std::time::Instant;

        assert!(tch::utils::has_mps(), "PyTorch MPS must be available");
        const BATCH: usize = 8;
        const WARMUP: u64 = 4;
        const MEASURED: u64 = 12;
        let config = VisionActTrainingConfig {
            state_dimensions: 6,
            action_dimensions: 6,
            camera_streams: vec!["gripper".to_owned(), "top".to_owned()],
            image_width: 640,
            image_height: 360,
            chunk_size: 100,
            hidden_dimensions: 512,
            latent_dimensions: 32,
            attention_heads: 8,
            feedforward_dimensions: 3_200,
            encoder_layers: 4,
            vae_encoder_layers: 4,
            decoder_layers: 1,
            kl_weight: 10.0,
            learning_rate: 1.0e-5,
            total_steps: WARMUP + MEASURED,
            seed: 1_000,
        };
        let batch = VisionActBatch {
            batch_size: BATCH,
            states: vec![0.0; BATCH * 6],
            images: vec![0.0; BATCH * 2 * 3 * 360 * 640],
            actions: vec![0.0; BATCH * 100 * 6],
            action_is_pad: vec![false; BATCH * 100],
        };
        let mut trainer = NativeActTrainer::new(config, "mps", None).unwrap();
        for step in 1..=WARMUP {
            trainer.train_step(batch.clone(), step).unwrap();
        }
        let started = Instant::now();
        let mut last_loss = 0.0;
        for step in WARMUP + 1..=WARMUP + MEASURED {
            last_loss = trainer.train_step(batch.clone(), step).unwrap().total;
        }
        let seconds = started.elapsed().as_secs_f64();
        let samples_per_second = BATCH as f64 * MEASURED as f64 / seconds;
        eprintln!(
            "full ACT direct tch MPS: {samples_per_second:.3} samples/s ({seconds:.3}s for {MEASURED} steps, loss={last_loss:.6}, parameters={})",
            trainer.parameter_count()
        );
        assert!(samples_per_second.is_finite() && samples_per_second > 0.0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "phase profiler for the full ACT MPS training step"]
    fn profile_full_act_mps_training_step() {
        use std::time::Instant;

        fn synchronize(tensor: &Tensor) {
            let _ = tensor
                .flatten(0, -1)
                .get(0)
                .to_device(Device::Cpu)
                .double_value(&[]);
        }

        assert!(tch::utils::has_mps(), "PyTorch MPS must be available");
        const BATCH: usize = 8;
        let config = VisionActTrainingConfig {
            state_dimensions: 6,
            action_dimensions: 6,
            camera_streams: vec!["gripper".to_owned(), "top".to_owned()],
            image_width: 640,
            image_height: 360,
            chunk_size: 100,
            hidden_dimensions: 512,
            latent_dimensions: 32,
            attention_heads: 8,
            feedforward_dimensions: 3_200,
            encoder_layers: 4,
            vae_encoder_layers: 4,
            decoder_layers: 1,
            kl_weight: 10.0,
            learning_rate: 1.0e-5,
            total_steps: 8,
            seed: 1_000,
        };
        let batch = VisionActBatch {
            batch_size: BATCH,
            states: vec![0.0; BATCH * 6],
            images: vec![0.0; BATCH * 2 * 3 * 360 * 640],
            actions: vec![0.0; BATCH * 100 * 6],
            action_is_pad: vec![false; BATCH * 100],
        };
        let mut trainer = Trainer::new(config, None, Device::Mps).unwrap();
        for step in 1..=8 {
            let started = Instant::now();
            let device_batch = trainer.batch(batch.clone()).unwrap();
            synchronize(&device_batch.images[0]);
            let uploaded = Instant::now();
            let output = trainer.model.forward_training(
                &device_batch.states,
                &device_batch.images,
                &device_batch.actions,
                &device_batch.action_is_pad,
                true,
            );
            let loss = act_loss(
                output,
                &device_batch.actions,
                &device_batch.action_is_pad,
                trainer.config.kl_weight,
            );
            synchronize(&loss.total);
            let forwarded = Instant::now();
            trainer.optimizer.zero_grad();
            loss.total.backward();
            let first_parameter = trainer.variables.trainable_variables().remove(0);
            synchronize(&first_parameter.grad());
            let backward = Instant::now();
            trainer.optimizer.clip_grad_norm(GLOBAL_GRADIENT_NORM_LIMIT);
            let clipped = Instant::now();
            trainer.optimizer.step();
            synchronize(&first_parameter);
            let optimized = Instant::now();
            eprintln!(
                "step {step}: upload={:.6} forward={:.6} backward={:.6} clip={:.6} optimizer={:.6} total={:.6}",
                uploaded.duration_since(started).as_secs_f64(),
                forwarded.duration_since(uploaded).as_secs_f64(),
                backward.duration_since(forwarded).as_secs_f64(),
                clipped.duration_since(backward).as_secs_f64(),
                optimized.duration_since(clipped).as_secs_f64(),
                optimized.duration_since(started).as_secs_f64(),
            );
        }
    }
}
