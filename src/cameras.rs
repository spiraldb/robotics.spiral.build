//! Camera discovery, still capture, and live JPEG streaming.
//!
//! Probing an unknown video node can hang or take a driver down with it, so
//! discovery and still capture run in a short-lived copy of the current
//! executable and are killed if they overrun. Any binary that calls
//! [`discover`] or [`capture_still`] must therefore hand control to
//! [`run_worker`] at startup when [`worker_mode`] returns a mode.
//!
//! Live streaming runs in the calling thread instead: by the time an operator
//! asks for a preview, the device has already survived discovery, and piping
//! frames back out of a subprocess would cost more than it protects.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, RgbImage};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, CameraInfo, FrameFormat, RequestedFormat,
    RequestedFormatType,
};
use nokhwa::{Camera, query};
use serde::{Deserialize, Serialize};
use tempfile::{Builder as TempDirBuilder, TempDir};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const CAMERA_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CAPTURE_GRACE_PERIOD: Duration = Duration::from_secs(10);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(20);
const WORKER_MODE_ENV: &str = "REROBOT_INTERNAL_CAMERA_WORKER";
const WORKER_REQUEST_ENV: &str = "REROBOT_INTERNAL_CAMERA_REQUEST";
const WORKER_RESPONSE_ENV: &str = "REROBOT_INTERNAL_CAMERA_RESPONSE";
const WORKER_IMAGE_ENV: &str = "REROBOT_INTERNAL_CAMERA_IMAGE";

/// JPEG quality used when a camera's frames have to be re-encoded.
pub const DEFAULT_JPEG_QUALITY: u8 = 80;

/// A camera that answered a probe, together with the profile it will stream.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DetectedCamera {
    pub info: CameraInfo,
    pub stream_format: CameraFormat,
}

impl DetectedCamera {
    /// Stable identifier for this camera, as stored in robot files and URLs.
    pub fn identifier(&self) -> String {
        camera_identifier(self.info.index())
    }

    /// Human-readable device name.
    pub fn name(&self) -> String {
        self.info.human_name()
    }

    /// Width of the selected stream profile, in pixels.
    pub fn width(&self) -> u32 {
        self.stream_format.resolution().width()
    }

    /// Height of the selected stream profile, in pixels.
    pub fn height(&self) -> u32 {
        self.stream_format.resolution().height()
    }

    /// Frame rate of the selected stream profile.
    pub fn fps(&self) -> u32 {
        self.stream_format.frame_rate()
    }
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

/// The result of scanning every video device the operating system reports.
#[derive(Debug, Default)]
pub struct ProbeOutcome {
    /// Cameras that opened successfully.
    pub cameras: Vec<DetectedCamera>,
    /// Nodes that exist but are not usable video-capture devices.
    pub skipped: Vec<String>,
    /// Cameras that failed to probe, each described with its reason.
    pub failures: Vec<String>,
}

/// A single encoded frame from a live camera.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JpegFrame {
    /// A complete JPEG image.
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Frames delivered on this stream so far, starting at one.
    pub sequence: u64,
}

/// Return the worker mode this process was launched with, if any.
///
/// Binaries that discover or capture from cameras must check this before
/// parsing their own arguments and hand the mode to [`run_worker`].
pub fn worker_mode() -> Result<Option<String>> {
    match env::var(WORKER_MODE_ENV) {
        Ok(mode) => Ok(Some(mode)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("{WORKER_MODE_ENV} contains non-Unicode data").into())
        }
    }
}

