//! Camera discovery and live JPEG streaming.
//!
//! Probing an unknown video node can hang or take a driver down with it, so
//! discovery runs in a short-lived copy of the current executable and is
//! killed if it overruns. Any binary that calls [`discover`] must hand control to
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
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(20);
const WORKER_MODE_ENV: &str = "SPIRAL_ROBOTICS_INTERNAL_CAMERA_WORKER";
const WORKER_REQUEST_ENV: &str = "SPIRAL_ROBOTICS_INTERNAL_CAMERA_REQUEST";
const WORKER_RESPONSE_ENV: &str = "SPIRAL_ROBOTICS_INTERNAL_CAMERA_RESPONSE";

/// JPEG quality used when a camera's frames have to be re-encoded.
pub const DEFAULT_JPEG_QUALITY: u8 = 80;

/// A camera that answered a probe, together with the profile it will stream.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DetectedCamera {
    info: CameraInfo,
    stream_format: CameraFormat,
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

/// Lifetime and encoding quality for one live JPEG stream.
#[derive(Clone, Copy, Debug)]
pub struct JpegStreamConfig<'a> {
    stop: &'a AtomicBool,
    quality: u8,
}

impl<'a> JpegStreamConfig<'a> {
    pub const fn new(stop: &'a AtomicBool, quality: u8) -> Self {
        Self { stop, quality }
    }
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
    cameras: Vec<DetectedCamera>,
    /// Nodes that exist but are not usable video-capture devices.
    skipped: Vec<String>,
    /// Cameras that failed to probe, each described with its reason.
    failures: Vec<String>,
}

impl ProbeOutcome {
    /// Split discovery into the usable devices and the two diagnostic classes.
    pub fn into_parts(self) -> (Vec<DetectedCamera>, Vec<String>, Vec<String>) {
        (self.cameras, self.skipped, self.failures)
    }
}

/// A single encoded frame from a live camera.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JpegFrame {
    /// A complete JPEG image.
    bytes: Vec<u8>,
    /// Frames delivered on this stream so far, starting at one.
    sequence: u64,
}

impl JpegFrame {
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
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

impl DetectedCamera {
    /// Stream JPEG frames until the configured stop flag is set.
    ///
    /// Cameras that already deliver MJPEG are passed through untouched; anything
    /// else is decoded and re-encoded at the configured quality.
    pub fn stream_jpeg_frames<F>(&self, config: JpegStreamConfig<'_>, mut sink: F) -> Result<()>
    where
        F: FnMut(JpegFrame),
    {
        let requested =
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::Exact(self.stream_format));
        let mut camera = Camera::new(camera_open_index(&self.info), requested)?;
        camera.open_stream()?;

        let mut sequence = 0_u64;
        let mut result = Ok(());
        while !config.stop.load(Ordering::Relaxed) {
            let frame = match camera.frame() {
                Ok(frame) => frame,
                Err(error) => {
                    result = Err(error.into());
                    break;
                }
            };
            let bytes = match frame.source_frame_format() {
                FrameFormat::MJPEG => frame.buffer().to_vec(),
                _ => match frame.decode_image::<RgbFormat>() {
                    Ok(image) => match encode_jpeg(&image, config.quality) {
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
            sink(JpegFrame { bytes, sequence });
        }

        // Always release the device, even when the loop failed.
        let stopped = camera.stop_stream();
        result.and_then(|()| stopped.map_err(Into::into))
    }
}

/// Identifier a camera is known by, matching the names LeRobot prints.
fn camera_identifier(index: &CameraIndex) -> String {
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
    let workspace = private_workspace(None, "spiral-robotics-camera-probe-")?;
    let request_path = workspace.path().join("request.json");
    let response_path = workspace.path().join("response.json");

    write_json(&request_path, &info)?;
    WorkerInvocation {
        mode: "probe",
        request_path: &request_path,
        response_path: &response_path,
    }
    .invoke(CAMERA_PROBE_TIMEOUT)?;
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

struct WorkerInvocation<'a> {
    mode: &'a str,
    request_path: &'a Path,
    response_path: &'a Path,
}

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

impl WorkerInvocation<'_> {
    fn invoke(self, timeout: Duration) -> Result<()> {
        let mut command = Command::new(env::current_exe()?);
        command
            .env(WORKER_MODE_ENV, self.mode)
            .env(WORKER_REQUEST_ENV, self.request_path)
            .env(WORKER_RESPONSE_ENV, self.response_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = WorkerChild(command.spawn()?);

        let status = wait_for_child(&mut child.0, timeout)?;
        if status.success() {
            return Ok(());
        }

        let detail = fs::read_to_string(self.response_path)
            .ok()
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("camera worker exited with {status}"));
        Err(detail.into())
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const SLEEPING_CHILD_ENV: &str = "SPIRAL_ROBOTICS_TEST_SLEEPING_CAMERA_CHILD";

    #[test]
    fn creates_distinct_private_workspaces() {
        let first = private_workspace(None, "spiral-robotics-camera-test-")
            .expect("first workspace should be created");
        let second = private_workspace(None, "spiral-robotics-camera-test-")
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
