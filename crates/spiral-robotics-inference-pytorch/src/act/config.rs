use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{Error, ImageInputSpecification, PolicyKind, PolicySpecification, Result};

/// Bound artifact-driven allocation and weight-loading work to the same model
/// depth Studio permits when it creates ACT checkpoints.
const MAX_TRANSFORMER_LAYERS: usize = 12;

/// Validated ACT contract. Unsupported or internally inconsistent JSON cannot
/// be represented by this type.
#[derive(Clone, Debug)]
pub struct ActConfig {
    state: VectorFeature,
    images: Vec<ImageFeature>,
    action: VectorFeature,
    pub(super) chunk_size: usize,
    action_steps: usize,
    pub(super) dim_model: usize,
    pub(super) n_heads: usize,
    pub(super) dim_feedforward: usize,
    pub(super) n_encoder_layers: usize,
    pub(super) n_decoder_layers: usize,
    pub(super) latent_dim: usize,
    image_feature_sizes: Vec<(usize, usize)>,
    pub(super) camera_embedding: bool,
}

#[derive(Clone, Debug)]
struct VectorFeature {
    name: String,
    dimensions: usize,
}

#[derive(Clone, Debug)]
struct ImageFeature {
    name: String,
    height: usize,
    width: usize,
}

#[derive(Debug, Deserialize)]
struct RawActConfig {
    #[serde(rename = "type")]
    policy_type: String,
    n_obs_steps: usize,
    normalization_mapping: BTreeMap<FeatureKind, NormalizationMode>,
    input_features: BTreeMap<String, RawFeature>,
    output_features: BTreeMap<String, RawFeature>,
    chunk_size: usize,
    n_action_steps: usize,
    vision_backbone: String,
    replace_final_stride_with_dilation: bool,
    pre_norm: bool,
    dim_model: usize,
    n_heads: usize,
    dim_feedforward: usize,
    feedforward_activation: String,
    n_encoder_layers: usize,
    n_decoder_layers: usize,
    use_vae: bool,
    latent_dim: usize,
    #[serde(default)]
    spiral_camera_embedding: bool,
    #[serde(default)]
    spiral_camera_order: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "UPPERCASE")]
enum FeatureKind {
    Visual,
    State,
    Action,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum NormalizationMode {
    MeanStd,
}

#[derive(Debug, Deserialize)]
struct RawFeature {
    #[serde(rename = "type")]
    kind: FeatureKind,
    shape: Vec<usize>,
}

impl ActConfig {
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let raw: RawActConfig = serde_json::from_slice(bytes)
            .map_err(|error| Error::Configuration(error.to_string()))?;
        Self::try_from(raw)
    }

    pub fn specification(&self) -> PolicySpecification {
        PolicySpecification {
            kind: PolicyKind::Act,
            state_dimensions: self.state.dimensions,
            image_inputs: self
                .images
                .iter()
                .map(|image| ImageInputSpecification {
                    name: image.name.clone(),
                    channels: 3,
                    height: image.height,
                    width: image.width,
                })
                .collect(),
            action_dimensions: self.action.dimensions,
            action_chunk_size: self.chunk_size,
            recommended_action_steps: self.action_steps,
        }
    }

    pub(super) fn state_name(&self) -> &str {
        &self.state.name
    }

    pub(super) fn state_dimensions(&self) -> usize {
        self.state.dimensions
    }

    pub(super) fn image_feature_sizes(&self) -> &[(usize, usize)] {
        &self.image_feature_sizes
    }

    pub(super) fn action_name(&self) -> &str {
        &self.action.name
    }

    pub(super) fn action_dimensions(&self) -> usize {
        self.action.dimensions
    }
}

impl TryFrom<RawActConfig> for ActConfig {
    type Error = Error;

