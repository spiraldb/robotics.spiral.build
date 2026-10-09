//! Camera capture into timestamped H.264 access units, written to the episode's
//! file as the encoder produces them.

use std::collections::VecDeque;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::future::BoxFuture;
use spiral_robotics_hardware::CaptureClock;
use spiral_robotics_recording::{AccessUnits, CameraStream, VideoSink, VideoUnit};
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

    /// The rate the camera's encoder runs at.
    fn encoder_fps(&self) -> u32 {
        self.fps.clamp(1, 120)
    }

    /// The camera's stream, as the episode's file records it.
    pub(crate) fn stream(&self) -> CameraStream {
        CameraStream::new(
            &self.stream_id,
            &self.camera_id,
            (self.width, self.height, self.encoder_fps()),
        )
    }
}

/// Frames waiting for a camera's encoder. A slow encoder makes capture skip
/// frames, as the preview does, rather than queue them without bound.
const ENCODER_QUEUE_FRAMES: usize = 4;

/// Turns one camera's JPEG frames into H.264 for a recording.
///
/// A computer runs FFmpeg; a microcontroller drives its hardware encoder.
pub trait EncoderDriver: Send + Sync {
    /// Encode each JPEG frame `frames` yields, at `fps`, until the channel
    /// closes, sending the Annex B H.264 stream to `output` as it is produced:
    /// no B-frames, an access unit delimiter before every frame, and every
    /// keyframe carrying its parameter sets, so a recording decodes from any
    /// keyframe. Resolves once the stream is complete.
    fn encode(
        &self,
        fps: u32,
        frames: mpsc::Receiver<Bytes>,
        output: mpsc::UnboundedSender<Bytes>,
    ) -> BoxFuture<'static, Result<(), String>>;
}

pub(crate) struct VideoCapture {
    source: VideoCaptureSource,
    progress: VideoCaptureProgress,
}

struct VideoCaptureSource {
    episode_id: String,
    cameras: Arc<CameraManager>,
    specs: Vec<CameraCaptureSpec>,
    encoder: Arc<dyn EncoderDriver>,
    clock: CaptureClock,
    sink: VideoSink,
}

struct VideoCaptureProgress {
    next_segment: u32,
    active: Option<VideoCaptureRun>,
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
    tasks: VecDeque<tokio::task::JoinHandle<Result<(), String>>>,
    failures: Vec<String>,
}

struct PreparedCameraSegment {
    spec: CameraCaptureSpec,
    frames: watch::Receiver<Option<Arc<Frame>>>,
    segment: CameraSegment,
}

impl PreparedCameraSegment {
    /// Encode the camera's frames until the capture finishes, writing each
    /// encoded frame to the episode's file as the encoder produces it.
    async fn record(self, mut control: watch::Receiver<CaptureState>) -> Result<(), String> {
        let Self {
            spec,
            mut frames,
            segment,
        } = self;
        let (encoder_input, encoder_frames) = mpsc::channel(ENCODER_QUEUE_FRAMES);
        // Unbounded, so the encoder never waits on the frames it produced
        // while this loop waits on its input: at most a few frames are in
        // flight.
        let (encoded, mut encoder_output) = mpsc::unbounded_channel();
        let encoding = tokio::spawn(segment.encoder.encode(
            spec.encoder_fps(),
            encoder_frames,
            encoded,
        ));
        let mut output = EncodedFrames::new(spec.stream_id.clone(), segment.sink);
        let mut input = Some(encoder_input);
        let mut last_sequence = 0_u64;
        let mut last_timestamp = None;
        loop {
            if input.is_some() && *control.borrow() == CaptureState::Finish {
                input = None;
            }
            tokio::select! {
                biased;
                changed = control.changed(), if input.is_some() => {
                    if changed.is_err() {
                        input = None;
                    }
                }
                changed = frames.changed(), if input.is_some() => {
                    if changed.is_err() {
                        input = None;
                        continue;
                    }
                    let frame = frames.borrow_and_update().clone();
                    let Some(frame) = frame else { continue };
                    if frame.sequence() <= last_sequence {
                        continue;
                    }
                    let Some(timestamp) = segment.clock.timestamp_ns(frame.captured_at()) else { continue; };
                    if last_timestamp.is_some_and(|previous| previous >= timestamp) { continue; }
                    let Some(sender) = input.as_ref() else { continue };
                    // A closed queue means the encoder stopped; its result says why.
                    if sender.send(frame.bytes().clone()).await.is_err() {
                        input = None;
                        continue;
                    }
                    last_sequence = frame.sequence();
                    last_timestamp = Some(timestamp);
                    output.expect(timestamp);
                }
                bytes = encoder_output.recv() => match bytes {
                    Some(bytes) => output.push(&bytes)?,
                    None => break,
                },
            }
        }
        encoding
            .await
            .map_err(|error| format!("camera {} encoder task failed: {error}", spec.stream_id))?
            .map_err(|error| format!("camera {}: {error}", spec.stream_id))?;
        output.finish()
    }
}

