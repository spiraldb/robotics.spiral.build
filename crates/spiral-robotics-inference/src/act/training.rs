//! Burn-native vision ACT training.
//!
//! The public boundary deliberately accepts owned host buffers. Spiral's ADBC
//! Arrow stream can therefore remain independent of Burn, while tensor upload,
//! autodiff, optimization, validation, and checkpoint export stay in Rust.

use std::collections::HashMap;
use std::rc::Rc;

use burn_autodiff::Autodiff;
use burn_core as burn;
use burn_core::module::{AutodiffModule, Initializer, Module, Param};
use burn_core::tensor::backend::{AutodiffBackend, Backend};
use burn_core::tensor::{Bool, Distribution, Tensor, TensorData};
use burn_nn::{Linear, LinearConfig};
use burn_optim::adaptor::OptimizerAdaptor;
use burn_optim::grad_clipping::GradientClippingConfig;
use burn_optim::{AdamW, AdamWConfig, GradientsParams, Optimizer};
use burn_store::{ModuleAdapter, ModuleSnapshot, SafetensorsStore, TensorSnapshot};
use safetensors::tensor::{Dtype, TensorView, serialize};

use crate::{Error, Result};

use super::layers::EncoderLayer;
use super::model::ActModel;
use super::weights::Weights;

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Store linear and normalization parameters in the stable layout consumed by
/// the native inference loader. Framework-specific module storage never leaks
/// into the durable checkpoint contract.
#[derive(Clone, Debug)]
struct InferenceLayoutAdapter;

impl ModuleAdapter for InferenceLayoutAdapter {
    fn adapt(&self, snapshot: &TensorSnapshot) -> TensorSnapshot {
        let Some(module) = snapshot.module_type() else {
            return snapshot.clone();
        };
        let Some(path) = snapshot.path_stack.as_ref() else {
            return snapshot.clone();
        };
        let Some(parameter) = path.last() else {
            return snapshot.clone();
        };
        if module == "Struct:Linear" && parameter == "weight" && snapshot.shape.len() == 2 {
            return transpose_snapshot(snapshot);
        }
        if module == "Struct:LayerNorm" {
            let external_name = match parameter.as_str() {
                "gamma" => Some("weight"),
                "beta" => Some("bias"),
                _ => None,
            };
            if let Some(external_name) = external_name {
                let mut path = path.clone();
                *path.last_mut().expect("parameter path") = external_name.to_owned();
                return TensorSnapshot::from_closure(
                    snapshot.clone_data_fn(),
                    snapshot.dtype,
                    snapshot.shape.clone(),
                    path,
                    snapshot.container_stack.clone().unwrap_or_default(),
                    snapshot.tensor_id.unwrap_or_default(),
                );
            }
        }
        snapshot.clone()
    }

    fn clone_box(&self) -> Box<dyn ModuleAdapter> {
        Box::new(self.clone())
    }
}

fn transpose_snapshot(snapshot: &TensorSnapshot) -> TensorSnapshot {
    let rows = snapshot.shape[0];
    let columns = snapshot.shape[1];
    let dtype = snapshot.dtype;
    let source = snapshot.clone_data_fn();
    let data = Rc::new(move || {
        let data = source()?;
        let element_size = dtype.size();
        let mut bytes = vec![0; data.as_bytes().len()];
        for row in 0..rows {
            for column in 0..columns {
                let source = (row * columns + column) * element_size;
                let destination = (column * rows + row) * element_size;
                bytes[destination..destination + element_size]
                    .copy_from_slice(&data.as_bytes()[source..source + element_size]);
            }
        }
        Ok(TensorData::from_bytes_vec(
            bytes,
            vec![columns, rows],
            dtype,
        ))
    });
    TensorSnapshot::from_closure(
        data,
        dtype,
        vec![columns, rows].into(),
        snapshot.path_stack.clone().unwrap_or_default(),
        snapshot.container_stack.clone().unwrap_or_default(),
        snapshot.tensor_id.unwrap_or_default(),
    )
}

/// Validated dimensions and optimizer settings for one ACT run.
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
                "invalid Burn vision ACT training dimensions or optimizer settings".to_owned(),
            ));
        }
        Ok(())
    }
}

