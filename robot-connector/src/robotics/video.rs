//! Camera capture into timestamped H.264 access units for the recording MCAP.

use std::collections::VecDeque;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::future::BoxFuture;
use spiral_robotics_hardware::CaptureClock;
pub(crate) use spiral_robotics_recording::CameraRecording;
use tokio::sync::{mpsc, watch};

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

/// Hard ceiling for one take's video. Camera bytes are transient transaction
/// state until the episode is committed to Spiral, never a local persistence
/// layer.
const MAX_VIDEO_TRANSACTION_BYTES: usize = 1024 * 1024 * 1024;
/// Frames waiting for a camera's encoder. A slow encoder makes capture skip
/// frames, as the preview does, rather than queue them without bound.
const ENCODER_QUEUE_FRAMES: usize = 4;

/// Turns one camera's JPEG frames into H.264 for a recording.
///
/// A computer runs FFmpeg; a microcontroller drives its hardware encoder.
pub trait VideoEncoder: Send + Sync {
    /// Encode each JPEG frame `frames` yields, at `fps`, until the channel
    /// closes. Answers an Annex B H.264 stream without B-frames whose every
    /// keyframe carries its parameter sets, so a recording decodes from any
    /// keyframe.
    fn encode(
        &self,
        fps: u32,
        frames: mpsc::Receiver<Bytes>,
    ) -> BoxFuture<'static, Result<Vec<u8>, String>>;
}

pub(crate) struct VideoCapture {
    source: VideoCaptureSource,
    progress: VideoCaptureProgress,
}

struct VideoCaptureSource {
    episode_id: String,
    cameras: Arc<CameraManager>,
    specs: Vec<CameraCaptureSpec>,
    encoder: Arc<dyn VideoEncoder>,
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
        let (encoder_input, encoder_frames) = mpsc::channel(ENCODER_QUEUE_FRAMES);
        let encoding = tokio::spawn(segment.encoder.encode(encoder_fps, encoder_frames));

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
                    // A closed queue means the encoder stopped; its result says why.
                    if encoder_input.send(frame.bytes().clone()).await.is_err() {
                        break;
                    }
                    last_sequence = frame.sequence();
                    timestamps.push(timestamp);
                }
            }
        }
        drop(encoder_input);

        let payload = encoding
            .await
            .map_err(|error| format!("camera {} encoder task failed: {error}", spec.stream_id))?
            .map_err(|error| format!("camera {}: {error}", spec.stream_id))?;
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
    encoder: Arc<dyn VideoEncoder>,
    clock: CaptureClock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureState {
    Recording,
    Finish,
}

impl VideoCapture {
    pub(crate) async fn start(
        cameras: Arc<CameraManager>,
        request: VideoCaptureRequest,
        encoder: &Arc<dyn VideoEncoder>,
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
                encoder: encoder.clone(),
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
        // never leave an earlier encoder or partial H.264 output behind.
        let mut prepared = Vec::with_capacity(pending.len());
        for spec in pending {
            let frames = subscribe(&spec.camera_id)?;
            prepared.push(PreparedCameraSegment {
                spec,
                frames,
                segment: CameraSegment {
                    encoder: self.source.encoder.clone(),
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

    /// Stands in for a platform encoder in tests that never start one.
    struct UnusedEncoder;

    impl VideoEncoder for UnusedEncoder {
        fn encode(
            &self,
            _fps: u32,
            _frames: mpsc::Receiver<Bytes>,
        ) -> BoxFuture<'static, Result<Vec<u8>, String>> {
            Box::pin(async { Err("these tests never start an encoder".to_owned()) })
        }
    }

    fn capture(specs: Vec<CameraCaptureSpec>) -> VideoCapture {
        VideoCapture {
            source: VideoCaptureSource {
                episode_id: "episode_000001".to_owned(),
                cameras: Arc::new(CameraManager::default()),
                specs,
                encoder: Arc::new(UnusedEncoder),
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
                include_bytes!(
                    "../../../crates/frontend/src/activities/episode/fixtures/seek.h264"
                )
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