/// One camera segment's encoder output, cut into frames, each stamped with
/// the time its picture was captured, on its way to the episode's file.
struct EncodedFrames {
    stream: String,
    sink: VideoSink,
    units: AccessUnits,
    /// Capture times of the frames the encoder took but has not yet emitted.
    times: VecDeque<u64>,
    written: u64,
}

impl EncodedFrames {
    fn new(stream: String, sink: VideoSink) -> Self {
        Self {
            stream,
            sink,
            units: AccessUnits::default(),
            times: VecDeque::new(),
            written: 0,
        }
    }

    /// The encoder took a frame captured at `timestamp`.
    fn expect(&mut self, timestamp: u64) {
        self.times.push_back(timestamp);
    }

    fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        for unit in self.units.push(bytes) {
            self.write(unit)?;
        }
        Ok(())
    }

    /// Write the stream's last frame, once the encoder finished.
    fn finish(mut self) -> Result<(), String> {
        if let Some(unit) = std::mem::take(&mut self.units).finish() {
            self.write(unit)?;
        }
        if self.written == 0 {
            return Err(format!("camera {} delivered no video frames", self.stream));
        }
        if !self.times.is_empty() {
            return Err(format!(
                "camera {} encoded {} of {} frames",
                self.stream,
                self.written,
                self.written + self.times.len() as u64
            ));
        }
        Ok(())
    }

    fn write(&mut self, unit: Vec<u8>) -> Result<(), String> {
        let time = self.times.pop_front().ok_or_else(|| {
            format!(
                "camera {} encoded more frames than it was given",
                self.stream
            )
        })?;
        if !self.sink.send(VideoUnit::new(&self.stream, time, unit)) {
            return Err(format!(
                "camera {}: the episode's file stopped taking frames",
                self.stream
            ));
        }
        self.written += 1;
        Ok(())
    }
}

