use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, CameraInfo, RequestedFormat, RequestedFormatType,
};
use nokhwa::{Camera, native_api_backend, query};
use rerobot::cameras::sanitize_camera_identifier;
use serde::{Deserialize, Serialize};
use tempfile::{Builder as TempDirBuilder, TempDir};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const CAMERA_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CAPTURE_GRACE_PERIOD: Duration = Duration::from_secs(10);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(20);
const WORKER_MODE_ENV: &str = "REROBOT_INTERNAL_CAMERA_WORKER";
const WORKER_REQUEST_ENV: &str = "REROBOT_INTERNAL_CAMERA_REQUEST";
const WORKER_RESPONSE_ENV: &str = "REROBOT_INTERNAL_CAMERA_RESPONSE";
const WORKER_IMAGE_ENV: &str = "REROBOT_INTERNAL_CAMERA_IMAGE";

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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DetectedCamera {
    info: CameraInfo,
    stream_format: CameraFormat,
}

#[derive(Debug, Deserialize, Serialize)]
struct CaptureRequest {
    camera: DetectedCamera,
    warmup_secs: u64,
    record_time_secs: u64,
    record_time_nanos: u32,
}

#[derive(Debug, Deserialize, Serialize)]
enum ProbeResponse {
    Camera(DetectedCamera),
    UnsupportedVideoNode,
}

#[derive(Debug)]
struct ProbeOutcome {
    cameras: Vec<DetectedCamera>,
    failures: Vec<String>,
}

