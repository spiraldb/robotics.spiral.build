//! Official torchvision ResNet-18 initialization for native ACT training.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use crate::artifact::download_verified_file;
use crate::error::IoResultExt;
use crate::{Error, Result};
use safetensors::tensor::{Dtype, TensorView, serialize};

pub const IMAGENET_RESNET18_URL: &str = "https://huggingface.co/timm/resnet18.tv_in1k/resolve/a987fd6b60f845221bfff84b6f0191273ba56ead/model.safetensors?download=true";
pub const IMAGENET_RESNET18_SHA256: &str =
    "694f673df6520a3158624e8a89af086f59923ee4cd7436fe5bc3bc71d295ad81";

/// Download an exact SafeTensors mirror of torchvision's
/// `ResNet18_Weights.IMAGENET1K_V1`, verify its bytes, and convert only its
/// feature extractor into the namespace consumed by the ACT trainer. The
/// classifier head and BatchNorm counters are omitted.
pub fn imagenet_resnet18_weights() -> Result<Vec<u8>> {
    let root = std::env::var_os("SPIRAL_ROBOTICS_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("spiral-robotics")
        })
        .join("backbones")
        .join(IMAGENET_RESNET18_SHA256);
    let source = root.join("resnet18-tv-in1k.safetensors");
    let converted = root.join("resnet18-imagenet1k-v1.safetensors");
    if converted.is_file() {
        let bytes = std::fs::read(&converted).at(&converted)?;
        if validate_converted(&bytes).is_ok() {
            return Ok(bytes);
        }
    }
    download_verified_file(IMAGENET_RESNET18_URL, &source, IMAGENET_RESNET18_SHA256)?;
    let source_bytes = std::fs::read(&source).at(&source)?;
    let bytes = convert_torchvision_resnet18(&source_bytes)?;
    let temporary = root.join("resnet18-imagenet1k-v1.safetensors.tmp");
    std::fs::write(&temporary, &bytes).at(&temporary)?;
    std::fs::rename(&temporary, &converted).at(&converted)?;
    Ok(bytes)
}

struct ConvertedTensor {
    name: String,
    shape: Vec<usize>,
    bytes: Vec<u8>,
}