/// One Arrow-materialized ACT batch in row-major host memory.
#[derive(Clone, Debug)]
pub struct VisionActBatch {
    pub batch_size: usize,
    pub states: Vec<f32>,
    /// `[batch, camera, 3, height, width]`, already ImageNet-normalized by Spiral.
    pub images: Vec<f32>,
    pub actions: Vec<f32>,
    pub action_is_pad: Vec<bool>,
}

/// Scalar diagnostics returned after one optimizer or validation step.
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

/// Runtime-selected native trainer. No framework types cross this boundary.
pub struct NativeActTrainer {
    engine: Box<dyn TrainerEngine>,
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
        #[cfg(all(target_os = "macos", feature = "metal"))]
        if matches!(requested.as_str(), "auto" | "mps" | "metal")
            && super::engine::metal_available()
        {
            type B = Autodiff<burn_wgpu::Metal>;
            let trainer = Trainer::<B>::new(config, pretrained_weights, Default::default())?;
            let parameter_count = trainer.parameter_count();
            return Ok(Self {
                engine: Box::new(trainer),
                backend_name: "Metal",
                parameter_count,
            });
        }
        if !matches!(requested.as_str(), "auto" | "cpu") {
            return Err(Error::Configuration(format!(
                "training device {requested_device:?} is unavailable; this build supports CPU{}",
                if cfg!(target_os = "macos") {
                    " and Metal"
                } else {
                    ""
                }
            )));
        }
        type B = Autodiff<burn_ndarray::NdArray<f32>>;
        let trainer = Trainer::<B>::new(config, pretrained_weights, Default::default())?;
        let parameter_count = trainer.parameter_count();
        Ok(Self {
            engine: Box::new(trainer),
            backend_name: "CPU",
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
        self.engine.learning_rate(step)
    }

    pub fn train_step(&mut self, batch: VisionActBatch, step: u64) -> Result<VisionActLoss> {
        self.engine.train_step(batch, step)
    }

    pub fn validation_step(&self, batch: VisionActBatch) -> Result<VisionActLoss> {
        self.engine.validation_step(batch)
    }

    pub fn checkpoint(&self, checkpoint: VisionActCheckpoint<'_>) -> Result<Vec<u8>> {
        self.engine.checkpoint(checkpoint)
    }
}

trait TrainerEngine: Send {
    fn parameter_count(&self) -> usize;
    fn learning_rate(&self, step: u64) -> f64;
    fn train_step(&mut self, batch: VisionActBatch, step: u64) -> Result<VisionActLoss>;
    fn validation_step(&self, batch: VisionActBatch) -> Result<VisionActLoss>;
    fn checkpoint(&self, checkpoint: VisionActCheckpoint<'_>) -> Result<Vec<u8>>;
}

#[derive(Module, Debug)]
struct FullActModel<B: Backend> {
    model: ActModel<B>,
    cvae_encoder: Vec<EncoderLayer<B>>,
    cvae_cls: Param<Tensor<B, 3>>,
    cvae_state: Linear<B>,
    cvae_action: Linear<B>,
    latent_stats: Linear<B>,
    chunk_size: usize,
    hidden_dimensions: usize,
    latent_dimensions: usize,
}

impl<B: Backend> FullActModel<B> {
    fn init(
        config: &VisionActTrainingConfig,
        pretrained: Option<&Weights>,
        device: &B::Device,
    ) -> Result<Self> {
        let feature_size = resnet18_feature_size(config.image_height, config.image_width);
        let feature_sizes = vec![feature_size; config.camera_streams.len()];
        let model = ActModel::init(
            config.state_dimensions,
            config.action_dimensions,
            feature_sizes,
            config.chunk_size,
            config.hidden_dimensions,
            config.latent_dimensions,
            config.attention_heads,
            config.feedforward_dimensions,
            config.encoder_layers,
            config.decoder_layers,
            pretrained,
            device,
        )?;
        let cvae_encoder = (0..config.vae_encoder_layers)
            .map(|_| {
                EncoderLayer::init_vae(
                    config.hidden_dimensions,
                    config.attention_heads,
                    config.feedforward_dimensions,
                    0.1,
                    device,
                )
            })
            .collect();
        Ok(Self {
            model,
            cvae_encoder,
            cvae_cls: Initializer::Normal {
                mean: 0.0,
                std: 1.0,
            }
            .init([1, 1, config.hidden_dimensions], device),
            cvae_state: LinearConfig::new(config.state_dimensions, config.hidden_dimensions)
                .init(device),
            cvae_action: LinearConfig::new(config.action_dimensions, config.hidden_dimensions)
                .init(device),
            latent_stats: LinearConfig::new(config.hidden_dimensions, config.latent_dimensions * 2)
                .init(device),
            chunk_size: config.chunk_size,
            hidden_dimensions: config.hidden_dimensions,
            latent_dimensions: config.latent_dimensions,
        })
    }

