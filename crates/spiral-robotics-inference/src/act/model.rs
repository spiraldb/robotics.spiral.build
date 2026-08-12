use std::f32::consts::TAU;

use burn_core as burn;
use burn_core::module::Initializer;
use burn_core::module::{Module, Param};
use burn_core::tensor::backend::Backend;
use burn_core::tensor::{Tensor, TensorData};
use burn_nn::conv::Conv2d;
use burn_nn::conv::Conv2dConfig;
use burn_nn::{Embedding, LayerNorm, Linear, PaddingConfig2d};
use burn_nn::{EmbeddingConfig, LayerNormConfig, LinearConfig};

use crate::Result;

use super::config::ActConfig;
use super::layers::{DecoderLayer, EncoderLayer, load_layer_norm, load_linear};
use super::resnet::ResNet18;
use super::weights::Weights;

#[derive(Module, Debug)]
pub(super) struct ActModel<B: Backend> {
    backbone: ResNet18<B>,
    image_projection: Conv2d<B>,
    state_projection: Linear<B>,
    latent_projection: Linear<B>,
    encoder_1d_position: Embedding<B>,
    camera_embedding: Embedding<B>,
    encoder: Vec<EncoderLayer<B>>,
    decoder_position: Embedding<B>,
    decoder: Vec<DecoderLayer<B>>,
    decoder_norm: LayerNorm<B>,
    pub(super) action_head: Linear<B>,
    image_feature_sizes: Vec<(usize, usize)>,
    chunk_size: usize,
    dimensions: usize,
    latent_dimensions: usize,
}

