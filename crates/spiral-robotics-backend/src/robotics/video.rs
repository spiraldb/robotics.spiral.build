//! Server-side camera recording into browser-playable MP4 assets.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

pub(crate) use spiral_robotics_storage::video::{VideoAsset, VideoAssetMetadata};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::robotics::cameras::{CameraManager, Frame};

#[derive(Clone, Debug)]
pub(crate) struct CameraCaptureSpec {
    pub(crate) stream_id: String,
    pub(crate) camera_id: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps: u32,
}

/// Hard ceiling for one active camera segment. Camera bytes are transient
/// transaction state until the episode is committed to Spiral, never a local
/// persistence layer.
const MAX_VIDEO_SEGMENT_BYTES: usize = 512 * 1024 * 1024;
const MAX_VIDEO_TRANSACTION_BYTES: usize = 1024 * 1024 * 1024;

pub(crate) struct VideoCapture {
    episode_id: String,
    cameras: Arc<CameraManager>,
    specs: Vec<CameraCaptureSpec>,
    ffmpeg: String,
    codec: String,
    next_segment: u32,
    active: Option<VideoCaptureRun>,
    completed: Vec<VideoAsset>,
}

struct VideoCaptureRun {
    control: watch::Sender<CaptureState>,
    tasks: Vec<tokio::task::JoinHandle<Result<VideoAsset, String>>>,
}

struct PreparedCameraSegment {
    spec: CameraCaptureSpec,
    frames: watch::Receiver<Option<Arc<Frame>>>,
    segment: CameraSegment,
}

/// Immutable configuration for one encoded interval of one camera stream.
struct CameraSegment {
    ffmpeg: String,
    codec: String,
    index: u32,
    timeline_offset_seconds: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureState {
    Recording,
    Finish,
}

#[derive(Clone, Debug)]
pub(crate) struct VideoEncoderSettings {
    pub(crate) executable: String,
    pub(crate) codec: String,
}

impl VideoCapture {
    pub(crate) async fn start(
        cameras: Arc<CameraManager>,
        episode_id: &str,
        specs: Vec<CameraCaptureSpec>,
        ffmpeg: &str,
        codec: &str,
    ) -> Result<Option<Self>, String> {
        if specs.is_empty() {
            return Ok(None);
        }
        if !safe_path_component(episode_id) {
            return Err(format!(
                "episode id {episode_id:?} cannot be used as a video asset key"
            ));
        }
        let mut capture = Self {
            episode_id: episode_id.to_owned(),
            cameras,
            specs,
            ffmpeg: ffmpeg.to_owned(),
            codec: codec.to_owned(),
            next_segment: 0,
            active: None,
            completed: Vec::new(),
        };
        capture.start_segment(0.0)?;
        Ok(Some(capture))
    }

    pub(crate) fn episode_id(&self) -> &str {
        &self.episode_id
    }

    pub(crate) fn completed_assets(&self) -> Vec<VideoAsset> {
        self.completed.clone()
    }

    fn start_segment(&mut self, timeline_offset_seconds: f64) -> Result<(), String> {
        let cameras = self.cameras.clone();
        self.start_segment_with(timeline_offset_seconds, move |camera_id| {
            cameras.subscribe(camera_id)
        })
    }

