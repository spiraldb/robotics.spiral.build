use burn_core as burn;
use burn_core::module::{Module, Param};
use burn_core::tensor::Tensor;
use burn_core::tensor::activation::relu;
use burn_core::tensor::backend::Backend;
use burn_nn::LinearConfig;
use burn_nn::attention::{MhaInput, MultiHeadAttention, MultiHeadAttentionConfig};
use burn_nn::{Dropout, DropoutConfig, LayerNorm, LayerNormConfig, Linear};

use crate::{Error, Result};

use super::weights::Weights;

pub(super) fn load_linear<B: Backend>(
    weights: &Weights,
    prefix: &str,
    input_dimensions: usize,
    output_dimensions: usize,
    device: &B::Device,
) -> Result<Linear<B>> {
    let weight = weights.tensor_exact::<B, 2>(
        &format!("{prefix}.weight"),
        [output_dimensions, input_dimensions],
        device,
    )?;
    let bias =
        weights.tensor_exact::<B, 1>(&format!("{prefix}.bias"), [output_dimensions], device)?;
    Ok(Linear {
        // Durable ACT checkpoints store [output, input], while Burn modules use
        // [input, output].
        weight: Param::from_tensor(weight.transpose()),
        bias: Some(Param::from_tensor(bias)),
    })
}

pub(super) fn load_layer_norm<B: Backend>(
    weights: &Weights,
    prefix: &str,
    dimensions: usize,
    device: &B::Device,
) -> Result<LayerNorm<B>> {
    let mut norm = LayerNormConfig::new(dimensions).init(device);
    norm.gamma = Param::from_tensor(weights.tensor_exact(
        &format!("{prefix}.weight"),
        [dimensions],
        device,
    )?);
    norm.beta = Some(Param::from_tensor(weights.tensor_exact(
        &format!("{prefix}.bias"),
        [dimensions],
        device,
    )?));
    Ok(norm)
}

fn load_attention<B: Backend>(
    weights: &Weights,
    prefix: &str,
    dimensions: usize,
    heads: usize,
    device: &B::Device,
) -> Result<MultiHeadAttention<B>> {
    if weights.contains(&format!("{prefix}.query.weight")) {
        let mut attention = MultiHeadAttentionConfig::new(dimensions, heads)
            .with_dropout(0.0)
            .init(device);
        attention.query = load_linear(
            weights,
            &format!("{prefix}.query"),
            dimensions,
            dimensions,
            device,
        )?;
        attention.key = load_linear(
            weights,
            &format!("{prefix}.key"),
            dimensions,
            dimensions,
            device,
        )?;
        attention.value = load_linear(
            weights,
            &format!("{prefix}.value"),
            dimensions,
            dimensions,
            device,
        )?;
        attention.output = load_linear(
            weights,
            &format!("{prefix}.output"),
            dimensions,
            dimensions,
            device,
        )?;
        return Ok(attention);
    }
    let packed_dimensions = dimensions.checked_mul(3).ok_or_else(|| {
        Error::Checkpoint("attention projection dimensions overflow usize".to_owned())
    })?;
    let packed_weight: Tensor<B, 2> = weights.tensor_exact(
        &format!("{prefix}.in_proj_weight"),
        [packed_dimensions, dimensions],
        device,
    )?;
    let packed_bias: Tensor<B, 1> = weights.tensor_exact(
        &format!("{prefix}.in_proj_bias"),
        [packed_dimensions],
        device,
    )?;

    let projection = |index: usize| Linear {
        weight: Param::from_tensor(
            packed_weight
                .clone()
                .narrow(0, index * dimensions, dimensions)
                .transpose(),
        ),
        bias: Some(Param::from_tensor(packed_bias.clone().narrow(
            0,
            index * dimensions,
            dimensions,
        ))),
    };

    let mut attention = MultiHeadAttentionConfig::new(dimensions, heads)
        .with_dropout(0.0)
        .init(device);
    attention.query = projection(0);
    attention.key = projection(1);
    attention.value = projection(2);
    attention.output = load_linear(
        weights,
        &format!("{prefix}.out_proj"),
        dimensions,
        dimensions,
        device,
    )?;
    Ok(attention)
}

#[derive(Module, Debug)]
pub(super) struct EncoderLayer<B: Backend> {
    attention: MultiHeadAttention<B>,
    linear1: Linear<B>,
    linear2: Linear<B>,
    norm1: LayerNorm<B>,
    norm2: LayerNorm<B>,
    dropout: Dropout,
}

impl<B: Backend> EncoderLayer<B> {
    pub fn init(
        dimensions: usize,
        heads: usize,
        feedforward_dimensions: usize,
        dropout: f64,
        device: &B::Device,
    ) -> Self {
        Self {
            attention: MultiHeadAttentionConfig::new(dimensions, heads)
                .with_dropout(dropout)
                .init(device),
            linear1: LinearConfig::new(dimensions, feedforward_dimensions).init(device),
            linear2: LinearConfig::new(feedforward_dimensions, dimensions).init(device),
            norm1: LayerNormConfig::new(dimensions).init(device),
            norm2: LayerNormConfig::new(dimensions).init(device),
            dropout: DropoutConfig::new(dropout).init(),
        }
    }

