use burn_core as burn;
use burn_core::module::{Module, Param, RunningState};
use burn_core::tensor::Tensor;
use burn_core::tensor::activation::relu;
use burn_core::tensor::backend::Backend;
use burn_core::tensor::module::max_pool2d;
use burn_nn::PaddingConfig2d;
use burn_nn::conv::Conv2d;
use burn_nn::conv::Conv2dConfig;

use crate::Result;

use super::weights::Weights;

#[derive(Module, Debug)]
struct FrozenBatchNorm2d<B: Backend> {
    scale: RunningState<Tensor<B, 1>>,
    offset: RunningState<Tensor<B, 1>>,
}

impl<B: Backend> FrozenBatchNorm2d<B> {
    fn load(weights: &Weights, prefix: &str, channels: usize, device: &B::Device) -> Result<Self> {
        if weights.contains(&format!("{prefix}.scale")) {
            return Ok(Self {
                scale: RunningState::new(weights.tensor_exact(
                    &format!("{prefix}.scale"),
                    [channels],
                    device,
                )?),
                offset: RunningState::new(weights.tensor_exact(
                    &format!("{prefix}.offset"),
                    [channels],
                    device,
                )?),
            });
        }
        let gamma: Tensor<B, 1> =
            weights.tensor_exact(&format!("{prefix}.weight"), [channels], device)?;
        let beta: Tensor<B, 1> =
            weights.tensor_exact(&format!("{prefix}.bias"), [channels], device)?;
        let mean: Tensor<B, 1> =
            weights.tensor_exact(&format!("{prefix}.running_mean"), [channels], device)?;
        let variance: Tensor<B, 1> =
            weights.tensor_exact(&format!("{prefix}.running_var"), [channels], device)?;
        // The reference ACT FrozenBatchNorm2d uses eps = 1e-5.
        let scale = gamma / variance.add_scalar(1.0e-5).sqrt();
        let offset = beta - mean * scale.clone();
        Ok(Self {
            scale: RunningState::new(scale),
            offset: RunningState::new(offset),
        })
    }

    fn init(channels: usize, device: &B::Device) -> Self {
        Self {
            scale: RunningState::new(Tensor::ones([channels], device)),
            offset: RunningState::new(Tensor::zeros([channels], device)),
        }
    }

    fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let scale = self.scale.value();
        let [channels] = scale.dims();
        input * scale.reshape([1, channels, 1, 1])
            + self.offset.value().reshape([1, channels, 1, 1])
    }
}

#[allow(clippy::too_many_arguments)]
fn load_conv2d<B: Backend>(
    weights: &Weights,
    prefix: &str,
    input_channels: usize,
    output_channels: usize,
    kernel: usize,
    stride: usize,
    padding: usize,
    device: &B::Device,
) -> Result<Conv2d<B>> {
    let weight: Tensor<B, 4> = weights.tensor_exact(
        &format!("{prefix}.weight"),
        [output_channels, input_channels, kernel, kernel],
        device,
    )?;
    Ok(Conv2d {
        weight: Param::from_tensor(weight),
        bias: None,
        stride: [stride, stride],
        kernel_size: [kernel, kernel],
        dilation: [1, 1],
        groups: 1,
        padding: PaddingConfig2d::Explicit(padding, padding, padding, padding),
    })
}

fn init_conv2d<B: Backend>(
    input_channels: usize,
    output_channels: usize,
    kernel: usize,
    stride: usize,
    padding: usize,
    device: &B::Device,
) -> Conv2d<B> {
    Conv2dConfig::new([input_channels, output_channels], [kernel, kernel])
        .with_stride([stride, stride])
        .with_padding(PaddingConfig2d::Explicit(
            padding, padding, padding, padding,
        ))
        .with_bias(false)
        .init(device)
}

#[derive(Module, Debug)]
struct BasicBlock<B: Backend> {
    conv1: Conv2d<B>,
    norm1: FrozenBatchNorm2d<B>,
    conv2: Conv2d<B>,
    norm2: FrozenBatchNorm2d<B>,
    downsample: Option<(Conv2d<B>, FrozenBatchNorm2d<B>)>,
}

impl<B: Backend> BasicBlock<B> {
    fn init(
        input_channels: usize,
        output_channels: usize,
        stride: usize,
        device: &B::Device,
    ) -> Self {
        let downsample = if stride != 1 || input_channels != output_channels {
            Some((
                init_conv2d(input_channels, output_channels, 1, stride, 0, device),
                FrozenBatchNorm2d::init(output_channels, device),
            ))
        } else {
            None
        };
        Self {
            conv1: init_conv2d(input_channels, output_channels, 3, stride, 1, device),
            norm1: FrozenBatchNorm2d::init(output_channels, device),
            conv2: init_conv2d(output_channels, output_channels, 3, 1, 1, device),
            norm2: FrozenBatchNorm2d::init(output_channels, device),
            downsample,
        }
    }