impl<B: Backend> ActModel<B> {
    pub fn load(config: &ActConfig, weights: &Weights, device: &B::Device) -> Result<Self> {
        let prefix = "model";
        let native = weights.contains(&format!("{prefix}.image_projection.weight"));
        let backbone = ResNet18::load(weights, &format!("{prefix}.backbone"), device)?;
        let image_projection = load_image_projection(
            weights,
            &format!(
                "{prefix}.{}",
                if native {
                    "image_projection"
                } else {
                    "encoder_img_feat_input_proj"
                }
            ),
            config.dim_model,
            device,
        )?;
        let state_projection = load_linear(
            weights,
            &format!(
                "{prefix}.{}",
                if native {
                    "state_projection"
                } else {
                    "encoder_robot_state_input_proj"
                }
            ),
            config.state_dimensions(),
            config.dim_model,
            device,
        )?;
        let latent_projection = load_linear(
            weights,
            &format!(
                "{prefix}.{}",
                if native {
                    "latent_projection"
                } else {
                    "encoder_latent_input_proj"
                }
            ),
            config.latent_dim,
            config.dim_model,
            device,
        )?;

        let one_dimensional_position: Tensor<B, 2> = weights.tensor_exact(
            &format!(
                "{prefix}.{}.weight",
                if native {
                    "encoder_1d_position"
                } else {
                    "encoder_1d_feature_pos_embed"
                }
            ),
            [2, config.dim_model],
            device,
        )?;
        let camera_embedding = if config.camera_embedding || native {
            weights.tensor_exact::<B, 2>(
                &format!(
                    "{prefix}.{}.weight",
                    if native {
                        "camera_embedding"
                    } else {
                        "camera_embed"
                    }
                ),
                [config.image_feature_sizes().len(), config.dim_model],
                device,
            )?
        } else {
            Tensor::zeros(
                [config.image_feature_sizes().len(), config.dim_model],
                device,
            )
        };
        let encoder_1d_position = Embedding {
            weight: Param::from_tensor(one_dimensional_position),
        };
        let camera_embedding = Embedding {
            weight: Param::from_tensor(camera_embedding),
        };

        let mut encoder = Vec::new();
        for layer in 0..config.n_encoder_layers {
            encoder.push(EncoderLayer::load(
                weights,
                &format!(
                    "{prefix}.{}.{layer}",
                    if native { "encoder" } else { "encoder.layers" }
                ),
                config.dim_model,
                config.n_heads,
                config.dim_feedforward,
                device,
            )?);
        }

        let decoder_position: Tensor<B, 2> = weights.tensor_exact(
            &format!(
                "{prefix}.{}.weight",
                if native {
                    "decoder_position"
                } else {
                    "decoder_pos_embed"
                }
            ),
            [config.chunk_size, config.dim_model],
            device,
        )?;
        let decoder_position = Embedding {
            weight: Param::from_tensor(decoder_position),
        };
        let mut decoder = Vec::new();
        for layer in 0..config.n_decoder_layers {
            decoder.push(DecoderLayer::load(
                weights,
                &format!(
                    "{prefix}.{}.{layer}",
                    if native { "decoder" } else { "decoder.layers" }
                ),
                config.dim_model,
                config.n_heads,
                config.dim_feedforward,
                device,
            )?);
        }
        let decoder_norm = load_layer_norm(
            weights,
            &format!(
                "{prefix}.{}",
                if native {
                    "decoder_norm"
                } else {
                    "decoder.norm"
                }
            ),
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
            latent_projection,
            encoder_1d_position,
            camera_embedding,
            encoder,
            decoder_position,
            decoder,
            decoder_norm,
            action_head,
            image_feature_sizes: config.image_feature_sizes().to_vec(),
            chunk_size: config.chunk_size,
            dimensions: config.dim_model,
            latent_dimensions: config.latent_dim,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn init(
        state_dimensions: usize,
        action_dimensions: usize,
        image_feature_sizes: Vec<(usize, usize)>,
        chunk_size: usize,
        dimensions: usize,
        latent_dimensions: usize,
        heads: usize,
        feedforward_dimensions: usize,
        encoder_layers: usize,
        decoder_layers: usize,
        pretrained: Option<&Weights>,
        device: &B::Device,
    ) -> Result<Self> {
        let backbone = if let Some(weights) = pretrained {
            ResNet18::load(weights, "model.backbone", device)?
        } else {
            ResNet18::init(device)
        };
        let image_projection = Conv2dConfig::new([512, dimensions], [1, 1]).init(device);
        let state_projection = LinearConfig::new(state_dimensions, dimensions).init(device);
        let latent_projection = LinearConfig::new(latent_dimensions, dimensions).init(device);
        let encoder_1d_position = EmbeddingConfig::new(2, dimensions)
            .with_initializer(Initializer::Normal {
                mean: 0.0,
                std: 0.02,
            })
            .init(device);
        let camera_embedding =
            EmbeddingConfig::new(image_feature_sizes.len(), dimensions).init(device);
        let encoder = (0..encoder_layers)
            .map(|_| EncoderLayer::init(dimensions, heads, feedforward_dimensions, 0.1, device))
            .collect();
        let decoder_position = EmbeddingConfig::new(chunk_size, dimensions)
            .with_initializer(Initializer::Normal {
                mean: 0.0,
                std: 0.02,
            })
            .init(device);
        let decoder = (0..decoder_layers)
            .map(|_| DecoderLayer::init(dimensions, heads, feedforward_dimensions, 0.1, device))
            .collect();
        Ok(Self {
            backbone,
            image_projection,
            state_projection,
            latent_projection,
            encoder_1d_position,
            camera_embedding,
            encoder,
            decoder_position,
            decoder,
            decoder_norm: LayerNormConfig::new(dimensions).init(device),
            action_head: LinearConfig::new(dimensions, action_dimensions).init(device),
            image_feature_sizes,
            chunk_size,
            dimensions,
            latent_dimensions,
        })
    }

    /// Inputs are normalized and batch-first with a fixed batch size of one.
    pub fn forward(&self, state: Tensor<B, 2>, images: Vec<Tensor<B, 4>>) -> Tensor<B, 3> {
        let [batch, _] = state.dims();
        let latent = Tensor::zeros([batch, self.latent_dimensions], &state.device());
        self.forward_with_latent(state, images, latent)
    }

    pub fn forward_with_latent(
        &self,
        state: Tensor<B, 2>,
        images: Vec<Tensor<B, 4>>,
        latent: Tensor<B, 2>,
    ) -> Tensor<B, 3> {
        let hidden = self.forward_hidden_with_latent(state, images, latent);
        self.action_head.forward(hidden)
    }

    pub fn forward_hidden_with_latent(
        &self,
        state: Tensor<B, 2>,
        images: Vec<Tensor<B, 4>>,
        latent: Tensor<B, 2>,
    ) -> Tensor<B, 3> {
        let mut image_tokens = Vec::with_capacity(images.len());
        for image in images {
            let features = self.image_projection.forward(self.backbone.forward(image));
            let [batch, channels, height, width] = features.dims();
            image_tokens.push(
                features
                    .reshape([batch, channels, height * width])
                    .swap_dims(1, 2),
            );
        }
        let state_token = self.state_projection.forward(state).unsqueeze_dim(1);
        let latent_token = self.latent_projection.forward(latent).unsqueeze_dim(1);
        let mut encoder_inputs = vec![latent_token, state_token];
        encoder_inputs.extend(image_tokens);
        let mut encoder_output = Tensor::cat(encoder_inputs, 1);
        let mut encoder_positions = vec![self.encoder_1d_position.weight.val().unsqueeze_dim(0)];
        for (camera, &(feature_height, feature_width)) in
            self.image_feature_sizes.iter().enumerate()
        {
            let image_position = position_embedding_2d::<B>(
                feature_height,
                feature_width,
                self.dimensions,
                &encoder_output.device(),
            ) + self
                .camera_embedding
                .weight
                .val()
                .narrow(0, camera, 1)
                .unsqueeze_dim(0);
            encoder_positions.push(image_position);
        }
        let encoder_position = Tensor::cat(encoder_positions, 1);
        for layer in &self.encoder {
            encoder_output = layer.forward(encoder_output, &encoder_position);
        }

        let [batch, _, _] = encoder_output.dims();
        let mut decoder_output = Tensor::zeros(
            [batch, self.chunk_size, self.dimensions],
            &encoder_output.device(),
        );
        let decoder_position = self.decoder_position.weight.val().unsqueeze_dim(0);
        for layer in &self.decoder {
            decoder_output = layer.forward(
                decoder_output,
                &encoder_output,
                &decoder_position,
                &encoder_position,
            );
        }
        self.decoder_norm.forward(decoder_output)
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
