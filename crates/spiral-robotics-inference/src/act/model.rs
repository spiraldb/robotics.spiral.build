use std::f32::consts::TAU;

use burn_core::module::Param;
use burn_core::tensor::backend::Backend;
use burn_core::tensor::{Tensor, TensorData};
use burn_nn::conv::Conv2d;
use burn_nn::{LayerNorm, Linear, PaddingConfig2d};

use crate::Result;

use super::config::ActConfig;
use super::layers::{DecoderLayer, EncoderLayer, load_layer_norm, load_linear};
use super::resnet::ResNet18;
use super::weights::Weights;

pub(super) struct ActModel<B: Backend> {
    backbone: ResNet18<B>,
    image_projection: Conv2d<B>,
    state_projection: Linear<B>,
    latent_token: Tensor<B, 3>,
    encoder_position: Tensor<B, 3>,
    encoder: Vec<EncoderLayer<B>>,
    decoder_position: Tensor<B, 3>,
    decoder: Vec<DecoderLayer<B>>,
    decoder_norm: LayerNorm<B>,
    action_head: Linear<B>,
    chunk_size: usize,
    dimensions: usize,
}

impl<B: Backend> ActModel<B> {
    pub fn load(config: &ActConfig, weights: &Weights, device: &B::Device) -> Result<Self> {
        let prefix = "model";
        let backbone = ResNet18::load(weights, &format!("{prefix}.backbone"), device)?;
        let image_projection = load_image_projection(
            weights,
            &format!("{prefix}.encoder_img_feat_input_proj"),
            config.dim_model,
            device,
        )?;
        let state_projection = load_linear(
            weights,
            &format!("{prefix}.encoder_robot_state_input_proj"),
            config.state_dimensions(),
            config.dim_model,
            device,
        )?;
        let latent_projection = load_linear(
            weights,
            &format!("{prefix}.encoder_latent_input_proj"),
            config.latent_dim,
            config.dim_model,
            device,
        )?;
        let latent_token = latent_projection
            .forward(Tensor::<B, 2>::zeros([1, config.latent_dim], device))
            .unsqueeze_dim(1);

        let (feature_height, feature_width) = config.image_feature_size();
        let image_position =
            position_embedding_2d::<B>(feature_height, feature_width, config.dim_model, device);
        let one_dimensional_position: Tensor<B, 2> = weights.tensor_exact(
            &format!("{prefix}.encoder_1d_feature_pos_embed.weight"),
            [2, config.dim_model],
            device,
        )?;
        let encoder_position = Tensor::cat(
            vec![one_dimensional_position.unsqueeze_dim(0), image_position],
            1,
        );

        let mut encoder = Vec::new();
        for layer in 0..config.n_encoder_layers {
            encoder.push(EncoderLayer::load(
                weights,
                &format!("{prefix}.encoder.layers.{layer}"),
                config.dim_model,
                config.n_heads,
                config.dim_feedforward,
                device,
            )?);
        }

        let decoder_position: Tensor<B, 2> = weights.tensor_exact(
            &format!("{prefix}.decoder_pos_embed.weight"),
            [config.chunk_size, config.dim_model],
            device,
        )?;
        let decoder_position = decoder_position.unsqueeze_dim(0);
        let mut decoder = Vec::new();
        for layer in 0..config.n_decoder_layers {
            decoder.push(DecoderLayer::load(
                weights,
                &format!("{prefix}.decoder.layers.{layer}"),
                config.dim_model,
                config.n_heads,
                config.dim_feedforward,
                device,
            )?);
        }
        let decoder_norm = load_layer_norm(
            weights,
            &format!("{prefix}.decoder.norm"),
            config.dim_model,
            device,
        )?;
        let action_head = load_linear(
            weights,
            &format!("{prefix}.action_head"),
            config.dim_model,
            config.action_dimensions(),
            device,
        )?;

        Ok(Self {
            backbone,
            image_projection,
            state_projection,
            latent_token,
            encoder_position,
            encoder,
            decoder_position,
            decoder,
            decoder_norm,
            action_head,
            chunk_size: config.chunk_size,
            dimensions: config.dim_model,
        })
    }

    /// Inputs are normalized and batch-first with a fixed batch size of one.
    pub fn forward(&self, state: Tensor<B, 2>, image: Tensor<B, 4>) -> Tensor<B, 3> {
        let features = self.image_projection.forward(self.backbone.forward(image));
        let [batch, channels, height, width] = features.dims();
        let image_tokens = features
            .reshape([batch, channels, height * width])
            .swap_dims(1, 2);
        let state_token = self.state_projection.forward(state).unsqueeze_dim(1);
        let mut encoder_output = Tensor::cat(
            vec![self.latent_token.clone(), state_token, image_tokens],
            1,
        );
        for layer in &self.encoder {
            encoder_output = layer.forward(encoder_output, &self.encoder_position);
        }

        let mut decoder_output = Tensor::zeros(
            [batch, self.chunk_size, self.dimensions],
            &encoder_output.device(),
        );
        for layer in &self.decoder {
            decoder_output = layer.forward(
                decoder_output,
                &encoder_output,
                &self.decoder_position,
                &self.encoder_position,
            );
        }
        self.action_head
            .forward(self.decoder_norm.forward(decoder_output))
    }
}

fn load_image_projection<B: Backend>(
    weights: &Weights,
    prefix: &str,
    dimensions: usize,
    device: &B::Device,
) -> Result<Conv2d<B>> {
    let weight: Tensor<B, 4> =
        weights.tensor_exact(&format!("{prefix}.weight"), [dimensions, 512, 1, 1], device)?;
    let bias: Tensor<B, 1> =
        weights.tensor_exact(&format!("{prefix}.bias"), [dimensions], device)?;
    Ok(Conv2d {
        weight: Param::from_tensor(weight),
        bias: Some(Param::from_tensor(bias)),
        stride: [1, 1],
        kernel_size: [1, 1],
        dilation: [1, 1],
        groups: 1,
        padding: PaddingConfig2d::Valid,
    })
}

/// DETR-style, normalized 2D positional embedding used by LeRobot ACT.
fn position_embedding_2d<B: Backend>(
    height: usize,
    width: usize,
    dimensions: usize,
    device: &B::Device,
) -> Tensor<B, 3> {
    let axis_dimensions = dimensions / 2;
    let inverse_frequency = (0..axis_dimensions)
        .map(|index| 10_000.0_f32.powf(2.0 * ((index / 2) as f32) / axis_dimensions as f32))
        .collect::<Vec<_>>();
    let mut values = vec![0.0; height * width * dimensions];
    for y in 0..height {
        let y_position = (y + 1) as f32 / (height as f32 + 1.0e-6) * TAU;
        for x in 0..width {
            let x_position = (x + 1) as f32 / (width as f32 + 1.0e-6) * TAU;
            let token = (y * width + x) * dimensions;
            for index in 0..axis_dimensions {
                let y_angle = y_position / inverse_frequency[index];
                let x_angle = x_position / inverse_frequency[index];
                values[token + index] = if index % 2 == 0 {
                    y_angle.sin()
                } else {
                    y_angle.cos()
                };
                values[token + axis_dimensions + index] = if index % 2 == 0 {
                    x_angle.sin()
                } else {
                    x_angle.cos()
                };
            }
        }
    }
    Tensor::from_data(
        TensorData::new(values, [1, height * width, dimensions]),
        device,
    )
}
