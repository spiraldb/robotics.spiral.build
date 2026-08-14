use tch::nn::Module;
use tch::{Kind, Tensor, nn};

#[derive(Debug)]
struct MultiHeadAttention {
    in_proj_weight: Tensor,
    in_proj_bias: Tensor,
    out_proj: nn::Linear,
    dimensions: i64,
    heads: i64,
    dropout: f64,
}

impl MultiHeadAttention {
    fn new(
        path: &nn::Path<'_>,
        dimensions: i64,
        heads: i64,
        dropout: f64,
        reset_output_weight: bool,
    ) -> Self {
        let packed = dimensions * 3;
        let bound = (6.0 / (dimensions + packed) as f64).sqrt();
        let in_proj_weight = path.uniform("in_proj_weight", &[packed, dimensions], -bound, bound);
        let in_proj_bias = path.zeros("in_proj_bias", &[packed]);
        let mut out_proj = nn::linear(
            path / "out_proj",
            dimensions,
            dimensions,
            Default::default(),
        );
        if let Some(bias) = out_proj.bs.as_mut() {
            tch::no_grad(|| {
                let _ = bias.zero_();
            });
        }
        if reset_output_weight {
            xavier_uniform_(&mut out_proj.ws);
        }
        Self {
            in_proj_weight,
            in_proj_bias,
            out_proj,
            dimensions,
            heads,
            dropout,
        }
    }

    fn forward(
        &self,
        query: &Tensor,
        key: &Tensor,
        value: &Tensor,
        key_padding_mask: Option<&Tensor>,
        train: bool,
    ) -> Tensor {
        let q_weight = self.in_proj_weight.narrow(0, 0, self.dimensions);
        let k_weight = self
            .in_proj_weight
            .narrow(0, self.dimensions, self.dimensions);
        let v_weight = self
            .in_proj_weight
            .narrow(0, self.dimensions * 2, self.dimensions);
        let q_bias = self.in_proj_bias.narrow(0, 0, self.dimensions);
        let k_bias = self
            .in_proj_bias
            .narrow(0, self.dimensions, self.dimensions);
        let v_bias = self
            .in_proj_bias
            .narrow(0, self.dimensions * 2, self.dimensions);
        let query = query.linear(&q_weight, Some(&q_bias));
        let key = key.linear(&k_weight, Some(&k_bias));
        let value = value.linear(&v_weight, Some(&v_bias));

        let query_size = query.size();
        let key_size = key.size();
        let query_tokens = query_size[0];
        let key_tokens = key_size[0];
        let batch = query_size[1];
        let head_dimensions = self.dimensions / self.heads;
        let query = query
            .view([query_tokens, batch * self.heads, head_dimensions])
            .transpose(0, 1)
            * (1.0 / (head_dimensions as f64).sqrt());
        let key = key
            .view([key_tokens, batch * self.heads, head_dimensions])
            .transpose(0, 1);
        let value = value
            .view([key_tokens, batch * self.heads, head_dimensions])
            .transpose(0, 1);
        let mut scores = query.bmm(&key.transpose(1, 2));
        if let Some(mask) = key_padding_mask {
            let mask = mask
                .view([batch, 1, 1, key_tokens])
                .expand([batch, self.heads, 1, key_tokens], true)
                .reshape([batch * self.heads, 1, key_tokens]);
            scores = scores.masked_fill(&mask, f64::NEG_INFINITY);
        }
        let attention = scores.softmax(-1, Kind::Float).dropout(self.dropout, train);
        let context = attention
            .bmm(&value)
            .transpose(0, 1)
            .contiguous()
            .view([query_tokens * batch, self.dimensions]);
        self.out_proj
            .forward(&context)
            .view([query_tokens, batch, self.dimensions])
    }
}

#[derive(Debug)]
pub(super) struct EncoderLayer {
    attention: MultiHeadAttention,
    linear1: nn::Linear,
    linear2: nn::Linear,
    norm1: nn::LayerNorm,
    norm2: nn::LayerNorm,
    dropout: f64,
}

