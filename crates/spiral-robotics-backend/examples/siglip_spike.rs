use std::path::{Path, PathBuf};
use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::siglip;
use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Repo, RepoType};
use image::imageops::FilterType;
use tokenizers::Tokenizer;
use tokenizers::utils::padding::{PaddingDirection, PaddingParams, PaddingStrategy};
use tokenizers::utils::truncation::TruncationParams;

const REPO: &str = "google/siglip-base-patch16-224";
const REVISION: &str = "7fd15f0689c79d79e38b1c2e2e2370a7bf2761ed";

type Error = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), Error> {
    let image_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: cargo run -p spiral-robotics-backend --example siglip_spike -- <image>")?;

    let download_started = Instant::now();
    let api = ApiBuilder::new().with_progress(true).build()?;
    let repo = api.repo(Repo::with_revision(
        REPO.to_owned(),
        RepoType::Model,
        REVISION.to_owned(),
    ));
    let config = repo.get("config.json")?;
    let tokenizer = repo.get("tokenizer.json")?;
    let weights = repo.get("model.safetensors")?;
    eprintln!(
        "artifacts ready in {:.2}s: {} bytes",
        download_started.elapsed().as_secs_f64(),
        std::fs::metadata(&weights)?.len()
    );

    let _force_cpu = std::env::var("SIGLIP_SPIKE_DEVICE").is_ok_and(|value| value == "cpu");
    #[cfg(target_os = "macos")]
    if !_force_cpu {
        match Device::new_metal(0) {
            Ok(device) => match run(&device, "metal", &config, &tokenizer, &weights, &image_path) {
                Ok(()) => return Ok(()),
                Err(error) => eprintln!("Metal SigLIP failed; retrying on CPU: {error}"),
            },
            Err(error) => eprintln!("Metal unavailable; using CPU: {error}"),
        }
    }

    run(
        &Device::Cpu,
        "cpu",
        &config,
        &tokenizer,
        &weights,
        &image_path,
    )
}

fn run(
    device: &Device,
    device_name: &str,
    config_path: &Path,
    tokenizer_path: &Path,
    weights_path: &Path,
    image_path: &Path,
) -> Result<(), Error> {
    let load_started = Instant::now();
    let config: siglip::Config = serde_json::from_slice(&std::fs::read(config_path)?)?;
    let mut tokenizer = Tokenizer::from_file(tokenizer_path)?;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::Fixed(config.text_config.max_position_embeddings),
        direction: PaddingDirection::Right,
        pad_to_multiple_of: None,
        pad_id: config.text_config.pad_token_id,
        pad_type_id: 0,
        pad_token: "</s>".to_owned(),
    }));
    tokenizer.with_truncation(Some(TruncationParams {
        max_length: config.text_config.max_position_embeddings,
        ..Default::default()
    }))?;
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, device)? };
    let model = siglip::Model::new(&config, vb)?;
    device.synchronize()?;
    eprintln!(
        "{device_name} model load: {:.3}s",
        load_started.elapsed().as_secs_f64()
    );

    let prompts = ["a photo of a cat", "a photo of a dog"];
    let text_started = Instant::now();
    let encodings = tokenizer.encode_batch(prompts.to_vec(), true)?;
    let sequence = config.text_config.max_position_embeddings;
    let token_ids = encodings
        .iter()
        .flat_map(|encoding| encoding.get_ids().iter().copied())
        .collect::<Vec<_>>();
    let input_ids = Tensor::from_vec(token_ids, (prompts.len(), sequence), device)?;
    let _text_features = model.get_text_features(&input_ids)?;
    device.synchronize()?;
    eprintln!(
        "{device_name} text tower ({} concepts): {:.3}s",
        prompts.len(),
        text_started.elapsed().as_secs_f64()
    );

    let decode_started = Instant::now();
    let image = image::open(image_path)?.to_rgb8();
    let resized = image::imageops::resize(
        &image,
        config.vision_config.image_size as u32,
        config.vision_config.image_size as u32,
        FilterType::Triangle,
    );
    let pixels = resized
        .as_raw()
        .iter()
        .map(|value| (*value as f32 / 127.5) - 1.0)
        .collect::<Vec<_>>();
    let size = config.vision_config.image_size;
    let pixel_values =
        Tensor::from_vec(pixels, (1, size, size, 3), device)?.permute((0, 3, 1, 2))?;
    device.synchronize()?;
    eprintln!(
        "image decode+resize+upload: {:.3}s",
        decode_started.elapsed().as_secs_f64()
    );

    let image_started = Instant::now();
    let (_, logits) = model.forward(&pixel_values, &input_ids)?;
    let scores = candle_nn::ops::sigmoid(&logits)?.to_vec2::<f32>()?;
    device.synchronize()?;
    eprintln!(
        "{device_name} calibrated image+text forward: {:.3}s",
        image_started.elapsed().as_secs_f64()
    );
    println!("{device_name}: {scores:?}");
    Ok(())
}