    fn forward(
        &self,
        state: Tensor<B, 2>,
        images: Vec<Tensor<B, 4>>,
        actions: Tensor<B, 3>,
        action_is_pad: Tensor<B, 2, Bool>,
    ) -> ModelOutput<B> {
        let [batch, _] = state.dims();
        let cls = self.cvae_cls.val().repeat_dim(0, batch);
        let state_token = self.cvae_state.forward(state.clone()).unsqueeze_dim(1);
        let action_tokens = self.cvae_action.forward(actions);
        let mut tokens = Tensor::cat(vec![cls, state_token, action_tokens], 1);
        let position = sinusoidal_position_embedding_1d::<B>(
            self.chunk_size + 2,
            self.hidden_dimensions,
            &state.device(),
        )
        .unsqueeze_dim(0);
        let prefix_mask = Tensor::<B, 2, Bool>::zeros([batch, 2], &state.device());
        let padding = Tensor::cat(vec![prefix_mask, action_is_pad], 1);
        for layer in &self.cvae_encoder {
            tokens = layer.forward_with_padding(tokens, &position, Some(padding.clone()));
        }
        let encoded = tokens.narrow(1, 0, 1).squeeze_dim(1);
        let stats = self.latent_stats.forward(encoded);
        let mean = stats.clone().narrow(1, 0, self.latent_dimensions);
        let log_variance = stats.narrow(1, self.latent_dimensions, self.latent_dimensions);
        let noise = mean.random_like(Distribution::Normal(0.0, 1.0));
        let latent = mean.clone() + log_variance.clone().mul_scalar(0.5).exp() * noise;
        let hidden = self.model.forward_hidden_with_latent(state, images, latent);
        let actions = self.model.action_head.forward(hidden);
        ModelOutput {
            actions,
            mean,
            log_variance,
        }
    }
}

struct ModelOutput<B: Backend> {
    actions: Tensor<B, 3>,
    mean: Tensor<B, 2>,
    log_variance: Tensor<B, 2>,
}

struct LossOutput<B: Backend> {
    total: Tensor<B, 1>,
    l1: Tensor<B, 1>,
    kl: Tensor<B, 1>,
}

struct DeviceBatch<B: Backend> {
    states: Tensor<B, 2>,
    images: Vec<Tensor<B, 4>>,
    actions: Tensor<B, 3>,
    action_is_pad: Tensor<B, 2, Bool>,
}

struct Trainer<B: AutodiffBackend> {
    config: VisionActTrainingConfig,
    model: Option<FullActModel<B>>,
    optimizer: OptimizerAdaptor<AdamW, FullActModel<B>, B>,
    device: B::Device,
}

impl<B: AutodiffBackend> Trainer<B> {
    fn new(
        config: VisionActTrainingConfig,
        pretrained_weights: Option<&[u8]>,
        device: B::Device,
    ) -> Result<Self> {
        B::seed(&device, config.seed);
        let pretrained = pretrained_weights
            .map(|bytes| Weights::from_bytes("pretrained ACT backbone", bytes.to_vec()))
            .transpose()?;
        let model = FullActModel::init(&config, pretrained.as_ref(), &device)?;
        let optimizer = AdamWConfig::new()
            .with_weight_decay(1.0e-4)
            .with_grad_clipping(Some(GradientClippingConfig::Norm(10.0)))
            .init();
        Ok(Self {
            config,
            model: Some(model),
            optimizer,
            device,
        })
    }

