//! Camera capture into timestamped H.264 access units for the recording MCAP.

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use spiral_robotics_hardware::CaptureClock;
pub(crate) use spiral_robotics_recording::CameraRecording;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::robotics::cameras::{CameraManager, Frame};

#[derive(Clone, Debug)]
pub(crate) struct CameraCaptureSpec {
    stream_id: String,
    camera_id: String,
    width: u32,
    height: u32,
    fps: u32,
}

impl CameraCaptureSpec {
    pub(crate) fn new(
        stream_id: impl Into<String>,
        camera_id: impl Into<String>,
        format: (u32, u32, u32),
    ) -> Self {
        Self {
            stream_id: stream_id.into(),
            camera_id: camera_id.into(),
            width: format.0,
            height: format.1,
            fps: format.2,
        }
    }
}

/// Hard ceiling for one active camera segment. Camera bytes are transient
/// transaction state until the episode is committed to Spiral, never a local
/// persistence layer.
const MAX_VIDEO_SEGMENT_BYTES: usize = 512 * 1024 * 1024;
const MAX_VIDEO_TRANSACTION_BYTES: usize = 1024 * 1024 * 1024;
const H264_PACKET_FILTER: &str = "dump_extra=freq=keyframe,h264_metadata=aud=insert";

pub(crate) struct VideoCapture {
    source: VideoCaptureSource,
    progress: VideoCaptureProgress,
}

struct VideoCaptureSource {
    episode_id: String,
    cameras: Arc<CameraManager>,
    specs: Vec<CameraCaptureSpec>,
    ffmpeg: String,
    codec: String,
    clock: CaptureClock,
}

struct VideoCaptureProgress {
    next_segment: u32,
    active: Option<VideoCaptureRun>,
    completed: Vec<CameraRecording>,
}

pub(crate) struct VideoCaptureRequest {
    episode_id: String,
    cameras: Vec<CameraCaptureSpec>,
    clock: CaptureClock,
}

impl VideoCaptureRequest {
    pub(crate) fn new(
        episode_id: &str,
        cameras: Vec<CameraCaptureSpec>,
        clock: CaptureClock,
    ) -> Self {
        Self {
            episode_id: episode_id.to_owned(),
            cameras,
            clock,
        }
    }

    fn has_safe_asset_key(&self) -> bool {
        !self.episode_id.is_empty()
            && self.episode_id.len() <= 64
            && self.episode_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
    }
}

struct VideoCaptureRun {
    control: watch::Sender<CaptureState>,
    tasks: VecDeque<tokio::task::JoinHandle<Result<CameraRecording, String>>>,
    failures: Vec<String>,
}

struct PreparedCameraSegment {
    spec: CameraCaptureSpec,
    frames: watch::Receiver<Option<Arc<Frame>>>,
    segment: CameraSegment,
}

