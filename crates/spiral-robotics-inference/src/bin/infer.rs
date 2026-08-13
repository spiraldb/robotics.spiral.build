use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use spiral_robotics_inference::act::{
    REFERENCE_CONFIG_URL, REFERENCE_REVISION, REFERENCE_SOURCE, REFERENCE_WEIGHTS_URL,
    reference_remote_model,
};
use spiral_robotics_inference::{
    ImageView, InferenceInput, ModelFiles, Policy, RemoteModel, download_remote_model,
};

#[derive(Debug, Parser)]
#[command(about = "Run native robot-policy inference from SafeTensors")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download and cache a pinned model artifact.
    Download(ModelArguments),
    /// Run one ACT action-chunk prediction.
    Run {
        /// Local directory containing config.json and model.safetensors.
        #[arg(long)]
        model_dir: Option<PathBuf>,
        #[command(flatten)]
        model: ModelArguments,
        /// RGB image file. A black frame is used when omitted.
        #[arg(long)]
        image: Option<PathBuf>,
        /// Comma-separated robot state. Zeros are used when omitted.
        #[arg(long, value_delimiter = ',')]
        state: Option<Vec<f32>>,
        /// Number of forward passes to run in the same warmed-up process.
        #[arg(long, default_value_t = 1)]
        runs: usize,
        /// Measure a cold first inference instead of warming the backend first.
        #[arg(long)]
        cold: bool,
        /// Write the last action chunk as JSON for inspection or parity tests.
        #[arg(long)]
        output_json: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, clap::Args)]
struct ModelArguments {
    /// Raw HTTPS URL for config.json.
    #[arg(long, default_value = REFERENCE_CONFIG_URL)]
    config_url: String,
    /// Raw HTTPS URL for model.safetensors.
    #[arg(long, default_value = REFERENCE_WEIGHTS_URL)]
    weights_url: String,
    /// Human-readable source recorded in the model identity.
    #[arg(long, default_value = REFERENCE_SOURCE)]
    source: String,
    #[arg(long, default_value = REFERENCE_REVISION)]
    revision: String,
    /// Expected SHA-256 of config.json. Must be paired with --weights-sha256.
    #[arg(long, requires = "weights_sha256")]
    config_sha256: Option<String>,
    /// Expected SHA-256 of model.safetensors. Must be paired with --config-sha256.
    #[arg(long, requires = "config_sha256")]
    weights_sha256: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Arguments::parse().command {
        Command::Download(model) => {
            let files = download(&model)?;
            println!("config: {}", files.config_path().display());
            println!("weights: {}", files.weights_path().display());
        }
        Command::Run {
            model_dir,
            model,
            image,
            state,
            runs,
            cold,
            output_json,
        } => run(
            model_dir.as_deref(),
            &model,
            image.as_deref(),
            state,
            runs,
            cold,
            output_json.as_deref(),
        )?,
    }
    Ok(())
}

fn download(model: &ModelArguments) -> spiral_robotics_inference::Result<ModelFiles> {
    let model = if model.config_url == REFERENCE_CONFIG_URL
        && model.weights_url == REFERENCE_WEIGHTS_URL
        && model.revision == REFERENCE_REVISION
    {
        reference_remote_model()?
    } else {
        let remote = RemoteModel::pinned(
            &model.source,
            &model.revision,
            &model.config_url,
            &model.weights_url,
        )?;
        let (Some(config), Some(weights)) = (&model.config_sha256, &model.weights_sha256) else {
            return Err(spiral_robotics_inference::Error::Artifact(
                "custom remote models require both config and weights SHA-256 checksums".to_owned(),
            ));
        };
        remote.with_checksums(config, weights)?
    };
    download_remote_model(&model)
}

fn run(
    model_dir: Option<&Path>,
    model: &ModelArguments,
    image_path: Option<&Path>,
    state: Option<Vec<f32>>,
    runs: usize,
    cold: bool,
    output_json: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    if runs == 0 {
        return Err("--runs must be at least one".into());
    }
    let files = match model_dir {
        Some(directory) => ModelFiles::from_directory(directory)?,
        None => download(model)?,
    };

    #[cfg(feature = "metal")]
    let mut policy = spiral_robotics_inference::act::load_metal(&files)?;
    #[cfg(not(feature = "metal"))]
    let mut policy = spiral_robotics_inference::act::load_cpu(&files)?;

    if !cold {
        let warm_up = policy.warm_up()?;
        println!("warm-up: {:.2?}", warm_up.total);
    }

    let specification = policy.specification().clone();
    let image_spec = specification
        .image_inputs
        .first()
        .ok_or("policy has no image input specification")?;
    let pixels = match image_path {
        Some(path) => load_rgb(path, image_spec.width, image_spec.height)?,
        None => vec![0; image_spec.width * image_spec.height * 3],
    };
    let state = state.unwrap_or_else(|| vec![0.0; specification.state_dimensions]);
    let image = ImageView::packed_rgb8(
        &image_spec.name,
        image_spec.width,
        image_spec.height,
        &pixels,
    )?;
    let images = [image];
    let mut measurements = Vec::with_capacity(runs);
    let mut output = None;
    for _ in 0..runs {
        let next = policy.infer(InferenceInput {
            state: &state,
            images: &images,
        })?;
        measurements.push(next.stats);
        output = Some(next);
    }
    let output = output.ok_or("--runs must be at least one")?;

    let identity = policy.identity();
    println!(
        "model: {}@{}",
        identity.source().unwrap_or("local"),
        identity.revision().unwrap_or("unversioned")
    );
    println!(
        "weights SHA-256: {}",
        identity.weights_sha256().unwrap_or("unavailable")
    );
    println!(
        "output: {} steps × {} action dimensions",
        output.steps(),
        output.action_dimensions()
    );
    println!("first action: {:?}", output.action(0).unwrap_or_default());
    for (index, stats) in measurements.iter().enumerate() {
        println!(
            "latency[{}]: preprocess={:.2?}, model={:.2?}, postprocess={:.2?}, total={:.2?}",
            index + 1,
            stats.preprocessing,
            stats.model,
            stats.postprocessing,
            stats.total
        );
    }
    if let Some(path) = output_json {
        let document = serde_json::json!({
            "steps": output.steps(),
            "action_dimensions": output.action_dimensions(),
            "values": output.as_slice(),
        });
        std::fs::write(path, serde_json::to_vec_pretty(&document)?)?;
        println!("actions: {}", path.display());
    }
    Ok(())
}

fn load_rgb(
    path: &Path,
    expected_width: usize,
    expected_height: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let image = image::ImageReader::open(path)?.decode()?.into_rgb8();
    let (width, height) = image.dimensions();
    if width as usize != expected_width || height as usize != expected_height {
        return Err(format!(
            "{} is {width}x{height}; model expects {expected_width}x{expected_height}",
            path.display()
        )
        .into());
    }
    Ok(image.into_raw())
}