    fn load(
        weights: &Weights,
        prefix: &str,
        input_channels: usize,
        output_channels: usize,
        stride: usize,
        device: &B::Device,
    ) -> Result<Self> {
        let native = weights.contains(&format!("{prefix}.norm1.scale"));
        let downsample = if stride != 1 || input_channels != output_channels {
            Some((
                load_conv2d(
                    weights,
                    &format!("{prefix}.downsample.0"),
                    input_channels,
                    output_channels,
                    1,
                    stride,
                    0,
                    device,
                )?,
                FrozenBatchNorm2d::load(
                    weights,
                    &format!("{prefix}.downsample.1"),
                    output_channels,
                    device,
                )?,
            ))
        } else {
            None
        };
        Ok(Self {
            conv1: load_conv2d(
                weights,
                &format!("{prefix}.conv1"),
                input_channels,
                output_channels,
                3,
                stride,
                1,
                device,
            )?,
            norm1: FrozenBatchNorm2d::load(
                weights,
                &format!("{prefix}.{}", if native { "norm1" } else { "bn1" }),
                output_channels,
                device,
            )?,
            conv2: load_conv2d(
                weights,
                &format!("{prefix}.conv2"),
                output_channels,
                output_channels,
                3,
                1,
                1,
                device,
            )?,
            norm2: FrozenBatchNorm2d::load(
                weights,
                &format!("{prefix}.{}", if native { "norm2" } else { "bn2" }),
                output_channels,
                device,
            )?,
            downsample,
        })
    }

    fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let residual = match &self.downsample {
            Some((conv, norm)) => norm.forward(conv.forward(input.clone())),
            None => input.clone(),
        };
        let output = relu(self.norm1.forward(self.conv1.forward(input)));
        let output = self.norm2.forward(self.conv2.forward(output));
        relu(output + residual)
    }
}

/// Exact feature-map portion of the reference ACT ResNet-18.
#[derive(Module, Debug)]
pub(super) struct ResNet18<B: Backend> {
    conv1: Conv2d<B>,
    norm1: FrozenBatchNorm2d<B>,
    layers: Vec<Vec<BasicBlock<B>>>,
}

impl<B: Backend> ResNet18<B> {
    pub fn init(device: &B::Device) -> Self {
        let channels = [64, 128, 256, 512];
        let mut input_channels = 64;
        let mut layers = Vec::new();
        for (index, output_channels) in channels.into_iter().enumerate() {
            let stride = if index == 0 { 1 } else { 2 };
            layers.push(vec![
                BasicBlock::init(input_channels, output_channels, stride, device),
                BasicBlock::init(output_channels, output_channels, 1, device),
            ]);
            input_channels = output_channels;
        }
        Self {
            conv1: init_conv2d(3, 64, 7, 2, 3, device),
            norm1: FrozenBatchNorm2d::init(64, device),
            layers,
        }
    }

    pub fn load(weights: &Weights, prefix: &str, device: &B::Device) -> Result<Self> {
        let native = weights.contains(&format!("{prefix}.layers.0.0.conv1.weight"));
        let channels = [64, 128, 256, 512];
        let mut input_channels = 64;
        let mut layers = Vec::new();
        for (index, output_channels) in channels.into_iter().enumerate() {
            let layer = index + 1;
            let stride = if layer == 1 { 1 } else { 2 };
            layers.push(vec![
                BasicBlock::load(
                    weights,
                    &format!(
                        "{prefix}.{}",
                        if native {
                            format!("layers.{index}.0")
                        } else {
                            format!("layer{layer}.0")
                        }
                    ),
                    input_channels,
                    output_channels,
                    stride,
                    device,
                )?,
                BasicBlock::load(
                    weights,
                    &format!(
                        "{prefix}.{}",
                        if native {
                            format!("layers.{index}.1")
                        } else {
                            format!("layer{layer}.1")
                        }
                    ),
                    output_channels,
                    output_channels,
                    1,
                    device,
                )?,
            ]);
            input_channels = output_channels;
        }
        Ok(Self {
            conv1: load_conv2d(weights, &format!("{prefix}.conv1"), 3, 64, 7, 2, 3, device)?,
            norm1: FrozenBatchNorm2d::load(
                weights,
                &format!("{prefix}.{}", if native { "norm1" } else { "bn1" }),
                64,
                device,
            )?,
            layers,
        })
    }

    pub fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let mut output = relu(self.norm1.forward(self.conv1.forward(input)));
        output = max_pool2d(output, [3, 3], [2, 2], [1, 1], [1, 1], false);
        for layer in &self.layers {
            for block in layer {
                output = block.forward(output);
            }
        }
        output
    }
}