impl PreparedCameraSegment {
    async fn record(
        self,
        mut control: watch::Receiver<CaptureState>,
    ) -> Result<CameraRecording, String> {
        let Self {
            spec,
            mut frames,
            segment,
        } = self;
        let encoder_fps = spec.fps.clamp(1, 120);
        let rate = encoder_fps.to_string();
        let mut child = tokio::process::Command::new(&segment.ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "-framerate",
                &rate,
                "-i",
                "pipe:0",
                "-an",
                "-c:v",
                &segment.codec,
                "-vf",
                "scale=in_range=auto:out_range=tv",
                "-pix_fmt",
                "yuv420p",
                "-color_range",
                "tv",
                "-bf",
                "0",
                "-g",
                "30",
                "-bsf:v",
                H264_PACKET_FILTER,
                "-f",
                "h264",
                "-y",
            ])
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start {:?}: {error}", segment.ffmpeg))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "FFmpeg did not open its input pipe".to_owned())?;
        let mut stderr = child.stderr.take();
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "FFmpeg did not open its output pipe".to_owned())?;
        let output_task = tokio::spawn(async move {
            let mut payload = Vec::new();
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                let read = stdout
                    .read(&mut buffer)
                    .await
                    .map_err(|error| format!("could not read FFmpeg H.264 output: {error}"))?;
                if read == 0 {
                    break;
                }
                if payload.len().saturating_add(read) > MAX_VIDEO_SEGMENT_BYTES {
                    return Err(format!(
                        "camera segment exceeded the {} MiB in-memory transaction limit",
                        MAX_VIDEO_SEGMENT_BYTES / (1024 * 1024)
                    ));
                }
                payload.extend_from_slice(&buffer[..read]);
            }
            Ok::<_, String>(payload)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            if let Some(mut stderr) = stderr.take() {
                let _ = stderr.read_to_end(&mut bytes).await;
            }
            String::from_utf8_lossy(&bytes).trim().to_owned()
        });

        let mut timestamps = Vec::new();
        let mut last_sequence = 0_u64;
        loop {
            if *control.borrow() == CaptureState::Finish {
                break;
            }
            tokio::select! {
                biased;
                changed = control.changed() => {
                    if changed.is_err() || *control.borrow() == CaptureState::Finish {
                        break;
                    }
                }
                changed = frames.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let frame = frames.borrow_and_update().clone();
                    let Some(frame) = frame else { continue };
                    if frame.sequence() <= last_sequence {
                        continue;
                    }
                    let Some(timestamp) = segment.clock.timestamp_ns(frame.captured_at()) else { continue; };
                    if timestamps.last().is_some_and(|previous| *previous >= timestamp) { continue; }
                    stdin.write_all(frame.bytes()).await.map_err(|error| {
                        format!("camera {} could not write to FFmpeg: {error}", spec.stream_id)
                    })?;
                    last_sequence = frame.sequence();
                    timestamps.push(timestamp);
                }
            }
        }
        drop(stdin);

        let status = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
            Ok(result) => result.map_err(|error| format!("FFmpeg wait failed: {error}"))?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(format!("FFmpeg did not finish camera {}", spec.stream_id));
            }
        };
        let stderr = stderr_task.await.unwrap_or_default();
        if !status.success() {
            return Err(format!(
                "FFmpeg failed for camera {}{}",
                spec.stream_id,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            ));
        }
        let payload = output_task
            .await
            .map_err(|error| format!("FFmpeg output task failed: {error}"))??;
        if timestamps.is_empty() || payload.is_empty() {
            return Err(format!(
                "camera {} delivered no video frames",
                spec.stream_id
            ));
        }
        CameraRecording::from_annex_b(
            spec.stream_id,
            (spec.width, spec.height, encoder_fps),
            (payload, timestamps),
        )
        .map(|recording| recording.with_camera_id(spec.camera_id))
    }
}

/// Immutable configuration for one encoded interval of one camera stream.
struct CameraSegment {
    ffmpeg: String,
    codec: String,
    clock: CaptureClock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureState {
    Recording,
    Finish,
}

#[derive(Clone, Debug)]
pub(crate) struct VideoEncoderSettings {
    executable: String,
    codec: String,
}

impl VideoEncoderSettings {
    pub(crate) fn new(executable: String, codec: String) -> Self {
        Self { executable, codec }
    }
}

impl VideoCapture {
    pub(crate) async fn start(
        cameras: Arc<CameraManager>,
        request: VideoCaptureRequest,
        encoder: &VideoEncoderSettings,
    ) -> Result<Option<Self>, String> {
        if request.cameras.is_empty() {
            return Ok(None);
        }
        if !request.has_safe_asset_key() {
            return Err(format!(
                "episode id {:?} cannot be used as a video asset key",
                request.episode_id
            ));
        }
        let mut capture = Self {
            source: VideoCaptureSource {
                episode_id: request.episode_id,
                cameras,
                specs: request.cameras,
                ffmpeg: encoder.executable.clone(),
                codec: encoder.codec.clone(),
                clock: request.clock,
            },
            progress: VideoCaptureProgress {
                next_segment: 0,
                active: None,
                completed: Vec::new(),
            },
        };
        capture.start_segment(0.0)?;
        Ok(Some(capture))
    }

    pub(crate) fn episode_id(&self) -> &str {
        &self.source.episode_id
    }

    pub(crate) fn completed_assets(&self) -> Vec<CameraRecording> {
        self.progress.completed.clone()
    }

    fn start_segment(&mut self, timeline_offset_seconds: f64) -> Result<(), String> {
        let cameras = self.source.cameras.clone();
        self.start_segment_with(timeline_offset_seconds, move |camera_id| {
            cameras.subscribe(camera_id)
        })
    }

