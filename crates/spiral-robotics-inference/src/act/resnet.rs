use burn_core::module::Param;
use burn_core::tensor::Tensor;
use burn_core::tensor::activation::relu;
use burn_core::tensor::backend::Backend;
use burn_core::tensor::module::max_pool2d;
use burn_nn::PaddingConfig2d;
use burn_nn::conv::Conv2d;

use crate::Result;

use super::weights::Weights;

struct FrozenBatchNorm2d<B: Backend> {
    scale: Tensor<B, 1>,
    offset: Tensor<B, 1>,
}

impl<B: Backend> FrozenBatchNorm2d<B> {
    fn load(weights: &Weights, prefix: &str, channels: usize, device: &B::Device) -> Result<Self> {
        let gamma: Tensor<B, 1> =
            weights.tensor_exact(&format!("{prefix}.weight"), [channels], device)?;
        let beta: Tensor<B, 1> =
            weights.tensor_exact(&format!("{prefix}.bias"), [channels], device)?;
        let mean: Tensor<B, 1> =
            weights.tensor_exact(&format!("{prefix}.running_mean"), [channels], device)?;
        let variance: Tensor<B, 1> =
            weights.tensor_exact(&format!("{prefix}.running_var"), [channels], device)?;
        // torchvision.ops.misc.FrozenBatchNorm2d uses eps = 1e-5.
        let scale = gamma / variance.add_scalar(1.0e-5).sqrt();
        let offset = beta - mean * scale.clone();
        Ok(Self { scale, offset })
    }

    fn forward(&self, input: Tensor<B, 4>) -> Tensor<B, 4> {
        let [channels] = self.scale.dims();
        input * self.scale.clone().reshape([1, channels, 1, 1])
            + self.offset.clone().reshape([1, channels, 1, 1])
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

struct BasicBlock<B: Backend> {
    conv1: Conv2d<B>,
    norm1: FrozenBatchNorm2d<B>,
    conv2: Conv2d<B>,
    norm2: FrozenBatchNorm2d<B>,
    downsample: Option<(Conv2d<B>, FrozenBatchNorm2d<B>)>,
}

impl<B: Backend> BasicBlock<B> {
    fn load(
        weights: &Weights,
        prefix: &str,
        input_channels: usize,
        output_channels: usize,
        stride: usize,
        device: &B::Device,
    ) -> Result<Self> {
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
                &format!("{prefix}.bn1"),
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
                &format!("{prefix}.bn2"),
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

/// Exact feature-map portion of torchvision ResNet-18.
pub(super) struct ResNet18<B: Backend> {
    conv1: Conv2d<B>,
    norm1: FrozenBatchNorm2d<B>,
    layers: Vec<Vec<BasicBlock<B>>>,
}

impl<B: Backend> ResNet18<B> {
    pub fn load(weights: &Weights, prefix: &str, device: &B::Device) -> Result<Self> {
        let channels = [64, 128, 256, 512];
        let mut input_channels = 64;
        let mut layers = Vec::new();
        for (index, output_channels) in channels.into_iter().enumerate() {
            let layer = index + 1;
            let stride = if layer == 1 { 1 } else { 2 };
            layers.push(vec![
                BasicBlock::load(
                    weights,
                    &format!("{prefix}.layer{layer}.0"),
                    input_channels,
                    output_channels,
                    stride,
                    device,
                )?,
                BasicBlock::load(
                    weights,
                    &format!("{prefix}.layer{layer}.1"),
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
            norm1: FrozenBatchNorm2d::load(weights, &format!("{prefix}.bn1"), 64, device)?,
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
