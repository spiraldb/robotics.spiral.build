//! Camera discovery and live JPEG streaming.
//!
//! Probing an unknown video node can hang or take a driver down with it, so
//! discovery runs in a short-lived copy of the current executable and is
//! killed if it overruns. Any binary that calls [`ProbeOutcome::discover`] must
//! call [`CameraRuntime::run_worker_from_environment`] before parsing its own
//! arguments.
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
        self.info.stable_identifier()
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

impl ProbeResponse {
    fn probe_in_worker(info: CameraInfo) -> Result<Self> {
        let workspace = WorkerWorkspace::create("spiral-robotics-camera-probe-")?;
        let request = workspace.request();
        let response = workspace.response();

        request.write(&info)?;
        WorkerInvocation {
            mode: "probe",
            request: &request,
            response: &response,
        }
        .invoke(CAMERA_PROBE_TIMEOUT)?;
        response.read()
    }
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

/// Process-level camera initialization required by discovery and capture.
pub struct CameraRuntime {
    _private: (),
}

/// An isolated camera operation requested through the process environment.
struct CameraWorkerMode(String);

impl ProbeOutcome {
    /// Probe every camera the operating system reports, one isolated child at
    /// a time.
    pub fn discover() -> Result<Self> {
        let mut candidates = query(ApiBackend::Auto)?;
        candidates.sort_by(|left, right| left.index().cmp(right.index()));

        let mut outcome = Self::default();
        for info in candidates {
            let identifier = info.stable_identifier();
            let human_name = info.human_name();
            match ProbeResponse::probe_in_worker(info) {
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
    fn new(bytes: Vec<u8>, sequence: u64) -> Self {
        Self { bytes, sequence }
    }

    fn encode_rgb(image: &RgbImage, quality: u8, sequence: u64) -> Result<Self> {
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, quality.clamp(1, 100)).encode(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )?;
        Ok(Self::new(bytes, sequence))
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl CameraRuntime {
    /// Run an isolated camera worker when this process was launched as one.
    ///
    /// Returns `true` after handling a worker invocation and `false` for the
    /// ordinary application process. Binaries must call this before argument
    /// parsing so internal worker arguments never reach the application CLI.
    pub fn run_worker_from_environment() -> Result<bool> {
        let mode = match CameraWorkerMode::from_environment()? {
            Some(mode) => mode,
            None => return Ok(false),
        };
        mode.run()?;
        Ok(true)
    }
}

impl CameraWorkerMode {
    fn from_environment() -> Result<Option<Self>> {
        match env::var(WORKER_MODE_ENV) {
            Ok(mode) => Ok(Some(Self(mode))),
            Err(env::VarError::NotPresent) => Ok(None),
            Err(env::VarError::NotUnicode(_)) => {
                Err(format!("{WORKER_MODE_ENV} contains non-Unicode data").into())
            }
        }
    }

    /// Run the isolated camera operation this process was launched to perform.
    fn run(&self) -> Result<()> {
        let request = WorkerFile::from_environment(WORKER_REQUEST_ENV)?;
        let response = WorkerFile::from_environment(WORKER_RESPONSE_ENV)?;

        let result = (|| match self.0.as_str() {
            "probe" => {
                let info: CameraInfo = request.read()?;
                let probe = if info.supports_video_capture()? {
                    let stream_format = info.probe_format()?;
                    ProbeResponse::Camera(DetectedCamera {
                        info,
                        stream_format,
                    })
                } else {
                    ProbeResponse::UnsupportedVideoNode
                };
                response.write(&probe)
            }
            mode => Err(format!("unknown internal camera worker mode '{mode}'").into()),
        })();

        if let Err(error) = &result {
            let _ = response.write_failure(error.as_ref());
        }
        result
    }
}

/// Ask the operating system for camera permission, where it is required.
///
/// On macOS this must run before any camera is opened, and it blocks until the
/// operator answers the system prompt.
#[cfg(target_os = "macos")]
impl CameraRuntime {
    pub fn initialize_access() -> Result<()> {
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
}

/// Ask the operating system for camera permission, where it is required.
#[cfg(not(target_os = "macos"))]
impl CameraRuntime {
    pub fn initialize_access() -> Result<()> {
        Ok(())
    }
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
        let mut camera = Camera::new(self.info.open_index(), requested)?;
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
            sequence += 1;
            let frame = match frame.source_frame_format() {
                FrameFormat::MJPEG => JpegFrame::new(frame.buffer().to_vec(), sequence),
                _ => match frame.decode_image::<RgbFormat>() {
                    Ok(image) => match JpegFrame::encode_rgb(&image, config.quality, sequence) {
                        Ok(frame) => frame,
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
            sink(frame);
        }

        // Always release the device, even when the loop failed.
        let stopped = camera.stop_stream();
        result.and_then(|()| stopped.map_err(Into::into))
    }
}

trait CameraInfoExt {
    fn stable_identifier(&self) -> String;
    fn open_index(&self) -> CameraIndex;
    fn probe_format(&self) -> Result<CameraFormat>;
    fn supports_video_capture(&self) -> Result<bool>;
}

impl CameraInfoExt for CameraInfo {
    /// Identifier a camera is known by, matching the names LeRobot prints.
    fn stable_identifier(&self) -> String {
        #[cfg(target_os = "linux")]
        if let CameraIndex::Index(index) = self.index() {
            return format!("/dev/video{index}");
        }

        self.index().as_string()
    }

    fn open_index(&self) -> CameraIndex {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let device_id = self.misc();
            if !device_id.is_empty() {
                return CameraIndex::String(device_id);
            }
        }

        self.index().clone()
    }

    fn probe_format(&self) -> Result<CameraFormat> {
        let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
        let mut camera = Camera::new(self.open_index(), requested)?;

        // Opening the stream filters out V4L metadata-only nodes and busy devices.
        camera.open_stream()?;
        let stream_format = camera.camera_format();
        camera.stop_stream()?;

        Ok(stream_format)
    }

    fn supports_video_capture(&self) -> Result<bool> {
        #[cfg(target_os = "linux")]
        {
            use v4l::capability::Flags;

            let CameraIndex::Index(index) = self.index() else {
                return Ok(true);
            };
            let device = v4l::Device::new(usize::try_from(*index)?)?;
            let capabilities = device.query_caps()?.capabilities;
            Ok(capabilities.contains(Flags::VIDEO_CAPTURE | Flags::STREAMING))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(true)
        }
    }
}

struct WorkerChild(Child);

struct WorkerFile(PathBuf);

struct WorkerWorkspace(TempDir);

struct WorkerInvocation<'a> {
    mode: &'a str,
    request: &'a WorkerFile,
    response: &'a WorkerFile,
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
            .env(WORKER_REQUEST_ENV, self.request.path())
            .env(WORKER_RESPONSE_ENV, self.response.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = WorkerChild(command.spawn()?);

        let status = child.wait(timeout)?;
        if status.success() {
            return Ok(());
        }

        let detail = self
            .response
            .read_failure()
            .ok()
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| format!("camera worker exited with {status}"));
        Err(detail.into())
    }
}

impl WorkerChild {
    fn wait(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let started = Instant::now();
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }

            let elapsed = started.elapsed();
            if elapsed >= timeout {
                match self.0.kill() {
                    Ok(()) => {
                        self.0.wait()?;
                    }
                    Err(kill_error) => {
                        if let Some(status) = self.0.try_wait()? {
                            return Ok(status);
                        }
                        return Err(format!(
                            "failed to stop timed-out camera worker: {kill_error}"
                        )
                        .into());
                    }
                }
                return Err(format!("camera operation timed out after {timeout:.1?}").into());
            }

            thread::sleep(CHILD_POLL_INTERVAL.min(timeout.saturating_sub(elapsed)));
        }
    }
}

impl WorkerFile {
    fn from_environment(variable: &str) -> Result<Self> {
        env::var_os(variable)
            .map(PathBuf::from)
            .map(Self)
            .ok_or_else(|| format!("internal camera worker is missing {variable}").into())
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn read<T>(&self) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        Ok(serde_json::from_slice(&fs::read(&self.0)?)?)
    }

