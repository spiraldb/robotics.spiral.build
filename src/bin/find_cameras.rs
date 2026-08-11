use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use nokhwa::native_api_backend;
use nokhwa::utils::ApiBackend;
use spiral_robotics::cameras::{self, DetectedCamera, ProbeOutcome};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Find native webcams and save a test image from each one.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Find native webcams and save a test image from each one"
)]
struct Args {
    /// Camera family to discover. Native webcams use OpenCV-compatible IDs.
    #[arg(value_enum, default_value_t = CameraType::Opencv)]
    camera_type: CameraType,

    /// Directory in which to save captured PNG images.
    #[arg(long, default_value = "outputs/captured_images", value_name = "PATH")]
    output_dir: PathBuf,

    /// Time to read frames from each camera before saving the latest image.
    #[arg(long, default_value_t = 2.0, value_name = "SECONDS")]
    record_time_s: f64,

    /// Time to let each camera warm up before reading frames.
    #[arg(long, default_value_t = 1, value_name = "SECONDS")]
    warmup_s: u64,

    /// Probe and list cameras without a timed capture period or image files.
    #[arg(long)]
    list_only: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CameraType {
    Opencv,
}

fn main() -> ExitCode {
    let result = match cameras::worker_mode() {
        Ok(Some(mode)) => cameras::run_worker(&mode),
        Ok(None) => run(Args::parse()),
        Err(error) => Err(error),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<()> {
    let record_time = if args.list_only {
        None
    } else {
        let duration = Duration::try_from_secs_f64(args.record_time_s)
            .map_err(|_| "--record-time-s must be a finite number greater than zero")?;
        if duration.is_zero() {
            return Err("--record-time-s must be a finite number greater than zero".into());
        }
        Some(duration)
    };

    cameras::initialize_camera_access()?;

    println!("Searching for native cameras...");
    let ProbeOutcome {
        cameras: detected,
        skipped,
        failures: probe_failures,
    } = match args.camera_type {
        CameraType::Opencv => cameras::discover()?,
    };
    for node in &skipped {
        eprintln!("info: skipping unsupported video node {node}");
    }
    for failure in &probe_failures {
        eprintln!("warning: skipping camera {failure}");
    }

    if detected.is_empty() {
        println!("No usable cameras were detected.");
        return fail_on_camera_errors(probe_failures);
    }

    print_cameras(&detected);
    if args.list_only {
        return fail_on_camera_errors(probe_failures);
    }
    let record_time = record_time.expect("capture mode validates its duration");

    fs::create_dir_all(&args.output_dir)?;
    println!("Saving test images to {}", args.output_dir.display());

    let mut failures = probe_failures;
    for camera in &detected {
        println!(
            "Opening camera {} ({})...",
            camera.identifier(),
            camera.name()
        );

        match cameras::capture_still(
            camera,
            &args.output_dir,
            Duration::from_secs(args.warmup_s),
            record_time,
        ) {
            Ok(output_path) => println!("  Saved image: {}", output_path.display()),
            Err(error) => failures.push(format!("{}: {error}", camera.identifier())),
        }
    }

    fail_on_camera_errors(failures)
}

fn fail_on_camera_errors(failures: Vec<String>) -> Result<()> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "one or more camera operations failed: {}",
            failures.join("; ")
        )
        .into())
    }
}

fn print_cameras(cameras: &[DetectedCamera]) {
    println!("\n--- Detected Cameras ---");
    for (number, camera) in cameras.iter().enumerate() {
        let info = &camera.info;

        println!("Camera #{number}:");
        println!("  Name: {}", camera.name());
        println!("  Type: OpenCV-compatible native camera");
        println!("  Id: {}", camera.identifier());
        println!(
            "  Backend api: {:?}",
            native_api_backend().unwrap_or(ApiBackend::Auto)
        );
        if !info.description().is_empty() {
            println!("  Description: {}", info.description());
        }
        let device_id = info.misc();
        if !device_id.is_empty() {
            println!("  Device id: {device_id}");
        }
        println!("  Selected stream profile:");
        println!("    Format: {}", camera.stream_format.format());
        println!("    Width: {}", camera.width());
        println!("    Height: {}", camera.height());
        println!("    Fps: {}", camera.fps());
        println!("--------------------");
    }
}