/// Run the isolated camera operation this process was launched to perform.
pub fn run_worker(mode: &str) -> Result<()> {
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

/// Ask the operating system for camera permission, where it is required.
///
/// On macOS this must run before any camera is opened, and it blocks until the
/// operator answers the system prompt.
#[cfg(target_os = "macos")]
pub fn initialize_camera_access() -> Result<()> {
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

/// Ask the operating system for camera permission, where it is required.
#[cfg(not(target_os = "macos"))]
pub fn initialize_camera_access() -> Result<()> {
    Ok(())
}

/// Probe every camera the operating system reports, one isolated child at a
/// time.
pub fn discover() -> Result<ProbeOutcome> {
    let mut candidates = query(ApiBackend::Auto)?;
    candidates.sort_by(|left, right| left.index().cmp(right.index()));

    let mut outcome = ProbeOutcome::default();
    for info in candidates {
        let identifier = camera_identifier(info.index());
        let human_name = info.human_name();
        match probe_in_worker(info) {
            Ok(ProbeResponse::Camera(camera)) => outcome.cameras.push(camera),
            Ok(ProbeResponse::UnsupportedVideoNode) => {
                outcome.skipped.push(format!("{identifier} ({human_name})"));
            }
            Err(error) => outcome
                .failures
                .push(format!("{identifier} ({human_name}): {error}")),
        }
    }

    Ok(outcome)
}

/// Save one recent frame from `detected` into `output_dir` as a PNG.
pub fn capture_still(
    detected: &DetectedCamera,
    output_dir: &Path,
    warmup: Duration,
    record_time: Duration,
) -> Result<PathBuf> {
    let identifier = sanitize_camera_identifier(&detected.identifier());
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

/// Stream JPEG frames from `detected` until `stop` is set.
///
/// Cameras that already deliver MJPEG are passed through untouched; anything
/// else is decoded and re-encoded at `quality`. The stop flag is read between
/// frames, so shutdown waits for at most one frame interval.
pub fn stream_jpeg_frames<F>(
    detected: &DetectedCamera,
    stop: &AtomicBool,
    quality: u8,
    mut sink: F,
) -> Result<()>
where
    F: FnMut(JpegFrame),
{
    let requested =
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::Exact(detected.stream_format));
    let mut camera = Camera::new(camera_open_index(&detected.info), requested)?;
    camera.open_stream()?;

    let mut sequence = 0_u64;
    let mut result = Ok(());
    while !stop.load(Ordering::Relaxed) {
        let frame = match camera.frame() {
            Ok(frame) => frame,
            Err(error) => {
                result = Err(error.into());
                break;
            }
        };
        let resolution = frame.resolution();
        let bytes = match frame.source_frame_format() {
            FrameFormat::MJPEG => frame.buffer().to_vec(),
            _ => match frame.decode_image::<RgbFormat>() {
                Ok(image) => match encode_jpeg(&image, quality) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        result = Err(error);
                        break;
                    }
                },
                Err(error) => {
                    result = Err(error.into());
                    break;
                }
            },
        };

        sequence += 1;
        sink(JpegFrame {
            bytes,
            width: resolution.width(),
            height: resolution.height(),
            sequence,
        });
    }

    // Always release the device, even when the loop failed.
    let stopped = camera.stop_stream();
    result.and_then(|()| stopped.map_err(Into::into))
}

/// Build a filesystem-safe suffix from a camera identifier.
pub fn sanitize_camera_identifier(identifier: &str) -> String {
    let sanitized = identifier
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

/// Identifier a camera is known by, matching the names LeRobot prints.
pub fn camera_identifier(index: &CameraIndex) -> String {
    #[cfg(target_os = "linux")]
    if let CameraIndex::Index(index) = index {
        return format!("/dev/video{index}");
    }

    index.as_string()
}

fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality.clamp(1, 100)).encode(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgb8,
    )?;
    Ok(bytes)
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

fn capture_latest_frame(
    detected: &DetectedCamera,
    warmup: Duration,
    record_time: Duration,
) -> Result<RgbImage> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const SLEEPING_CHILD_ENV: &str = "REROBOT_TEST_SLEEPING_CAMERA_CHILD";

    #[test]
    fn makes_camera_identifiers_safe_for_paths() {
        assert_eq!(
            sanitize_camera_identifier("/dev/video0\\stream:main?"),
            "_dev_video0_stream_main_"
        );
    }

    #[test]
    fn gives_empty_identifiers_a_name() {
        assert_eq!(sanitize_camera_identifier(""), "unknown");
    }

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
            .args(["--exact", "cameras::tests::sleeping_child_fixture"])
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

    #[test]
    fn encodes_a_decodable_jpeg() {
        let image = RgbImage::from_fn(32, 16, |x, y| {
            image::Rgb([(x * 8) as u8, (y * 16) as u8, 128])
        });

        let bytes = encode_jpeg(&image, DEFAULT_JPEG_QUALITY).expect("encode");

        assert_eq!(&bytes[..2], &[0xff, 0xd8], "JPEG start-of-image marker");
        let decoded = image::load_from_memory(&bytes).expect("decode");
        assert_eq!((decoded.width(), decoded.height()), (32, 16));
    }
}
