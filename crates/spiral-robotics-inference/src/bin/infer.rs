use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use spiral_robotics_inference::act::{
    REFERENCE_REPOSITORY, REFERENCE_REVISION, reference_hub_model,
};
use spiral_robotics_inference::{
    HubModel, ImageView, InferenceInput, ModelFiles, Policy, download_from_hub,
};

#[derive(Debug, Parser)]
#[command(about = "Run native robot-policy inference without Python or PyTorch")]
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
    #[arg(long, default_value = REFERENCE_REPOSITORY)]
    repository: String,
    #[arg(long, default_value = REFERENCE_REVISION)]
    revision: String,
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
    let model = if model.repository == REFERENCE_REPOSITORY && model.revision == REFERENCE_REVISION
    {
        reference_hub_model()?
    } else {
        HubModel::pinned(&model.repository, &model.revision)?
    };
    download_from_hub(&model)
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
    #[cfg(all(not(feature = "metal"), feature = "cpu"))]
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
        identity.repository().unwrap_or("local"),
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