/// Immutable configuration for one encoded interval of one camera stream.
struct CameraSegment {
    encoder: Arc<dyn EncoderDriver>,
    clock: CaptureClock,
    sink: VideoSink,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureState {
    Recording,
    Finish,
}

impl VideoCapture {
    /// Start filming `request`'s cameras into the episode's file through
    /// `sink`.
    pub(crate) async fn start(
        cameras: Arc<CameraManager>,
        request: VideoCaptureRequest,
        encoder: &Arc<dyn EncoderDriver>,
        sink: VideoSink,
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
                sink,
            },
            progress: VideoCaptureProgress {
                next_segment: 0,
                active: None,
            },
        };
        capture.start_segment(0.0)?;
        Ok(Some(capture))
    }

    pub(crate) fn episode_id(&self) -> &str {
        &self.source.episode_id
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
                    sink: self.source.sink.clone(),
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

    /// Stop feeding the current encoders immediately. Their last frames are
    /// written after the joint-writer pause barrier has completed.
    pub(crate) fn request_pause(&self) {
        if let Some(active) = &self.progress.active {
            active.control.send_replace(CaptureState::Finish);
        }
    }

    /// Finish the current segment, its last frames written, for paused
    /// review.
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
        while let Some(task) = active.tasks.front_mut() {
            let result = task.await;
            active.tasks.pop_front();
            match result {
                Ok(Ok(())) => {}
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

    /// Finish filming; the failures of its last segment.
    pub(crate) async fn finish(mut self) -> Vec<String> {
        self.finish_active().await
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_common::recording::EpisodeArm;
    use spiral_robotics_recording::{EpisodeFile, EpisodeRecordingRequest, RecordingBuffer};

    use super::*;

    /// Stands in for a platform encoder in tests that never start one.
    struct UnusedEncoder;

    impl EncoderDriver for UnusedEncoder {
        fn encode(
            &self,
            _fps: u32,
            _frames: mpsc::Receiver<Bytes>,
            _output: mpsc::UnboundedSender<Bytes>,
        ) -> BoxFuture<'static, Result<(), String>> {
            Box::pin(async { Err("these tests never start an encoder".to_owned()) })
        }
    }

    /// The file of a one-arm episode filmed by `cameras`.
    fn episode_file(directory: &std::path::Path, cameras: &[CameraCaptureSpec]) -> EpisodeFile {
        let request = EpisodeRecordingRequest::new("data".to_owned(), "test".to_owned(), 60.0)
            .with_arms(
                vec![EpisodeArm::new("arm", None, vec!["joint".into()])],
                vec![],
            );
        let start = RecordingBuffer::new().begin(request).unwrap();
        EpisodeFile::create(
            directory.join("episode.mcap"),
            &start,
            cameras.iter().map(CameraCaptureSpec::stream).collect(),
        )
        .unwrap()
    }

    fn capture(specs: Vec<CameraCaptureSpec>, sink: VideoSink) -> VideoCapture {
        VideoCapture {
            source: VideoCaptureSource {
                episode_id: "episode_000001".to_owned(),
                cameras: Arc::new(CameraManager::default()),
                specs,
                encoder: Arc::new(UnusedEncoder),
                clock: CaptureClock::default(),
                sink,
            },
            progress: VideoCaptureProgress {
                next_segment: 0,
                active: None,
            },
        }
    }

    #[tokio::test]
    async fn a_cancelled_pause_keeps_every_failure_and_the_encoders_still_pending() {
        use std::future::Future as _;
        use std::task::{Context, Waker};

        let directory = tempfile::tempdir().unwrap();
        let file = episode_file(directory.path(), &[]);
        let (ready, finished) = tokio::sync::oneshot::channel();
        let encoded = tokio::spawn(async move {
            ready.send(()).unwrap();
            Ok(())
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
        let mut capture = capture(Vec::new(), file.video_sink());
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
        release.send(()).unwrap();
        let failures = capture.finish_pause().await;
        assert_eq!(failures, ["first encoder failed", "last encoder failed"]);
    }

    #[test]
    fn each_encoded_frame_carries_the_capture_time_of_the_picture_it_encodes() {
        let directory = tempfile::tempdir().unwrap();
        let spec = CameraCaptureSpec::new("wrist", "camera-1", (32, 32, 30));
        let file = episode_file(directory.path(), std::slice::from_ref(&spec));
        let stream =
            include_bytes!("../../../crates/frontend/src/activities/episode/fixtures/seek.h264");
        let frames = || {
            let mut frames = EncodedFrames::new("wrist".to_owned(), file.video_sink());
            for index in 0..8 {
                frames.expect(index * 33_333_333);
            }
            frames
        };
        let mut whole = frames();
        for piece in stream.chunks(100) {
            whole.push(piece).unwrap();
        }
        assert_eq!(whole.finish(), Ok(()));

        let mut short = frames();
        short.expect(8 * 33_333_333);
        short.push(stream).unwrap();
        assert!(
            short
                .finish()
                .unwrap_err()
                .contains("encoded 8 of 9 frames")
        );

        let mut unstamped = EncodedFrames::new("wrist".to_owned(), file.video_sink());
        unstamped.expect(0);
        assert!(
            unstamped
                .push(stream)
                .unwrap_err()
                .contains("more frames than it was given")
        );
        assert!(
            EncodedFrames::new("wrist".to_owned(), file.video_sink())
                .finish()
                .unwrap_err()
                .contains("no video frames")
        );
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
        let directory = tempfile::tempdir().unwrap();
        let file = episode_file(directory.path(), &[]);
        let mut capture = capture(
            vec![
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
            ],
            file.video_sink(),
        );
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
