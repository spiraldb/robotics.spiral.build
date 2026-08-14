use std::f32::consts::TAU;

use tch::nn::Module;
use tch::{Device, Kind, Tensor, nn};

use crate::{Error, Result};

use super::layers::{DecoderLayer, EncoderLayer};
use super::resnet::ResNet18;

#[derive(Debug)]
pub(super) struct ActModel {
    backbone: ResNet18,
    image_projection: nn::Conv2D,
    state_projection: nn::Linear,
    latent_projection: nn::Linear,
    encoder_1d_position: nn::Embedding,
    camera_embedding: Option<nn::Embedding>,
    encoder: Vec<EncoderLayer>,
    decoder_position: nn::Embedding,
    decoder: Vec<DecoderLayer>,
    decoder_norm: nn::LayerNorm,
    action_head: nn::Linear,
    image_positions: Vec<Tensor>,
    chunk_size: i64,
    dimensions: i64,
    latent_dimensions: i64,
}

impl ActModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: &nn::Path<'_>,
        state_dimensions: usize,
        action_dimensions: usize,
        image_feature_sizes: &[(usize, usize)],
        chunk_size: usize,
        dimensions: usize,
        latent_dimensions: usize,
        heads: usize,
        feedforward_dimensions: usize,
        encoder_layers: usize,
        decoder_layers: usize,
        dropout: f64,
        camera_embedding: bool,
    ) -> Result<Self> {
        let state_dimensions = dimension("state", state_dimensions)?;
        let action_dimensions = dimension("action", action_dimensions)?;
        let chunk_size = dimension("chunk", chunk_size)?;
        let dimensions = dimension("hidden", dimensions)?;
        let latent_dimensions = dimension("latent", latent_dimensions)?;
        let heads = dimension("attention heads", heads)?;
        let feedforward_dimensions = dimension("feedforward", feedforward_dimensions)?;
        let backbone = ResNet18::new(&(path / "backbone"));
        let image_projection = nn::conv2d(
            path / "encoder_img_feat_input_proj",
            512,
            dimensions,
            1,
            Default::default(),
        );
        let state_projection = nn::linear(
            path / "encoder_robot_state_input_proj",
            state_dimensions,
            dimensions,
            Default::default(),
        );
        let latent_projection = nn::linear(
            path / "encoder_latent_input_proj",
            latent_dimensions,
            dimensions,
            Default::default(),
        );
        let encoder_1d_position = nn::embedding(
            path / "encoder_1d_feature_pos_embed",
            2,
            dimensions,
            Default::default(),
        );
        let camera_embedding = camera_embedding.then(|| {
            nn::embedding(
                path / "camera_embed",
                image_feature_sizes.len() as i64,
                dimensions,
                Default::default(),
            )
        });
        let encoder = (0..encoder_layers)
            .map(|layer| {
                EncoderLayer::new(
                    &(path / "encoder" / "layers" / layer),
                    dimensions,
                    heads,
                    feedforward_dimensions,
                    dropout,
                    true,
                )
            })
            .collect();
        let decoder_position = nn::embedding(
            path / "decoder_pos_embed",
            chunk_size,
            dimensions,
            Default::default(),
        );
        let decoder = (0..decoder_layers)
            .map(|layer| {
                DecoderLayer::new(
                    &(path / "decoder" / "layers" / layer),
                    dimensions,
                    heads,
                    feedforward_dimensions,
                    dropout,
                )
            })
            .collect();
        let decoder_norm = nn::layer_norm(
            path / "decoder" / "norm",
            vec![dimensions],
            Default::default(),
        );
        let action_head = nn::linear(
            path / "action_head",
            dimensions,
            action_dimensions,
            Default::default(),
        );
        let image_positions = image_feature_sizes
            .iter()
            .map(|&(height, width)| {
                position_embedding_2d(height, width, dimensions as usize, path.device())
            })
            .collect::<Result<Vec<_>>>()?;
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
            image_positions,
            chunk_size,
            dimensions,
            latent_dimensions,
        })
    }

    #[cfg(test)]
    pub(super) fn encoder_layer_count(&self) -> usize {
        self.encoder.len()
    }

    pub fn forward(&self, state: &Tensor, images: &[Tensor]) -> Tensor {
        let batch = state.size()[0];
        let latent = Tensor::zeros(
            [batch, self.latent_dimensions],
            (Kind::Float, state.device()),
        );
        self.forward_with_latent(state, images, &latent, false)
    }

    pub fn forward_with_latent(
        &self,
        state: &Tensor,
        images: &[Tensor],
        latent: &Tensor,
        train: bool,
    ) -> Tensor {
        let hidden = self.forward_hidden_with_latent(state, images, latent, train);
        self.action_head.forward(&hidden)
    }

    fn forward_hidden_with_latent(
        &self,
        state: &Tensor,
        images: &[Tensor],
        latent: &Tensor,
        train: bool,
    ) -> Tensor {
        let mut encoder_inputs = Vec::with_capacity(images.len() + 2);
        encoder_inputs.push(self.latent_projection.forward(latent).unsqueeze(0));
        encoder_inputs.push(self.state_projection.forward(state).unsqueeze(0));
        for image in images {
            let features = self.image_projection.forward(&self.backbone.forward(image));
            encoder_inputs.push(features.flatten(2, 3).permute([2, 0, 1]));
        }
        let mut encoder_output = Tensor::cat(&encoder_inputs, 0);
        let mut encoder_positions = Vec::with_capacity(self.image_positions.len() + 1);
        encoder_positions.push(self.encoder_1d_position.ws.unsqueeze(1));
        for (camera, position) in self.image_positions.iter().enumerate() {
            let position = if let Some(camera_embedding) = &self.camera_embedding {
                position
                    + camera_embedding
                        .ws
                        .get(camera as i64)
                        .view([1, 1, self.dimensions])
            } else {
                position.shallow_clone()
            };
            encoder_positions.push(position);
        }
        let encoder_position = Tensor::cat(&encoder_positions, 0);
        for layer in &self.encoder {
            encoder_output = layer.forward(&encoder_output, &encoder_position, None, train);
        }

        let batch = encoder_output.size()[1];
        let mut decoder_output = Tensor::zeros(
            [self.chunk_size, batch, self.dimensions],
            (encoder_output.kind(), encoder_output.device()),
        );
        let decoder_position = self.decoder_position.ws.unsqueeze(1);
        for layer in &self.decoder {
            decoder_output = layer.forward(
                &decoder_output,
                &encoder_output,
                &decoder_position,
                &encoder_position,
                train,
            );
        }
        self.decoder_norm.forward(&decoder_output).transpose(0, 1)
    }
}

fn dimension(name: &str, value: usize) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| Error::Configuration(format!("ACT {name} dimension exceeds i64")))
}

/// DETR-style, normalized 2D positional embedding used by LeRobot ACT.
fn position_embedding_2d(
    height: usize,
    width: usize,
    dimensions: usize,
    device: Device,
) -> Result<Tensor> {
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
    let tokens = i64::try_from(height * width)
        .map_err(|_| Error::Configuration("ACT image token count exceeds i64".to_owned()))?;
    let dimensions = i64::try_from(dimensions)
        .map_err(|_| Error::Configuration("ACT position width exceeds i64".to_owned()))?;
    Ok(Tensor::from_slice(&values)
        .view([tokens, 1, dimensions])
        .to_device(device))
}