    fn start_segment_with(
        &mut self,
        timeline_offset_seconds: f64,
        mut subscribe: impl FnMut(&str) -> Result<watch::Receiver<Option<Arc<Frame>>>, String>,
    ) -> Result<(), String> {
        if self.active.is_some() {
            return Ok(());
        }
        if !timeline_offset_seconds.is_finite() || timeline_offset_seconds < 0.0 {
            return Err("video segment timeline offset must be finite and non-negative".to_owned());
        }
        if self.ffmpeg.trim().is_empty() {
            return Err("video encoder executable cannot be empty".to_owned());
        }
        if self.codec.trim().is_empty() {
            return Err("video encoder codec cannot be empty".to_owned());
        }
        let index = self.next_segment;
        let mut pending = Vec::with_capacity(self.specs.len());
        for spec in self.specs.iter().cloned() {
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
        // never leave an earlier FFmpeg child or partial MP4 behind.
        let mut prepared = Vec::with_capacity(pending.len());
        for spec in pending {
            let frames = subscribe(&spec.camera_id)?;
            prepared.push(PreparedCameraSegment {
                spec,
                frames,
                segment: CameraSegment {
                    ffmpeg: self.ffmpeg.clone(),
                    codec: self.codec.clone(),
                    index,
                    timeline_offset_seconds,
                },
            });
        }

        let (control, _) = watch::channel(CaptureState::Recording);
        let mut tasks = Vec::with_capacity(prepared.len());
        for prepared in prepared {
            tasks.push(tokio::spawn(record_camera(
                prepared.spec,
                prepared.frames,
                control.subscribe(),
                prepared.segment,
            )));
        }
        self.next_segment += 1;
        self.active = Some(VideoCaptureRun { control, tasks });
        Ok(())
    }

    /// Stop feeding the current encoders immediately. Their finalized MP4s are
    /// collected after the joint-writer pause barrier has completed.
    pub(crate) fn request_pause(&self) {
        if let Some(active) = &self.active {
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
        let mut assets = Vec::new();
        let mut failures = Vec::new();
        let Some(active) = self.active.take() else {
            return failures;
        };
        active.control.send_replace(CaptureState::Finish);
        for task in active.tasks {
            match task.await {
                Ok(Ok(asset)) => assets.push(asset),
                Ok(Err(error)) => failures.push(error),
                Err(error) => failures.push(format!("camera recording task failed: {error}")),
            }
        }
        let mut total = self
            .completed
            .iter()
            .map(|asset| asset.payload.len())
            .sum::<usize>();
        for asset in assets {
            let next = total.saturating_add(asset.payload.len());
            if next > MAX_VIDEO_TRANSACTION_BYTES {
                failures.push(format!(
                    "episode video exceeded the {} MiB in-memory transaction limit",
                    MAX_VIDEO_TRANSACTION_BYTES / (1024 * 1024)
                ));
                continue;
            }
            total = next;
            self.completed.push(asset);
        }
        failures
    }

    pub(crate) async fn finish(mut self) -> (Vec<VideoAsset>, Vec<String>) {
        let failures = self.finish_active().await;
        (self.completed, failures)
    }
}

async fn record_camera(
    spec: CameraCaptureSpec,
    mut frames: watch::Receiver<Option<Arc<Frame>>>,
    mut control: watch::Receiver<CaptureState>,
    segment: CameraSegment,
) -> Result<VideoAsset, String> {
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
            // JPEG camera frames are full-range. Convert their sample values as
            // well as tagging the H.264 output, so decoders see ordinary
            // limited-range yuv420p instead of the deprecated yuvj420p alias.
            "-vf",
            "scale=in_range=auto:out_range=tv",
            "-pix_fmt",
            "yuv420p",
            "-color_range",
            "tv",
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof",
            "-f",
            "mp4",
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
                .map_err(|error| format!("could not read FFmpeg MP4 output: {error}"))?;
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

    let mut written = 0_u64;
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
                if frame.sequence <= last_sequence {
                    continue;
                }
                stdin.write_all(&frame.bytes).await.map_err(|error| {
                    format!("camera {} could not write to FFmpeg: {error}", spec.stream_id)
                })?;
                last_sequence = frame.sequence;
                written += 1;
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
    if written == 0 || payload.is_empty() {
        return Err(format!(
            "camera {} delivered no video frames",
            spec.stream_id
        ));
    }
    Ok(VideoAsset {
        stream_id: spec.stream_id,
        camera_id: spec.camera_id,
        payload,
        width: spec.width,
        height: spec.height,
        fps: encoder_fps,
        frames: written,
        segment_index: segment.index,
        timeline_offset_seconds: Some(segment.timeline_offset_seconds),
    })
}

fn safe_path_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(specs: Vec<CameraCaptureSpec>) -> VideoCapture {
        VideoCapture {
            episode_id: "episode_000001".to_owned(),
            cameras: Arc::new(CameraManager::new()),
            specs,
            ffmpeg: "ffmpeg".to_owned(),
            codec: "libx264".to_owned(),
            next_segment: 0,
            active: None,
            completed: Vec::new(),
        }
    }

    #[test]
    fn episode_ids_are_safe_logical_keys() {
        assert!(safe_path_component("episode_000001"));
        assert!(!safe_path_component("../episode"));
        assert!(!safe_path_component(""));
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
        assert!(capture.active.is_none());
        assert_eq!(capture.next_segment, 0);
    }

    #[test]
    fn video_metadata_is_derived_without_a_filesystem_path() {
        let asset = VideoAsset {
            stream_id: "follower.camera.wrist".to_owned(),
            camera_id: "camera-1".to_owned(),
            payload: b"mp4".to_vec(),
            width: 640,
            height: 480,
            fps: 30,
            frames: 15,
            segment_index: 2,
            timeline_offset_seconds: Some(4.5),
        };
        assert_eq!(asset.metadata().duration_seconds, 0.5);
        assert_eq!(asset.metadata().segment_index, 2);
        assert_eq!(asset.payload, b"mp4");
    }
}