    fn try_from(raw: RawActConfig) -> Result<Self> {
        if raw.policy_type != "act" {
            return Err(Error::Configuration(format!(
                "expected policy type 'act', found {:?}",
                raw.policy_type
            )));
        }
        if raw.n_obs_steps != 1 {
            return Err(Error::Configuration(format!(
                "only one observation step is supported, found {}",
                raw.n_obs_steps
            )));
        }
        if raw.vision_backbone != "resnet18" {
            return Err(Error::Configuration(format!(
                "only the resnet18 ACT backbone is supported, found {:?}",
                raw.vision_backbone
            )));
        }
        if raw.replace_final_stride_with_dilation {
            return Err(Error::Configuration(
                "dilated ACT backbones are not supported".to_owned(),
            ));
        }
        if raw.pre_norm {
            return Err(Error::Configuration(
                "only post-norm ACT checkpoints are supported".to_owned(),
            ));
        }
        if raw.feedforward_activation != "relu" {
            return Err(Error::Configuration(format!(
                "only ReLU ACT checkpoints are supported, found {:?}",
                raw.feedforward_activation
            )));
        }
        if !raw.use_vae {
            return Err(Error::Configuration(
                "the runtime requires a VAE-trained ACT checkpoint".to_owned(),
            ));
        }
        if raw.dim_model == 0
            || raw.n_heads == 0
            || !raw.dim_model.is_multiple_of(raw.n_heads)
            || !raw.dim_model.is_multiple_of(4)
        {
            return Err(Error::Configuration(format!(
                "model width {} must be positive and divisible by both {} attention heads and 4",
                raw.dim_model, raw.n_heads
            )));
        }
        if raw.dim_feedforward == 0 || raw.latent_dim == 0 {
            return Err(Error::Configuration(
                "feedforward and latent dimensions must be positive".to_owned(),
            ));
        }
        if !(1..=MAX_TRANSFORMER_LAYERS).contains(&raw.n_encoder_layers)
            || !(1..=MAX_TRANSFORMER_LAYERS).contains(&raw.n_decoder_layers)
        {
            return Err(Error::Configuration(format!(
                "ACT encoder and decoder layer counts must each be between 1 and {MAX_TRANSFORMER_LAYERS}"
            )));
        }
        if raw.chunk_size == 0 || raw.n_action_steps == 0 || raw.n_action_steps > raw.chunk_size {
            return Err(Error::Configuration(format!(
                "invalid action window: {} execution steps for chunk size {}",
                raw.n_action_steps, raw.chunk_size
            )));
        }
        for kind in [FeatureKind::Visual, FeatureKind::State, FeatureKind::Action] {
            if raw.normalization_mapping.get(&kind) != Some(&NormalizationMode::MeanStd) {
                return Err(Error::Configuration(format!(
                    "only MEAN_STD normalization is supported for {kind:?}"
                )));
            }
        }
        if raw.input_features.len() < 2 || raw.output_features.len() != 1 {
            return Err(Error::Configuration(format!(
                "the runtime requires one state, one or more image inputs, and one action output; found {} inputs and {} outputs",
                raw.input_features.len(),
                raw.output_features.len()
            )));
        }

        let (state_name, state_feature) =
            exactly_one(&raw.input_features, FeatureKind::State, "state")?;
        let (action_name, action_feature) =
            exactly_one(&raw.output_features, FeatureKind::Action, "action")?;
        let state_dimensions = vector_dimensions(state_name, state_feature, "state")?;
        let action_dimensions = vector_dimensions(action_name, action_feature, "action")?;
        let visual_features = raw
            .input_features
            .iter()
            .filter(|(_, feature)| feature.kind == FeatureKind::Visual)
            .collect::<BTreeMap<_, _>>();
        if visual_features.is_empty() {
            return Err(Error::Configuration(
                "ACT config has no image feature".to_owned(),
            ));
        }
        let image_names = if raw.spiral_camera_order.is_empty() {
            visual_features.keys().map(|name| (*name).clone()).collect()
        } else {
            let unique = raw
                .spiral_camera_order
                .iter()
                .collect::<std::collections::BTreeSet<_>>();
            if unique.len() != raw.spiral_camera_order.len()
                || raw.spiral_camera_order.len() != visual_features.len()
                || raw
                    .spiral_camera_order
                    .iter()
                    .any(|name| !visual_features.contains_key(name))
            {
                return Err(Error::Configuration(
                    "spiral_camera_order must list every visual feature exactly once".to_owned(),
                ));
            }
            raw.spiral_camera_order.clone()
        };
        let mut images = Vec::with_capacity(image_names.len());
        let mut image_feature_sizes = Vec::with_capacity(image_names.len());
        for image_name in image_names {
            let image_feature = visual_features[&image_name];
            let [3, height, width] = image_feature.shape.as_slice() else {
                return Err(Error::Configuration(format!(
                    "image feature {image_name:?} must have shape [3, height, width], found {:?}",
                    image_feature.shape
                )));
            };
            if *height == 0 || *width == 0 {
                return Err(Error::Configuration(format!(
                    "image feature {image_name:?} has zero spatial extent"
                )));
            }
            let feature_size = resnet18_feature_size(*height, *width)?;
            checked_product(&[3, *height, *width], "image element count overflows usize")?;
            checked_product(
                &[feature_size.0, feature_size.1, raw.dim_model],
                "image positional embedding size overflows usize",
            )?;
            images.push(ImageFeature {
                name: image_name,
                height: *height,
                width: *width,
            });
            image_feature_sizes.push(feature_size);
        }
        checked_product(
            &[raw.chunk_size, action_dimensions],
            "action chunk size overflows usize",
        )?;
        checked_product(
            &[raw.chunk_size, raw.dim_model],
            "decoder query embedding size overflows usize",
        )?;
        checked_product(
            &[state_dimensions, raw.dim_model],
            "state projection size overflows usize",
        )?;
        checked_product(
            &[action_dimensions, raw.dim_model],
            "action projection size overflows usize",
        )?;
        checked_product(
            &[raw.latent_dim, raw.dim_model],
            "latent projection size overflows usize",
        )?;
        checked_product(
            &[3, raw.dim_model, raw.dim_model],
            "attention projection size overflows usize",
        )?;
        checked_product(
            &[raw.dim_model, raw.dim_feedforward],
            "feedforward projection size overflows usize",
        )?;

        Ok(Self {
            state: VectorFeature {
                name: state_name.to_owned(),
                dimensions: state_dimensions,
            },
            images,
            action: VectorFeature {
                name: action_name.to_owned(),
                dimensions: action_dimensions,
            },
            chunk_size: raw.chunk_size,
            action_steps: raw.n_action_steps,
            dim_model: raw.dim_model,
            n_heads: raw.n_heads,
            dim_feedforward: raw.dim_feedforward,
            n_encoder_layers: raw.n_encoder_layers,
            n_decoder_layers: raw.n_decoder_layers,
            latent_dim: raw.latent_dim,
            image_feature_sizes,
            camera_embedding: raw.spiral_camera_embedding,
        })
    }
}