fn main() -> ExitCode {
    let result = match env::var(WORKER_MODE_ENV) {
        Ok(mode) => run_internal_worker(&mode),
        Err(env::VarError::NotPresent) => run(Args::parse()),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("{WORKER_MODE_ENV} contains non-Unicode data").into())
        }
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

    initialize_camera_access()?;

    println!("Searching for native cameras...");
    let candidates = match args.camera_type {
        CameraType::Opencv => query(ApiBackend::Auto)?,
    };
    let ProbeOutcome {
        cameras,
        failures: probe_failures,
    } = probe_cameras(candidates);

    if cameras.is_empty() {
        println!("No usable cameras were detected.");
        return fail_on_camera_errors(probe_failures);
    }

    print_cameras(&cameras);
    if args.list_only {
        return fail_on_camera_errors(probe_failures);
    }
    let record_time = record_time.expect("capture mode validates its duration");

    fs::create_dir_all(&args.output_dir)?;
    println!("Saving test images to {}", args.output_dir.display());

    let mut failures = probe_failures;
    for camera in &cameras {
        let identifier = camera_identifier(camera.info.index());
        println!(
            "Opening camera {identifier} ({})...",
            camera.info.human_name()
        );

        match capture_in_worker(
            camera,
            &args.output_dir,
            Duration::from_secs(args.warmup_s),
            record_time,
        ) {
            Ok(output_path) => println!("  Saved image: {}", output_path.display()),
            Err(error) => failures.push(format!("{identifier}: {error}")),
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

#[cfg(target_os = "macos")]
fn initialize_camera_access() -> Result<()> {
    use std::sync::mpsc;

    let (sender, receiver) = mpsc::channel();
    nokhwa::nokhwa_initialize(move |granted| {
        let _ = sender.send(granted);
    });

    match receiver.recv_timeout(Duration::from_secs(120)) {
        Ok(true) => Ok(()),
        Ok(false) => Err(
            "camera access was denied; allow access in System Settings > Privacy & Security > Camera"
                .into(),
        ),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("timed out after 120 seconds while requesting camera access".into())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("camera permission request ended without a result".into())
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn initialize_camera_access() -> Result<()> {
    Ok(())
}

fn probe_cameras(mut candidates: Vec<CameraInfo>) -> ProbeOutcome {
    candidates.sort_by(|left, right| left.index().cmp(right.index()));

    let mut cameras = Vec::new();
    let mut failures = Vec::new();
    for info in candidates {
        let identifier = camera_identifier(info.index());
        let human_name = info.human_name();
        match probe_in_worker(info) {
            Ok(ProbeResponse::Camera(camera)) => cameras.push(camera),
            Ok(ProbeResponse::UnsupportedVideoNode) => {
                eprintln!("info: skipping unsupported video node {identifier} ({human_name})");
            }
            Err(error) => {
                let failure = format!("{identifier} ({human_name}): {error}");
                eprintln!("warning: skipping camera {failure}");
                failures.push(failure);
            }
        }
    }

    ProbeOutcome { cameras, failures }
}

fn probe_in_worker(info: CameraInfo) -> Result<ProbeResponse> {
    let workspace = private_workspace(None, "rerobot-camera-probe-")?;
    let request_path = workspace.path().join("request.json");
    let response_path = workspace.path().join("response.json");

    write_json(&request_path, &info)?;
    invoke_worker(
        "probe",
        &request_path,
        &response_path,
        CAMERA_PROBE_TIMEOUT,
        None,
    )?;
    read_json(&response_path)
}

fn probe_camera(info: &CameraInfo) -> Result<CameraFormat> {
    let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
    let mut camera = Camera::new(camera_open_index(info), requested)?;

    // Opening the stream filters out V4L metadata-only nodes and busy devices.
    camera.open_stream()?;
    let stream_format = camera.camera_format();
    camera.stop_stream()?;

    Ok(stream_format)
}

#[cfg(target_os = "linux")]
fn is_supported_video_capture_node(info: &CameraInfo) -> Result<bool> {
    use v4l::capability::Flags;

    let CameraIndex::Index(index) = info.index() else {
        return Ok(true);
    };
    let device = v4l::Device::new(usize::try_from(*index)?)?;
    let capabilities = device.query_caps()?.capabilities;
    Ok(capabilities.contains(Flags::VIDEO_CAPTURE | Flags::STREAMING))
}

#[cfg(not(target_os = "linux"))]
fn is_supported_video_capture_node(_info: &CameraInfo) -> Result<bool> {
    Ok(true)
}

fn print_cameras(cameras: &[DetectedCamera]) {
    println!("\n--- Detected Cameras ---");
    for (number, camera) in cameras.iter().enumerate() {
        let info = &camera.info;
        let format = camera.stream_format;
        let resolution = format.resolution();

        println!("Camera #{number}:");
        println!("  Name: {}", info.human_name());
        println!("  Type: OpenCV-compatible native camera");
        println!("  Id: {}", camera_identifier(info.index()));
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
        println!("    Format: {}", format.format());
        println!("    Width: {}", resolution.width());
        println!("    Height: {}", resolution.height());
        println!("    Fps: {}", format.frame_rate());
        println!("--------------------");
    }
}

fn capture_in_worker(
    detected: &DetectedCamera,
    output_dir: &Path,
    warmup: Duration,
    record_time: Duration,
) -> Result<PathBuf> {
    let identifier = sanitize_camera_identifier(&camera_identifier(detected.info.index()));
    let output_path = output_dir.join(format!("opencv_{identifier}.png"));
    let workspace = private_workspace(Some(output_dir), ".rerobot-camera-capture-")?;
    let image_path = workspace.path().join("image.png");
    let request_path = workspace.path().join("request.json");
    let response_path = workspace.path().join("response.txt");
    let request = CaptureRequest {
        camera: detected.clone(),
        warmup_secs: warmup.as_secs(),
        record_time_secs: record_time.as_secs(),
        record_time_nanos: record_time.subsec_nanos(),
    };
    let timeout = warmup
        .saturating_add(record_time)
        .saturating_add(CAPTURE_GRACE_PERIOD);

    write_json(&request_path, &request)?;
    invoke_worker(
        "capture",
        &request_path,
        &response_path,
        timeout,
        Some(&image_path),
    )?;
    if !image_path.is_file() {
        return Err("camera worker exited without producing an image".into());
    }
    promote_capture(&image_path, &output_path)?;
    Ok(output_path)
}

fn capture_latest_frame(
    detected: &DetectedCamera,
    warmup: Duration,
    record_time: Duration,
) -> Result<image::RgbImage> {
    let requested =
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::Exact(detected.stream_format));
    let mut camera = Camera::new(camera_open_index(&detected.info), requested)?;
    camera.open_stream()?;

    if !warmup.is_zero() {
        thread::sleep(warmup);
    }

    let started = Instant::now();
    let mut latest = None;
    while latest.is_none() || started.elapsed() < record_time {
        let frame = camera.frame()?;
        latest = Some(frame.decode_image::<RgbFormat>()?);
    }

    camera.stop_stream()?;
    Ok(latest.expect("the capture loop always reads at least one frame"))
}

fn camera_open_index(info: &CameraInfo) -> CameraIndex {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let device_id = info.misc();
        if !device_id.is_empty() {
            return CameraIndex::String(device_id);
        }
    }

    info.index().clone()
}

struct WorkerChild(Child);

impl Drop for WorkerChild {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                if self.0.kill().is_ok() {
                    let _ = self.0.wait();
                } else {
                    let _ = self.0.try_wait();
                }
            }
        }
    }
}

fn invoke_worker(
    mode: &str,
    request_path: &Path,
    response_path: &Path,
    timeout: Duration,
    image_path: Option<&Path>,
) -> Result<()> {
    let mut command = Command::new(env::current_exe()?);
    command
        .env(WORKER_MODE_ENV, mode)
        .env(WORKER_REQUEST_ENV, request_path)
        .env(WORKER_RESPONSE_ENV, response_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(image_path) = image_path {
        command.env(WORKER_IMAGE_ENV, image_path);
    }
    let mut child = WorkerChild(command.spawn()?);

    let status = wait_for_child(&mut child.0, timeout)?;
    if status.success() {
        return Ok(());
    }

    let detail = fs::read_to_string(response_path)
        .ok()
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| format!("camera worker exited with {status}"));
    Err(detail.into())
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            match child.kill() {
                Ok(()) => {
                    child.wait()?;
                }
                Err(kill_error) => {
                    if let Some(status) = child.try_wait()? {
                        return Ok(status);
                    }
                    return Err(
                        format!("failed to stop timed-out camera worker: {kill_error}").into(),
                    );
                }
            }
            return Err(format!("camera operation timed out after {timeout:.1?}").into());
        }

        thread::sleep(CHILD_POLL_INTERVAL.min(timeout.saturating_sub(elapsed)));
    }
}