    pub fn load(
        weights: &Weights,
        prefix: &str,
        dimensions: usize,
        heads: usize,
        feedforward_dimensions: usize,
        device: &B::Device,
    ) -> Result<Self> {
        let native = weights.contains(&format!("{prefix}.attention.query.weight"));
        Ok(Self {
            attention: load_attention(
                weights,
                &format!(
                    "{prefix}.{}",
                    if native { "attention" } else { "self_attn" }
                ),
                dimensions,
                heads,
                device,
            )?,
            linear1: load_linear(
                weights,
                &format!("{prefix}.linear1"),
                dimensions,
                feedforward_dimensions,
                device,
            )?,
            linear2: load_linear(
                weights,
                &format!("{prefix}.linear2"),
                feedforward_dimensions,
                dimensions,
                device,
            )?,
            norm1: load_layer_norm(weights, &format!("{prefix}.norm1"), dimensions, device)?,
            norm2: load_layer_norm(weights, &format!("{prefix}.norm2"), dimensions, device)?,
            dropout: DropoutConfig::new(0.0).init(),
        })
    }

    /// Post-norm, batch-first equivalent of LeRobot's ACT encoder layer.
    pub fn forward(&self, x: Tensor<B, 3>, position: &Tensor<B, 3>) -> Tensor<B, 3> {
        let query_key = x.clone() + position.clone();
        let attention = self
            .attention
            .forward(MhaInput::new(query_key.clone(), query_key, x.clone()))
            .context;
        let x = self.norm1.forward(x + self.dropout.forward(attention));
        let feedforward = self.dropout.forward(relu(self.linear1.forward(x.clone())));
        let feedforward = self.linear2.forward(feedforward);
        self.norm2.forward(x + self.dropout.forward(feedforward))
    }
}

#[derive(Module, Debug)]
pub(super) struct DecoderLayer<B: Backend> {
    self_attention: MultiHeadAttention<B>,
    cross_attention: MultiHeadAttention<B>,
    linear1: Linear<B>,
    linear2: Linear<B>,
    norm1: LayerNorm<B>,
    norm2: LayerNorm<B>,
    norm3: LayerNorm<B>,
    dropout: Dropout,
}

impl<B: Backend> DecoderLayer<B> {
    pub fn init(
        dimensions: usize,
        heads: usize,
        feedforward_dimensions: usize,
        dropout: f64,
        device: &B::Device,
    ) -> Self {
        Self {
            self_attention: MultiHeadAttentionConfig::new(dimensions, heads)
                .with_dropout(dropout)
                .init(device),
            cross_attention: MultiHeadAttentionConfig::new(dimensions, heads)
                .with_dropout(dropout)
                .init(device),
            linear1: LinearConfig::new(dimensions, feedforward_dimensions).init(device),
            linear2: LinearConfig::new(feedforward_dimensions, dimensions).init(device),
            norm1: LayerNormConfig::new(dimensions).init(device),
            norm2: LayerNormConfig::new(dimensions).init(device),
            norm3: LayerNormConfig::new(dimensions).init(device),
            dropout: DropoutConfig::new(dropout).init(),
        }
    }

    pub fn load(
        weights: &Weights,
        prefix: &str,
        dimensions: usize,
        heads: usize,
        feedforward_dimensions: usize,
        device: &B::Device,
    ) -> Result<Self> {
        let native = weights.contains(&format!("{prefix}.self_attention.query.weight"));
        Ok(Self {
            self_attention: load_attention(
                weights,
                &format!(
                    "{prefix}.{}",
                    if native {
                        "self_attention"
                    } else {
                        "self_attn"
                    }
                ),
                dimensions,
                heads,
                device,
            )?,
            cross_attention: load_attention(
                weights,
                &format!(
                    "{prefix}.{}",
                    if native {
                        "cross_attention"
                    } else {
                        "multihead_attn"
                    }
                ),
                dimensions,
                heads,
                device,
            )?,
            linear1: load_linear(
                weights,
                &format!("{prefix}.linear1"),
                dimensions,
                feedforward_dimensions,
                device,
            )?,
            linear2: load_linear(
                weights,
                &format!("{prefix}.linear2"),
                feedforward_dimensions,
                dimensions,
                device,
            )?,
            norm1: load_layer_norm(weights, &format!("{prefix}.norm1"), dimensions, device)?,
            norm2: load_layer_norm(weights, &format!("{prefix}.norm2"), dimensions, device)?,
            norm3: load_layer_norm(weights, &format!("{prefix}.norm3"), dimensions, device)?,
            dropout: DropoutConfig::new(0.0).init(),
        })
    }

    pub fn forward(
        &self,
        x: Tensor<B, 3>,
        encoder: &Tensor<B, 3>,
        decoder_position: &Tensor<B, 3>,
        encoder_position: &Tensor<B, 3>,
    ) -> Tensor<B, 3> {
        let query_key = x.clone() + decoder_position.clone();
        let self_attention = self
            .self_attention
            .forward(MhaInput::new(query_key.clone(), query_key, x.clone()))
            .context;
        let x = self.norm1.forward(x + self.dropout.forward(self_attention));

        let cross_attention = self
            .cross_attention
            .forward(MhaInput::new(
                x.clone() + decoder_position.clone(),
                encoder.clone() + encoder_position.clone(),
                encoder.clone(),
            ))
            .context;
        let x = self
            .norm2
            .forward(x + self.dropout.forward(cross_attention));

        let feedforward = self.dropout.forward(relu(self.linear1.forward(x.clone())));
        let feedforward = self.linear2.forward(feedforward);
        self.norm3.forward(x + self.dropout.forward(feedforward))
    }
}