fn exactly_one<'a>(
    features: &'a BTreeMap<String, RawFeature>,
    kind: FeatureKind,
    label: &str,
) -> Result<(&'a str, &'a RawFeature)> {
    let mut matches = features
        .iter()
        .filter(|(_, feature)| feature.kind == kind)
        .map(|(name, feature)| (name.as_str(), feature));
    let first = matches
        .next()
        .ok_or_else(|| Error::Configuration(format!("ACT config has no {label} feature")))?;
    if matches.next().is_some() {
        return Err(Error::Configuration(format!(
            "ACT config has more than one {label} feature"
        )));
    }
    Ok(first)
}

fn vector_dimensions(name: &str, feature: &RawFeature, label: &str) -> Result<usize> {
    let [dimensions] = feature.shape.as_slice() else {
        return Err(Error::Configuration(format!(
            "{label} feature {name:?} must have a vector shape, found {:?}",
            feature.shape
        )));
    };
    if *dimensions == 0 {
        return Err(Error::Configuration(format!(
            "{label} feature {name:?} has zero dimensions"
        )));
    }
    Ok(*dimensions)
}

fn checked_product(values: &[usize], message: &str) -> Result<usize> {
    values.iter().try_fold(1_usize, |product, value| {
        product
            .checked_mul(*value)
            .ok_or_else(|| Error::Configuration(message.to_owned()))
    })
}

