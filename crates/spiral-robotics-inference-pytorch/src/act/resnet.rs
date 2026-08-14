use tch::nn::Module;
use tch::{Tensor, nn};

#[derive(Debug)]
struct FrozenBatchNorm2d {
    weight: Tensor,
    bias: Tensor,
    running_mean: Tensor,
    running_var: Tensor,
}

impl FrozenBatchNorm2d {
    fn new(path: &nn::Path<'_>, channels: i64) -> Self {
        Self {
            weight: path.ones_no_train("weight", &[channels]),
            bias: path.zeros_no_train("bias", &[channels]),
            running_mean: path.zeros_no_train("running_mean", &[channels]),
            running_var: path.ones_no_train("running_var", &[channels]),
        }
    }

    fn forward(&self, input: &Tensor) -> Tensor {
        let scale = &self.weight * (&self.running_var + 1.0e-5).rsqrt();
        let offset = &self.bias - &self.running_mean * &scale;
        input * scale.view([1, -1, 1, 1]) + offset.view([1, -1, 1, 1])
    }
}

fn conv2d(
    path: &nn::Path<'_>,
    input_channels: i64,
    output_channels: i64,
    kernel: i64,
    stride: i64,
    padding: i64,
) -> nn::Conv2D {
    nn::conv2d(
        path,
        input_channels,
        output_channels,
        kernel,
        nn::ConvConfig {
            stride,
            padding,
            bias: false,
            ws_init: nn::Init::Kaiming {
                dist: nn::init::NormalOrUniform::Normal,
                fan: nn::init::FanInOut::FanOut,
                non_linearity: nn::init::NonLinearity::ReLU,
            },
            ..Default::default()
        },
    )
}

#[derive(Debug)]
struct BasicBlock {
    conv1: nn::Conv2D,
    norm1: FrozenBatchNorm2d,
    conv2: nn::Conv2D,
    norm2: FrozenBatchNorm2d,
    downsample: Option<(nn::Conv2D, FrozenBatchNorm2d)>,
}

impl BasicBlock {
    fn new(path: &nn::Path<'_>, input_channels: i64, output_channels: i64, stride: i64) -> Self {
        let downsample = if stride != 1 || input_channels != output_channels {
            Some((
                conv2d(
                    &(path / "downsample" / "0"),
                    input_channels,
                    output_channels,
                    1,
                    stride,
                    0,
                ),
                FrozenBatchNorm2d::new(&(path / "downsample" / "1"), output_channels),
            ))
        } else {
            None
        };
        Self {
            conv1: conv2d(
                &(path / "conv1"),
                input_channels,
                output_channels,
                3,
                stride,
                1,
            ),
            norm1: FrozenBatchNorm2d::new(&(path / "bn1"), output_channels),
            conv2: conv2d(&(path / "conv2"), output_channels, output_channels, 3, 1, 1),
            norm2: FrozenBatchNorm2d::new(&(path / "bn2"), output_channels),
            downsample,
        }
    }

    fn forward(&self, input: &Tensor) -> Tensor {
        let residual = match &self.downsample {
            Some((conv, norm)) => norm.forward(&conv.forward(input)),
            None => input.shallow_clone(),
        };
        let output = self.norm1.forward(&self.conv1.forward(input)).relu();
        let output = self.norm2.forward(&self.conv2.forward(&output));
        (output + residual).relu()
    }
}

/// Exact feature-map portion of torchvision's ResNet-18 used by LeRobot ACT.
#[derive(Debug)]
pub(super) struct ResNet18 {
    conv1: nn::Conv2D,
    norm1: FrozenBatchNorm2d,
    layers: Vec<Vec<BasicBlock>>,
}

impl ResNet18 {
    pub fn new(path: &nn::Path<'_>) -> Self {
        let channels = [64, 128, 256, 512];
        let mut input_channels = 64;
        let mut layers = Vec::with_capacity(channels.len());
        for (index, output_channels) in channels.into_iter().enumerate() {
            let stride = if index == 0 { 1 } else { 2 };
            let layer = path / format!("layer{}", index + 1);
            layers.push(vec![
                BasicBlock::new(&(&layer / "0"), input_channels, output_channels, stride),
                BasicBlock::new(&(&layer / "1"), output_channels, output_channels, 1),
            ]);
            input_channels = output_channels;
        }
        Self {
            conv1: conv2d(&(path / "conv1"), 3, 64, 7, 2, 3),
            norm1: FrozenBatchNorm2d::new(&(path / "bn1"), 64),
            layers,
        }
    }

    pub fn forward(&self, input: &Tensor) -> Tensor {
        let mut output = self.norm1.forward(&self.conv1.forward(input)).relu();
        output = output.max_pool2d([3, 3], [2, 2], [1, 1], [1, 1], false);
        for layer in &self.layers {
            for block in layer {
                output = block.forward(&output);
            }
        }
        output
    }
}