    fn start_segment_with(
        &mut self,
        timeline_offset_seconds: f64,
        mut subscribe: impl FnMut(&str) -> Result<watch::Receiver<Option<Arc<Frame>>>, String>,
    ) -> Result<(), String> {
        if self.progress.active.is_some() {
            return Ok(());
        }
        if !timeline_offset_seconds.is_finite() || timeline_offset_seconds < 0.0 {
            return Err("video segment timeline offset must be finite and non-negative".to_owned());
        }
        if self.source.ffmpeg.trim().is_empty() {
            return Err("video encoder executable cannot be empty".to_owned());
        }
        if self.source.codec.trim().is_empty() {
            return Err("video encoder codec cannot be empty".to_owned());
        }
        let mut pending = Vec::with_capacity(self.source.specs.len());
        for spec in self.source.specs.iter().cloned() {
            if spec.stream_id.trim().is_empty() {
                return Err("video camera stream id cannot be empty".to_owned());
            }
            if spec.camera_id.trim().is_empty() {
                return Err(format!(
                    "video stream {:?} has an empty camera id",
                    spec.stream_id
                ));
            }
            if spec.width == 0 || spec.height == 0 || spec.fps == 0 {
                return Err(format!(
                    "video stream {:?} must have non-zero width, height, and fps",
                    spec.stream_id
                ));
            }
            pending.push(spec);
        }

        // Subscriptions may fail independently (for example, a camera can be
        // unplugged between discovery and recording). Acquire every receiver
        // before the first encoder task starts so a later camera failure can
        // never leave an earlier FFmpeg child or partial H.264 output behind.
        let mut prepared = Vec::with_capacity(pending.len());
        for spec in pending {
            let frames = subscribe(&spec.camera_id)?;
            prepared.push(PreparedCameraSegment {
                spec,
                frames,
                segment: CameraSegment {
                    ffmpeg: self.source.ffmpeg.clone(),
                    codec: self.source.codec.clone(),
                    clock: self.source.clock.clone(),
                },
            });
        }

        let (control, _) = watch::channel(CaptureState::Recording);
        let mut tasks = VecDeque::with_capacity(prepared.len());
        for prepared in prepared {
            tasks.push_back(tokio::spawn(prepared.record(control.subscribe())));
        }
        self.progress.next_segment += 1;
        self.progress.active = Some(VideoCaptureRun {
            control,
            tasks,
            failures: Vec::new(),
        });
        Ok(())
    }

    /// Stop feeding the current encoders immediately. Their encoded H.264 packets are
    /// collected after the joint-writer pause barrier has completed.
    pub(crate) fn request_pause(&self) {
        if let Some(active) = &self.progress.active {
            active.control.send_replace(CaptureState::Finish);
        }
    }

    /// Finalize the current in-memory segment for paused review.
    pub(crate) async fn finish_pause(&mut self) -> Vec<String> {
        self.finish_active().await
    }

    /// Open the next active interval under the same episode and camera set.
    pub(crate) fn resume(&mut self, timeline_offset_seconds: f64) -> Result<(), String> {
        self.start_segment(timeline_offset_seconds)
    }

    async fn finish_active(&mut self) -> Vec<String> {
        let Some(active) = self.progress.active.as_mut() else {
            return Vec::new();
        };
        active.control.send_replace(CaptureState::Finish);
        let mut total = self
            .progress
            .completed
            .iter()
            .map(|asset| asset.encoded_bytes())
            .sum::<usize>();
        while let Some(task) = active.tasks.front_mut() {
            let result = task.await;
            active.tasks.pop_front();
            match result {
                Ok(Ok(asset)) => {
                    let next = total.saturating_add(asset.encoded_bytes());
                    if next > MAX_VIDEO_TRANSACTION_BYTES {
                        active.failures.push(format!(
                            "episode video exceeded the {} MiB in-memory transaction limit",
                            MAX_VIDEO_TRANSACTION_BYTES / (1024 * 1024)
                        ));
                    } else {
                        total = next;
                        self.progress.completed.push(asset);
                    }
                }
                Ok(Err(error)) => active.failures.push(error),
                Err(error) => active
                    .failures
                    .push(format!("camera recording task failed: {error}")),
            }
        }
        self.progress
            .active
            .take()
            .expect("completed capture run")
            .failures
    }