fn resnet18_feature_size(mut height: usize, mut width: usize) -> Result<(usize, usize)> {
    fn output(size: usize, kernel: usize, stride: usize, padding: usize) -> Option<usize> {
        size.checked_add(padding.checked_mul(2)?)?
            .checked_sub(kernel)?
            .checked_div(stride)?
            .checked_add(1)
    }

    for (kernel, stride, padding) in [(7, 2, 3), (3, 2, 1)]
        .into_iter()
        .chain(std::iter::repeat_n((3, 2, 1), 3))
    {
        height = output(height, kernel, stride, padding).ok_or_else(|| {
            Error::Configuration("image is too small for the ResNet-18 backbone".to_owned())
        })?;
        width = output(width, kernel, stride, padding).ok_or_else(|| {
            Error::Configuration("image is too small for the ResNet-18 backbone".to_owned())
        })?;
    }
    Ok((height, width))
}

#[cfg(test)]
mod tests {
    use super::{ActConfig, MAX_TRANSFORMER_LAYERS};

    const REFERENCE: &str = include_str!("../../fixtures/act-config.json");

    #[test]
    fn parses_reference_into_validated_contract() {
        let config = ActConfig::from_slice(REFERENCE.as_bytes()).unwrap();
        let specification = config.specification();
        assert_eq!(specification.state_dimensions, 14);
        assert_eq!(specification.action_dimensions, 14);
        assert_eq!(specification.action_chunk_size, 100);
        assert_eq!(specification.recommended_action_steps, 100);
        assert_eq!(specification.image_inputs[0].height, 480);
        assert_eq!(config.image_feature_sizes(), &[(15, 20)]);
    }

    #[test]
    fn rejects_zero_sized_images_without_panicking() {
        let mut document: serde_json::Value = serde_json::from_str(REFERENCE).unwrap();
        document["input_features"]["observation.images.top"]["shape"] =
            serde_json::json!([3, 0, 640]);
        let bytes = serde_json::to_vec(&document).unwrap();
        assert!(ActConfig::from_slice(&bytes).is_err());
    }

    #[test]
    fn malformed_shapes_never_create_a_public_contract() {
        let mut document: serde_json::Value = serde_json::from_str(REFERENCE).unwrap();
        document["input_features"]["observation.state"]["shape"] = serde_json::json!([]);
        let bytes = serde_json::to_vec(&document).unwrap();
        assert!(ActConfig::from_slice(&bytes).is_err());
    }

    #[test]
    fn transformer_depth_is_positive_and_bounded() {
        for (field, value) in [
            ("n_encoder_layers", 0),
            ("n_decoder_layers", 0),
            ("n_encoder_layers", MAX_TRANSFORMER_LAYERS + 1),
            ("n_decoder_layers", MAX_TRANSFORMER_LAYERS + 1),
        ] {
            let mut document: serde_json::Value = serde_json::from_str(REFERENCE).unwrap();
            document[field] = serde_json::json!(value);
            let bytes = serde_json::to_vec(&document).unwrap();
            assert!(
                ActConfig::from_slice(&bytes).is_err(),
                "{field}={value} must be rejected"
            );
        }
    }

    #[test]
    fn unsupported_extra_features_are_not_silently_ignored() {
        let mut document: serde_json::Value = serde_json::from_str(REFERENCE).unwrap();
        document["input_features"]["unused"] = serde_json::json!({"type": "STATE", "shape": [1]});
        let bytes = serde_json::to_vec(&document).unwrap();
        assert!(ActConfig::from_slice(&bytes).is_err());
    }

    #[test]
    fn studio_camera_order_is_explicit_for_multi_view_checkpoints() {
        let mut document: serde_json::Value = serde_json::from_str(REFERENCE).unwrap();
        document["input_features"]["observation.images.wrist"] =
            serde_json::json!({"type": "VISUAL", "shape": [3, 240, 320]});
        document["spiral_camera_embedding"] = serde_json::json!(true);
        document["spiral_camera_order"] =
            serde_json::json!(["observation.images.wrist", "observation.images.top"]);
        let config = ActConfig::from_slice(&serde_json::to_vec(&document).unwrap()).unwrap();
        let specification = config.specification();
        assert_eq!(
            specification
                .image_inputs
                .iter()
                .map(|image| image.name.as_str())
                .collect::<Vec<_>>(),
            ["observation.images.wrist", "observation.images.top"]
        );
        assert_eq!(config.image_feature_sizes(), &[(8, 10), (15, 20)]);
    }
}