fn run_internal_worker(mode: &str) -> Result<()> {
    let request_path = required_worker_path(WORKER_REQUEST_ENV)?;
    let response_path = required_worker_path(WORKER_RESPONSE_ENV)?;

    let result = (|| match mode {
        "probe" => {
            let info: CameraInfo = read_json(&request_path)?;
            let response = if is_supported_video_capture_node(&info)? {
                let stream_format = probe_camera(&info)?;
                ProbeResponse::Camera(DetectedCamera {
                    info,
                    stream_format,
                })
            } else {
                ProbeResponse::UnsupportedVideoNode
            };
            write_json(&response_path, &response)
        }
        "capture" => {
            let request: CaptureRequest = read_json(&request_path)?;
            let image_path = required_worker_path(WORKER_IMAGE_ENV)?;
            let warmup = Duration::from_secs(request.warmup_secs);
            let record_time = Duration::new(request.record_time_secs, request.record_time_nanos);
            let image = capture_latest_frame(&request.camera, warmup, record_time)?;
            image.save(image_path)?;
            Ok(())
        }
        _ => Err(format!("unknown internal camera worker mode '{mode}'").into()),
    })();

    if let Err(error) = &result {
        let _ = fs::write(response_path, error.to_string());
    }
    result
}

fn required_worker_path(variable: &str) -> Result<PathBuf> {
    env::var_os(variable)
        .map(PathBuf::from)
        .ok_or_else(|| format!("internal camera worker is missing {variable}").into())
}

fn read_json<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn write_json<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    fs::write(path, serde_json::to_vec(value)?)?;
    Ok(())
}

fn private_workspace(directory: Option<&Path>, prefix: &str) -> Result<TempDir> {
    let mut builder = TempDirBuilder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        builder.permissions(fs::Permissions::from_mode(0o700));
    }

    match directory {
        Some(directory) => Ok(builder.tempdir_in(directory)?),
        None => Ok(builder.tempdir()?),
    }
}

#[cfg(not(target_os = "windows"))]
fn promote_capture(temporary: &Path, output: &Path) -> Result<()> {
    fs::rename(temporary, output)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn promote_capture(temporary: &Path, output: &Path) -> Result<()> {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let temporary: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let output: Vec<u16> = output.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: Both paths are owned, NUL-terminated UTF-16 buffers that remain
    // alive for the duration of this call.
    let moved = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            output.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

fn camera_identifier(index: &CameraIndex) -> String {
    #[cfg(target_os = "linux")]
    if let CameraIndex::Index(index) = index {
        return format!("/dev/video{index}");
    }

    index.as_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLEEPING_CHILD_ENV: &str = "REROBOT_TEST_SLEEPING_CAMERA_CHILD";

    #[test]
    fn creates_distinct_private_workspaces() {
        let first = private_workspace(None, "rerobot-camera-test-")
            .expect("first workspace should be created");
        let second = private_workspace(None, "rerobot-camera-test-")
            .expect("second workspace should be created");
        assert_ne!(first.path(), second.path());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = first
                .path()
                .metadata()
                .expect("workspace metadata should be readable")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[test]
    fn stops_and_reaps_a_timed_out_worker() {
        let mut child = Command::new(env::current_exe().expect("test executable should exist"))
            .args(["--exact", "tests::sleeping_child_fixture"])
            .env(SLEEPING_CHILD_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("test worker should start");

        let error = wait_for_child(&mut child, Duration::from_millis(100))
            .expect_err("the sleeping worker should time out");
        assert!(error.to_string().contains("timed out"));
        assert!(
            child
                .try_wait()
                .expect("reaped child status should be available")
                .is_some()
        );
    }

    #[test]
    fn sleeping_child_fixture() {
        if env::var_os(SLEEPING_CHILD_ENV).is_some() {
            thread::sleep(Duration::from_secs(5));
        }
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn opens_camera_by_stable_native_identifier() {
        let info = CameraInfo::new("Camera", "", "stable-id", CameraIndex::Index(2));
        assert_eq!(
            camera_open_index(&info),
            CameraIndex::String("stable-id".to_owned())
        );
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn preserves_camera_index_without_native_string_support() {
        let info = CameraInfo::new("Camera", "", "stable-id", CameraIndex::Index(2));
        assert_eq!(camera_open_index(&info), CameraIndex::Index(2));
    }
}