impl EncoderLayer {
    pub fn new(
        path: &nn::Path<'_>,
        dimensions: i64,
        heads: i64,
        feedforward_dimensions: i64,
        dropout: f64,
        reset_transformer_weights: bool,
    ) -> Self {
        let attention = MultiHeadAttention::new(
            &(path / "self_attn"),
            dimensions,
            heads,
            dropout,
            reset_transformer_weights,
        );
        let mut linear1 = nn::linear(
            path / "linear1",
            dimensions,
            feedforward_dimensions,
            Default::default(),
        );
        let mut linear2 = nn::linear(
            path / "linear2",
            feedforward_dimensions,
            dimensions,
            Default::default(),
        );
        if reset_transformer_weights {
            xavier_uniform_(&mut linear1.ws);
            xavier_uniform_(&mut linear2.ws);
        }
        Self {
            attention,
            linear1,
            linear2,
            norm1: nn::layer_norm(path / "norm1", vec![dimensions], Default::default()),
            norm2: nn::layer_norm(path / "norm2", vec![dimensions], Default::default()),
            dropout,
        }
    }

    pub fn forward(
        &self,
        input: &Tensor,
        position: &Tensor,
        padding: Option<&Tensor>,
        train: bool,
    ) -> Tensor {
        let query_key = input + position;
        let attention = self
            .attention
            .forward(&query_key, &query_key, input, padding, train);
        let output = self
            .norm1
            .forward(&(input + attention.dropout(self.dropout, train)));
        let feedforward = self.linear2.forward(
            &self
                .linear1
                .forward(&output)
                .relu()
                .dropout(self.dropout, train),
        );
        self.norm2
            .forward(&(output + feedforward.dropout(self.dropout, train)))
    }
}

#[derive(Debug)]
pub(super) struct DecoderLayer {
    self_attention: MultiHeadAttention,
    cross_attention: MultiHeadAttention,
    linear1: nn::Linear,
    linear2: nn::Linear,
    norm1: nn::LayerNorm,
    norm2: nn::LayerNorm,
    norm3: nn::LayerNorm,
    dropout: f64,
}

impl DecoderLayer {
    pub fn new(
        path: &nn::Path<'_>,
        dimensions: i64,
        heads: i64,
        feedforward_dimensions: i64,
        dropout: f64,
    ) -> Self {
        let self_attention =
            MultiHeadAttention::new(&(path / "self_attn"), dimensions, heads, dropout, true);
        let cross_attention =
            MultiHeadAttention::new(&(path / "multihead_attn"), dimensions, heads, dropout, true);
        let mut linear1 = nn::linear(
            path / "linear1",
            dimensions,
            feedforward_dimensions,
            Default::default(),
        );
        let mut linear2 = nn::linear(
            path / "linear2",
            feedforward_dimensions,
            dimensions,
            Default::default(),
        );
        xavier_uniform_(&mut linear1.ws);
        xavier_uniform_(&mut linear2.ws);
        Self {
            self_attention,
            cross_attention,
            linear1,
            linear2,
            norm1: nn::layer_norm(path / "norm1", vec![dimensions], Default::default()),
            norm2: nn::layer_norm(path / "norm2", vec![dimensions], Default::default()),
            norm3: nn::layer_norm(path / "norm3", vec![dimensions], Default::default()),
            dropout,
        }
    }

    pub fn forward(
        &self,
        input: &Tensor,
        encoder: &Tensor,
        decoder_position: &Tensor,
        encoder_position: &Tensor,
        train: bool,
    ) -> Tensor {
        let query_key = input + decoder_position;
        let self_attention = self
            .self_attention
            .forward(&query_key, &query_key, input, None, train);
        let output = self
            .norm1
            .forward(&(input + self_attention.dropout(self.dropout, train)));

        let cross_attention = self.cross_attention.forward(
            &(&output + decoder_position),
            &(encoder + encoder_position),
            encoder,
            None,
            train,
        );
        let output = self
            .norm2
            .forward(&(output + cross_attention.dropout(self.dropout, train)));
        let feedforward = self.linear2.forward(
            &self
                .linear1
                .forward(&output)
                .relu()
                .dropout(self.dropout, train),
        );
        self.norm3
            .forward(&(output + feedforward.dropout(self.dropout, train)))
    }
}

fn xavier_uniform_(tensor: &mut Tensor) {
    let size = tensor.size();
    let fan_in = size[1..].iter().product::<i64>();
    let fan_out = if size.len() > 2 {
        size[0] * size[2..].iter().product::<i64>()
    } else {
        size[0]
    };
    let bound = (6.0 / (fan_in + fan_out) as f64).sqrt();
    tch::no_grad(|| {
        let _ = tensor.uniform_(-bound, bound);
    });
}