    pub(crate) async fn finish(mut self) -> (Vec<CameraRecording>, Vec<String>) {
        let failures = self.finish_active().await;
        (self.progress.completed, failures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires FFmpeg with libx264"]
    fn each_keyframe_retains_its_own_decoder_parameters() {
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=32x32:r=5:d=1",
                "-an",
                "-c:v",
                "libx264",
                "-flags",
                "+global_header",
                "-x264-params",
                "repeat-headers=0",
                "-bf",
                "0",
                "-g",
                "2",
                "-bsf:v",
                H264_PACKET_FILTER,
                "-f",
                "h264",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let camera = CameraRecording::from_annex_b(
            "wrist".into(),
            (32, 32, 5),
            (
                output.stdout,
                (0..5).map(|index| index * 200_000_000).collect(),
            ),
        )
        .unwrap();
        let preview = CameraRecording::preview(&[camera], "wrist").unwrap();
        assert_eq!(preview.packets().len(), 5);
        assert_eq!(
            preview
                .packets()
                .iter()
                .filter(|packet| packet.keyframe())
                .count(),
            3
        );
    }

    fn capture(specs: Vec<CameraCaptureSpec>) -> VideoCapture {
        VideoCapture {
            source: VideoCaptureSource {
                episode_id: "episode_000001".to_owned(),
                cameras: Arc::new(CameraManager::new()),
                specs,
                ffmpeg: "ffmpeg".to_owned(),
                codec: "libx264".to_owned(),
                clock: CaptureClock::default(),
            },
            progress: VideoCaptureProgress {
                next_segment: 0,
                active: None,
                completed: Vec::new(),
            },
        }
    }

    #[tokio::test]
    async fn cancelled_pause_preserves_finished_packets_errors_and_pending_encoders() {
        use std::future::Future as _;
        use std::task::{Context, Waker};

        let camera = CameraRecording::from_annex_b(
            "wrist".into(),
            (32, 32, 30),
            (
                include_bytes!("../../../frontend/src/activities/episode/fixtures/seek.h264")
                    .to_vec(),
                (0..8).map(|index| index * 33_333_333).collect(),
            ),
        )
        .unwrap();
        let (ready, finished) = tokio::sync::oneshot::channel();
        let encoded = tokio::spawn(async move {
            ready.send(()).unwrap();
            Ok(camera)
        });
        finished.await.unwrap();
        let (ready, finished) = tokio::sync::oneshot::channel();
        let failed = tokio::spawn(async move {
            ready.send(()).unwrap();
            Err("first encoder failed".to_owned())
        });
        finished.await.unwrap();
        let (release, pending) = tokio::sync::oneshot::channel();
        let waiting = tokio::spawn(async move {
            pending.await.unwrap();
            Err("last encoder failed".to_owned())
        });
        let (control, _) = watch::channel(CaptureState::Recording);
        let mut capture = capture(Vec::new());
        capture.progress.active = Some(VideoCaptureRun {
            control,
            tasks: VecDeque::from([encoded, failed, waiting]),
            failures: Vec::new(),
        });
        let mut finish = Box::pin(capture.finish_pause());
        assert!(
            finish
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        // Cancel after collecting two results but while the last encoder waits.
        drop(finish);
        assert_eq!(
            CameraRecording::preview(&capture.completed_assets(), "wrist")
                .unwrap()
                .packets()
                .len(),
            8
        );
        release.send(()).unwrap();
        let failures = capture.finish_pause().await;
        assert_eq!(failures, ["first encoder failed", "last encoder failed"]);
        assert_eq!(capture.completed_assets().len(), 1);
    }

    #[test]
    fn episode_ids_are_safe_logical_keys() {
        assert!(
            VideoCaptureRequest::new("episode_000001", Vec::new(), CaptureClock::default())
                .has_safe_asset_key()
        );
        assert!(
            !VideoCaptureRequest::new("../episode", Vec::new(), CaptureClock::default())
                .has_safe_asset_key()
        );
        assert!(
            !VideoCaptureRequest::new("", Vec::new(), CaptureClock::default()).has_safe_asset_key()
        );
    }

    #[test]
    fn subscription_failure_starts_no_encoder() {
        let mut capture = capture(vec![
            CameraCaptureSpec {
                stream_id: "first".to_owned(),
                camera_id: "camera-1".to_owned(),
                width: 640,
                height: 480,
                fps: 30,
            },
            CameraCaptureSpec {
                stream_id: "second".to_owned(),
                camera_id: "camera-2".to_owned(),
                width: 640,
                height: 480,
                fps: 30,
            },
        ]);
        let (_, receiver) = watch::channel(None::<Arc<Frame>>);
        let mut subscriptions = 0;
        let error = capture
            .start_segment_with(0.0, |_| {
                subscriptions += 1;
                if subscriptions == 1 {
                    Ok(receiver.clone())
                } else {
                    Err("second camera disappeared".to_owned())
                }
            })
            .unwrap_err();
        assert!(error.contains("second camera disappeared"));
        assert!(capture.progress.active.is_none());
        assert_eq!(capture.progress.next_segment, 0);
    }
}