    fn write<T>(&self, value: &T) -> Result<()>
    where
        T: Serialize,
    {
        fs::write(&self.0, serde_json::to_vec(value)?)?;
        Ok(())
    }

    fn read_failure(&self) -> std::io::Result<String> {
        fs::read_to_string(&self.0)
    }

    fn write_failure(&self, error: &(dyn Error + Send + Sync)) -> std::io::Result<()> {
        fs::write(&self.0, error.to_string())
    }
}

impl WorkerWorkspace {
    fn create(prefix: &str) -> Result<Self> {
        let mut builder = TempDirBuilder::new();
        builder.prefix(prefix);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            builder.permissions(fs::Permissions::from_mode(0o700));
        }

        Ok(Self(builder.tempdir()?))
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn request(&self) -> WorkerFile {
        WorkerFile(self.path().join("request.json"))
    }

    fn response(&self) -> WorkerFile {
        WorkerFile(self.path().join("response.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLEEPING_CHILD_ENV: &str = "SPIRAL_ROBOTICS_TEST_SLEEPING_CAMERA_CHILD";

    #[test]
    fn creates_distinct_private_workspaces() {
        let first = WorkerWorkspace::create("spiral-robotics-camera-test-")
            .expect("first workspace should be created");
        let second = WorkerWorkspace::create("spiral-robotics-camera-test-")
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
        let mut child = WorkerChild(
            Command::new(env::current_exe().expect("test executable should exist"))
                .args(["--exact", "cameras::tests::sleeping_child_fixture"])
                .env(SLEEPING_CHILD_ENV, "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("test worker should start"),
        );

        let error = child
            .wait(Duration::from_millis(100))
            .expect_err("the sleeping worker should time out");
        assert!(error.to_string().contains("timed out"));
        assert!(
            child
                .0
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
            info.open_index(),
            CameraIndex::String("stable-id".to_owned())
        );
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn preserves_camera_index_without_native_string_support() {
        let info = CameraInfo::new("Camera", "", "stable-id", CameraIndex::Index(2));
        assert_eq!(info.open_index(), CameraIndex::Index(2));
    }

    #[test]
    fn encodes_a_decodable_jpeg() {
        let image = RgbImage::from_fn(32, 16, |x, y| {
            image::Rgb([(x * 8) as u8, (y * 16) as u8, 128])
        });

        let bytes = JpegFrame::encode_rgb(&image, DEFAULT_JPEG_QUALITY, 1)
            .expect("encode")
            .into_bytes();

        assert_eq!(&bytes[..2], &[0xff, 0xd8], "JPEG start-of-image marker");
        let decoded = image::load_from_memory(&bytes).expect("decode");
        assert_eq!((decoded.width(), decoded.height()), (32, 16));
    }
}