    fn batch<T: Backend<Device = B::Device>>(
        &self,
        batch: VisionActBatch,
    ) -> Result<DeviceBatch<T>> {
        validate_batch(&self.config, &batch)?;
        let VisionActBatch {
            batch_size,
            states,
            images,
            actions,
            action_is_pad,
        } = batch;
        let states = Tensor::from_data(
            TensorData::new(states, [batch_size, self.config.state_dimensions]),
            &self.device,
        );
        let images = Tensor::<T, 5>::from_data(
            TensorData::new(
                images,
                [
                    batch_size,
                    self.config.camera_streams.len(),
                    3,
                    self.config.image_height,
                    self.config.image_width,
                ],
            ),
            &self.device,
        );
        let images = (0..self.config.camera_streams.len())
            .map(|camera| images.clone().narrow(1, camera, 1).squeeze_dim(1))
            .collect();
        Ok(DeviceBatch {
            states,
            images,
            actions: Tensor::from_data(
                TensorData::new(
                    actions,
                    [
                        batch_size,
                        self.config.chunk_size,
                        self.config.action_dimensions,
                    ],
                ),
                &self.device,
            ),
            action_is_pad: Tensor::from_data(
                TensorData::new(action_is_pad, [batch_size, self.config.chunk_size]),
                &self.device,
            ),
        })
    }
}

impl<B: AutodiffBackend> TrainerEngine for Trainer<B>
where
    B::Device: Send,
{
    fn parameter_count(&self) -> usize {
        self.model.as_ref().expect("trainer model").num_params()
    }

    fn learning_rate(&self, step: u64) -> f64 {
        let _ = step;
        self.config.learning_rate
    }

    fn train_step(&mut self, batch: VisionActBatch, step: u64) -> Result<VisionActLoss> {
        let batch = self.batch::<B>(batch)?;
        let model = self.model.as_ref().expect("trainer model");
        let output = model.forward(
            batch.states,
            batch.images,
            batch.actions.clone(),
            batch.action_is_pad.clone(),
        );
        let loss = act_loss(
            output,
            batch.actions,
            batch.action_is_pad,
            self.config.kl_weight,
        );
        let metrics = read_loss::<B::InnerBackend>(
            loss.total.clone().inner(),
            loss.l1.inner(),
            loss.kl.inner(),
        )?;
        if !metrics.total.is_finite() {
            return Err(Error::Inference(format!(
                "ACT loss became non-finite at step {step}"
            )));
        }
        let gradients = GradientsParams::from_grads(loss.total.backward(), model);
        let model = self.model.take().expect("trainer model");
        self.model = Some(
            self.optimizer
                .step(self.learning_rate(step), model, gradients),
        );
        Ok(metrics)
    }

    fn validation_step(&self, batch: VisionActBatch) -> Result<VisionActLoss> {
        let valid = self.model.as_ref().expect("trainer model").valid();
        let batch = self.batch::<B::InnerBackend>(batch)?;
        let output = valid.forward(
            batch.states,
            batch.images,
            batch.actions.clone(),
            batch.action_is_pad.clone(),
        );
        let loss = act_loss(
            output,
            batch.actions,
            batch.action_is_pad,
            self.config.kl_weight,
        );
        read_loss::<B::InnerBackend>(loss.total, loss.l1, loss.kl)
    }

    fn checkpoint(&self, checkpoint: VisionActCheckpoint<'_>) -> Result<Vec<u8>> {
        validate_checkpoint(&self.config, &checkpoint)?;
        let valid = self.model.as_ref().expect("trainer model").valid();
        let mut store = SafetensorsStore::from_bytes(None)
            .with_regex(r"^model\..*")
            .with_to_adapter(InferenceLayoutAdapter)
            .clear_metadata()
            .metadata("format", "spiral-act")
            .metadata("spiral_robotics", checkpoint.manifest_json);
        valid
            .save_into(&mut store)
            .map_err(|error| Error::Checkpoint(format!("saving Burn ACT tensors: {error}")))?;
        let bytes = store
            .get_bytes()
            .map_err(|error| Error::Checkpoint(format!("reading Burn ACT tensors: {error}")))?;
        add_normalization(&self.config, bytes, checkpoint)
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

fn act_loss<B: Backend>(
    output: ModelOutput<B>,
    target: Tensor<B, 3>,
    target_pad: Tensor<B, 2, Bool>,
    kl_weight: f64,
) -> LossOutput<B> {
    let keep = target_pad.clone().bool_not().float().unsqueeze_dim(2);
    let action_values = keep.clone().sum().clamp_min(1.0) * target.dims()[2] as f64;
    let l1 = ((output.actions - target).abs() * keep).sum() / action_values;
    let kl = (output.log_variance.clone().add_scalar(1.0)
        - output.mean.square()
        - output.log_variance.exp())
    .mul_scalar(-0.5)
    .sum_dim(1)
    .mean();
    let total = l1.clone() + kl.clone().mul_scalar(kl_weight);
    LossOutput { total, l1, kl }
}

fn read_loss<B: Backend>(
    total: Tensor<B, 1>,
    l1: Tensor<B, 1>,
    kl: Tensor<B, 1>,
) -> Result<VisionActLoss> {
    Ok(VisionActLoss {
        total: scalar(total)?,
        l1: scalar(l1)?,
        kl: scalar(kl)?,
        // Kept in the public diagnostics shape for older Studio clients. ACT
        // has no pad prediction head or pad loss.
        pad: 0.0,
    })
}

/// Fixed 1D sinusoidal positions used by LeRobot's VAE encoder.
fn sinusoidal_position_embedding_1d<B: Backend>(
    positions: usize,
    dimensions: usize,
    device: &B::Device,
) -> Tensor<B, 2> {
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
    Tensor::from_data(TensorData::new(values, [positions, dimensions]), device)
}

fn scalar<B: Backend>(value: Tensor<B, 1>) -> Result<f64> {
    let values = value
        .try_into_data()
        .map_err(|error| Error::Inference(format!("reading Burn scalar: {error}")))?
        .to_vec::<f32>()
        .map_err(|error| Error::Inference(format!("converting Burn scalar: {error}")))?;
    values
        .first()
        .copied()
        .map(f64::from)
        .ok_or_else(|| Error::Inference("Burn returned an empty scalar tensor".to_owned()))
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

fn add_normalization(
    config: &VisionActTrainingConfig,
    model_bytes: Vec<u8>,
    checkpoint: VisionActCheckpoint<'_>,
) -> Result<Vec<u8>> {
    let tensors = safetensors::SafeTensors::deserialize(&model_bytes)
        .map_err(|error| Error::Checkpoint(format!("reading Burn checkpoint: {error}")))?;
    let mut owned = tensors
        .tensors()
        .into_iter()
        .map(|(name, view)| OwnedTensor {
            name,
            dtype: view.dtype(),
            shape: view.shape().to_vec(),
            bytes: view.data().to_vec(),
        })
        .collect::<Vec<_>>();
    push_f32(
        &mut owned,
        "normalize_inputs.buffer_observation_state.mean",
        vec![config.state_dimensions],
        checkpoint.state_mean,
    );
    push_f32(
        &mut owned,
        "normalize_inputs.buffer_observation_state.std",
        vec![config.state_dimensions],
        checkpoint.state_std,
    );
    for camera in &config.camera_streams {
        let key = camera.replace(['.', '/'], "_");
        push_f32(
            &mut owned,
            &format!("normalize_inputs.buffer_{key}.mean"),
            vec![3, 1, 1],
            &IMAGENET_MEAN,
        );
        push_f32(
            &mut owned,
            &format!("normalize_inputs.buffer_{key}.std"),
            vec![3, 1, 1],
            &IMAGENET_STD,
        );
    }
    push_f32(
        &mut owned,
        "unnormalize_outputs.buffer_action.mean",
        vec![config.action_dimensions],
        checkpoint.action_mean,
    );
    push_f32(
        &mut owned,
        "unnormalize_outputs.buffer_action.std",
        vec![config.action_dimensions],
        checkpoint.action_std,
    );
    let views = owned
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resnet_shape_matches_the_runtime_feature_contract() {
        assert_eq!(resnet18_feature_size(224, 224), (7, 7));
        assert_eq!(resnet18_feature_size(64, 64), (2, 2));
    }

    #[test]
    fn act_learning_rate_has_no_scheduler() {
        let config = VisionActTrainingConfig {
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
        };
        let trainer = NativeActTrainer::new(config, "cpu", None).unwrap();
        assert_eq!(trainer.learning_rate(0), 1.0e-5);
        assert_eq!(trainer.learning_rate(50), 1.0e-5);
        assert_eq!(trainer.learning_rate(100), 1.0e-5);
    }

    #[test]
    fn vae_positions_match_the_fixed_lerobot_sinusoid() {
        type B = burn_ndarray::NdArray<f32>;
        let positions = sinusoidal_position_embedding_1d::<B>(3, 8, &Default::default())
            .into_data()
            .to_vec::<f32>()
            .unwrap();
        assert_eq!(&positions[..8], &[0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
        assert!((positions[8] - 1.0_f32.sin()).abs() < 1.0e-6);
        assert!((positions[9] - 1.0_f32.cos()).abs() < 1.0e-6);
        assert!((positions[10] - 0.1_f32.sin()).abs() < 1.0e-6);
        assert!((positions[11] - 0.1_f32.cos()).abs() < 1.0e-6);
    }

    #[test]
    fn vae_depth_is_independent_from_the_main_encoder_depth() {
        type B = burn_ndarray::NdArray<f32>;
        let config = VisionActTrainingConfig {
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
            vae_encoder_layers: 3,
            decoder_layers: 1,
            kl_weight: 1.0,
            learning_rate: 1.0e-5,
            total_steps: 1,
            seed: 1_000,
        };
        let model = FullActModel::<B>::init(&config, None, &Default::default()).unwrap();
        assert_eq!(model.model.encoder_layer_count(), 1);
        assert_eq!(model.cvae_encoder.len(), 3);
    }

    #[test]
    fn burn_checkpoint_loads_through_the_native_inference_boundary() {
        use crate::{ModelArtifact, Policy};

        let camera = "wrist".to_owned();
        let config = VisionActTrainingConfig {
            state_dimensions: 6,
            action_dimensions: 6,
            camera_streams: vec![camera.clone()],
            image_width: 32,
            image_height: 32,
            chunk_size: 4,
            hidden_dimensions: 64,
            latent_dimensions: 8,
            attention_heads: 8,
            feedforward_dimensions: 128,
            encoder_layers: 1,
            vae_encoder_layers: 2,
            decoder_layers: 1,
            kl_weight: 10.0,
            learning_rate: 1.0e-5,
            total_steps: 1,
            seed: 42,
        };
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
                .names()
                .iter()
                .all(|name| !name.contains("camera_embedding") && !name.contains("pad_head")),
            "new native ACT checkpoints must not contain Spiral-only camera or pad heads"
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
            "n_vae_encoder_layers": 2,
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

    /// Burn 0.21's optional fusion planner reuses an invalid cached ordering
    /// after a recurring graph synchronizes scalar losses, and CubeCL 0.10's
    /// autotune profiler can invalidate its Metal timestamp readback buffer.
    /// Keep the full train/validate/checkpoint sequence covered on eager Metal
    /// so either feature can't silently restore a training-loop panic.
    #[cfg(all(target_os = "macos", feature = "metal"))]
    #[test]
    fn metal_repeats_training_sync_segments_and_checkpoints() {
        if !super::super::engine::metal_available() {
            return;
        }

        let config = VisionActTrainingConfig {
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
            total_steps: 2,
            seed: 42,
        };
        let batch = || VisionActBatch {
            batch_size: 1,
            states: vec![0.0; 2],
            images: vec![0.0; 3 * 32 * 32],
            actions: vec![0.0; 2 * 2],
            action_is_pad: vec![false; 2],
        };
        let mut trainer = NativeActTrainer::new(config, "metal", None).unwrap();

        for step in 1..=2 {
            assert!(trainer.train_step(batch(), step).unwrap().total.is_finite());
        }
        assert!(trainer.validation_step(batch()).unwrap().total.is_finite());

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
}