fn convert_torchvision_resnet18(bytes: &[u8]) -> Result<Vec<u8>> {
    let loaded = safetensors::SafeTensors::deserialize(bytes)
        .map_err(|error| Error::Checkpoint(format!("reading torchvision ResNet-18: {error}")))?;
    let expected = expected_backbone_keys();
    let actual = loaded
        .names()
        .into_iter()
        .filter(|name| !name.starts_with("fc.") && !name.ends_with(".num_batches_tracked"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(Error::Checkpoint(format!(
            "torchvision ResNet-18 feature keys do not match IMAGENET1K_V1: missing={:?}, unexpected={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        )));
    }
    let mut tensors = Vec::with_capacity(expected.len());
    for name in expected {
        let tensor = loaded
            .tensor(&name)
            .expect("the exact key set was validated above");
        if tensor.dtype() != Dtype::F32 {
            return Err(Error::Checkpoint(format!(
                "torchvision tensor {name:?} must be F32, found {:?}",
                tensor.dtype()
            )));
        }
        tensors.push(ConvertedTensor {
            name: format!("model.backbone.{name}"),
            shape: tensor.shape().to_vec(),
            bytes: tensor.data().to_vec(),
        });
    }
    let views = tensors
        .iter()
        .map(|tensor| {
            TensorView::new(Dtype::F32, tensor.shape.clone(), &tensor.bytes)
                .map(|view| (tensor.name.as_str(), view))
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| Error::Checkpoint(format!("converting ImageNet backbone: {error}")))?;
    let metadata = HashMap::from([
        ("source".to_owned(), IMAGENET_RESNET18_URL.to_owned()),
        ("sha256".to_owned(), IMAGENET_RESNET18_SHA256.to_owned()),
        (
            "weights".to_owned(),
            "torchvision ResNet18_Weights.IMAGENET1K_V1".to_owned(),
        ),
    ]);
    let bytes = serialize(views, Some(metadata))
        .map_err(|error| Error::Checkpoint(format!("converting ImageNet backbone: {error}")))?;
    validate_converted(&bytes)?;
    Ok(bytes)
}

fn validate_converted(bytes: &[u8]) -> Result<()> {
    let tensors = safetensors::SafeTensors::deserialize(bytes)
        .map_err(|error| Error::Checkpoint(format!("converted ImageNet backbone: {error}")))?;
    let expected = expected_backbone_keys()
        .into_iter()
        .map(|name| format!("model.backbone.{name}"))
        .collect::<BTreeSet<_>>();
    let actual = tensors.names().into_iter().map(str::to_owned).collect();
    if expected != actual {
        return Err(Error::Checkpoint(
            "converted ImageNet backbone has an incomplete tensor set".to_owned(),
        ));
    }
    Ok(())
}

fn expected_backbone_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::from(["conv1.weight".to_owned()]);
    add_batch_norm_keys(&mut keys, "bn1");
    for layer in 1..=4 {
        for block in 0..=1 {
            for convolution in 1..=2 {
                keys.insert(format!("layer{layer}.{block}.conv{convolution}.weight"));
                add_batch_norm_keys(&mut keys, &format!("layer{layer}.{block}.bn{convolution}"));
            }
            if layer > 1 && block == 0 {
                keys.insert(format!("layer{layer}.{block}.downsample.0.weight"));
                add_batch_norm_keys(&mut keys, &format!("layer{layer}.{block}.downsample.1"));
            }
        }
    }
    keys
}

fn add_batch_norm_keys(keys: &mut BTreeSet<String>, prefix: &str) {
    for suffix in ["weight", "bias", "running_mean", "running_var"] {
        keys.insert(format!("{prefix}.{suffix}"));
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{convert_torchvision_resnet18, expected_backbone_keys, validate_converted};
    use crate::act::{NativeActTrainer, VisionActTrainingConfig};

    #[test]
    fn resnet18_feature_contract_has_every_frozen_batch_norm_tensor() {
        let keys = expected_backbone_keys();
        assert_eq!(keys.len(), 100);
        assert!(keys.contains("bn1.running_var"));
        assert!(keys.contains("layer4.0.downsample.1.running_mean"));
        assert!(!keys.iter().any(|key| key.starts_with("fc.")));
        assert!(!keys.iter().any(|key| key.ends_with("num_batches_tracked")));
        assert!(validate_converted(b"not safetensors").is_err());
    }

    #[test]
    #[ignore = "set SPIRAL_IMAGENET_RESNET18_SAFETENSORS to the pinned exact mirror"]
    fn official_torchvision_v1_converts_and_initializes_the_native_trainer() {
        let path = std::env::var_os("SPIRAL_IMAGENET_RESNET18_SAFETENSORS")
            .expect("SPIRAL_IMAGENET_RESNET18_SAFETENSORS must name the pinned mirror");
        let source = std::fs::read(path).unwrap();
        let bytes = convert_torchvision_resnet18(&source).unwrap();
        let tensors = safetensors::SafeTensors::deserialize(&bytes).unwrap();
        assert_eq!(tensors.len(), 100);
        let conv1 = tensors.tensor("model.backbone.conv1.weight").unwrap();
        assert_eq!(conv1.shape(), [64, 3, 7, 7]);
        let digest = format!("{:x}", Sha256::digest(conv1.data()));
        assert_eq!(
            digest,
            "3b3965901bdef70e4818b71aa892c3f1a58844e659b39e0bff24d57ef3270b20"
        );

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
            total_steps: 1,
            seed: 42,
        };
        NativeActTrainer::new(config, "cpu", Some(&bytes)).unwrap();
    }
}
