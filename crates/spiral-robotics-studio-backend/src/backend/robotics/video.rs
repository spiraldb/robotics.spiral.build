//! Server-side camera recording into browser-playable MP4 assets.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::backend::robotics::cameras::{CameraManager, Frame};

#[derive(Clone, Debug)]
pub(crate) struct CameraCaptureSpec {
    pub stream_id: String,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct VideoAsset {
    pub stream_id: String,
    pub camera_id: String,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
    /// Zero-based active interval for this logical camera stream.
    #[serde(default)]
    pub segment_index: u32,
    /// Start of this segment on the gap-free episode timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_offset_seconds: Option<f64>,
}

/// Filesystem-free metadata safe to expose to a remote studio client.
///
/// Paused captures expose one finalized, seekable asset per active interval.
/// `timeline_offset_seconds` maps each segment onto the same gap-free clock as
/// joint samples; `segment_index` selects its local HTTP asset.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct VideoAssetMetadata {
    pub stream_id: String,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
    pub duration_seconds: f64,
    pub segment_index: u32,
    pub timeline_offset_seconds: Option<f64>,
}

impl VideoAsset {
    pub(crate) fn metadata(&self) -> VideoAssetMetadata {
        VideoAssetMetadata {
            stream_id: self.stream_id.clone(),
            camera_id: self.camera_id.clone(),
            width: self.width,
            height: self.height,
            fps: self.fps,
            frames: self.frames,
            duration_seconds: if self.fps == 0 {
                0.0
            } else {
                self.frames as f64 / self.fps as f64
            },
            segment_index: self.segment_index,
            timeline_offset_seconds: self.timeline_offset_seconds,
        }
    }
}

const ASSET_MANIFEST: &str = "assets.json";

pub(crate) fn save_assets(
    root: &Path,
    episode_id: &str,
    assets: &[VideoAsset],
) -> Result<(), String> {
    let directory = root.join("videos").join(episode_id);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    let path = directory.join(ASSET_MANIFEST);
    let temporary = directory.join("assets.json.tmp");
    let portable = assets
        .iter()
        .cloned()
        .map(|mut asset| {
            asset.path = asset
                .path
                .strip_prefix(root)
                .map(Path::to_path_buf)
                .map_err(|error| {
                    format!(
                        "video asset {} is outside episode directory {}: {error}",
                        asset.path.display(),
                        root.display()
                    )
                })?;
            Ok(asset)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let bytes = serde_json::to_vec_pretty(&portable)
        .map_err(|error| format!("could not encode episode video manifest: {error}"))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|error| format!("could not open {}: {error}", temporary.display()))?;
    use std::io::Write as _;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    drop(file);
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("could not publish {}: {error}", path.display()))?;
    std::fs::File::open(&directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("could not sync {}: {error}", directory.display()))
}

pub(crate) fn load_assets(root: &Path, episode_id: &str) -> Result<Vec<VideoAsset>, String> {
    let path = root.join("videos").join(episode_id).join(ASSET_MANIFEST);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let mut assets: Vec<VideoAsset> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    for asset in &mut assets {
        let relative = if asset.path.is_absolute() {
            asset.path.strip_prefix(root).map(Path::to_path_buf)
        } else if let Ok(relative) = asset.path.strip_prefix(root) {
            // Older manifests written with a relative episode root contain
            // `episodes/videos/...` instead of `videos/...`.
            Ok(relative.to_path_buf())
        } else if let Some(root_name) = root
            .file_name()
            .filter(|root_name| asset.path.starts_with(Path::new(root_name)))
        {
            asset
                .path
                .strip_prefix(Path::new(root_name))
                .map(Path::to_path_buf)
        } else {
            Ok(asset.path.clone())
        }
        .map_err(|_| {
            format!(
                "video manifest asset {} is outside episode directory {}",
                asset.path.display(),
                root.display()
            )
        })?;
        if relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "video manifest contains an unavailable asset {}",
                asset.path.display()
            ));
        }
        let resolved = root.join(relative);
        if !resolved.is_file() {
            return Err(format!(
                "video manifest contains an unavailable asset {}",
                asset.path.display()
            ));
        }
        asset.path = resolved;
    }
    Ok(assets)
}

pub(crate) struct VideoCapture {
    episode_id: String,
    cameras: Arc<CameraManager>,
    root: PathBuf,
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
    output: PathBuf,
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
    pub executable: String,
    pub codec: String,
}

impl VideoCapture {
    pub(crate) async fn start(
        cameras: Arc<CameraManager>,
        root: &Path,
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
                "episode id {episode_id:?} cannot be used as a video directory"
            ));
        }
        let directory = root.join("videos").join(episode_id);
        let directory_existed = directory.exists();
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        let mut capture = Self {
            episode_id: episode_id.to_owned(),
            cameras,
            root: root.to_path_buf(),
            specs,
            ffmpeg: ffmpeg.to_owned(),
            codec: codec.to_owned(),
            next_segment: 0,
            active: None,
            completed: Vec::new(),
        };
        if let Err(error) = capture.start_segment(0.0) {
            if !directory_existed {
                let _ = std::fs::remove_dir(&directory);
            }
            return Err(error);
        }
        Ok(Some(capture))
    }

    pub(crate) fn episode_id(&self) -> &str {
        &self.episode_id
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
        let directory = self.root.join("videos").join(&self.episode_id);
        let mut outputs = BTreeSet::new();
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
            let output = directory.join(format!(
                "{}.segment-{index:03}.mp4",
                safe_file_stem(&spec.stream_id)
            ));
            if !outputs.insert(output.clone()) {
                return Err(format!(
                    "video stream {:?} maps to the same output path as another camera",
                    spec.stream_id
                ));
            }
            match std::fs::symlink_metadata(&output) {
                Ok(_) => {
                    return Err(format!(
                        "video segment output {} already exists",
                        output.display()
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "could not validate video output {}: {error}",
                        output.display()
                    ));
                }
            }
            pending.push((spec, output));
        }

        // Subscriptions may fail independently (for example, a camera can be
        // unplugged between discovery and recording). Acquire every receiver
        // before the first encoder task starts so a later camera failure can
        // never leave an earlier FFmpeg child or partial MP4 behind.
        let mut prepared = Vec::with_capacity(pending.len());
        for (spec, output) in pending {
            let frames = subscribe(&spec.camera_id)?;
            prepared.push(PreparedCameraSegment {
                spec,
                frames,
                segment: CameraSegment {
                    ffmpeg: self.ffmpeg.clone(),
                    codec: self.codec.clone(),
                    output,
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

    /// Finalize and publish a provisional segment manifest for paused review.
    pub(crate) async fn finish_pause(&mut self) -> Vec<String> {
        let failures = self.finish_active().await;
        if let Err(error) = save_assets(&self.root, &self.episode_id, &self.completed) {
            return failures.into_iter().chain([error]).collect();
        }
        failures
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
        self.completed.extend(assets);
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
            "+faststart",
            "-y",
        ])
        .arg(&segment.output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not start {:?}: {error}", segment.ffmpeg))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "FFmpeg did not open its input pipe".to_owned())?;
    let mut stderr = child.stderr.take();
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
    let bytes = tokio::fs::metadata(&segment.output)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if written == 0 || bytes == 0 {
        let _ = tokio::fs::remove_file(&segment.output).await;
        return Err(format!(
            "camera {} delivered no video frames",
            spec.stream_id
        ));
    }
    Ok(VideoAsset {
        stream_id: spec.stream_id,
        camera_id: spec.camera_id,
        path: segment.output,
        width: spec.width,
        height: spec.height,
        fps: encoder_fps,
        frames: written,
        segment_index: segment.index,
        timeline_offset_seconds: Some(segment.timeline_offset_seconds),
    })
}

fn safe_file_stem(value: &str) -> String {
    let rendered = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if rendered.is_empty() {
        "camera".to_owned()
    } else {
        rendered
    }
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

    #[test]
    fn stream_names_cannot_escape_the_video_directory() {
        assert_eq!(safe_file_stem("arm/wrist ../"), "arm_wrist____");
    }

    #[test]
    fn second_camera_subscription_failure_starts_no_encoder_or_output() {
        let directory = tempfile::tempdir().unwrap();
        let episode_id = "episode_000001";
        let video_directory = directory.path().join("videos").join(episode_id);
        std::fs::create_dir_all(&video_directory).unwrap();
        let mut capture = VideoCapture {
            episode_id: episode_id.to_owned(),
            cameras: Arc::new(CameraManager::new()),
            root: directory.path().to_path_buf(),
            specs: vec![
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
            ffmpeg: "ffmpeg".to_owned(),
            codec: "libx264".to_owned(),
            next_segment: 0,
            active: None,
            completed: Vec::new(),
        };
        let (frames, receiver) = watch::channel(None::<Arc<Frame>>);
        let mut subscriptions = 0;

        let error = capture
            .start_segment_with(0.0, |_camera_id| {
                subscriptions += 1;
                if subscriptions == 1 {
                    Ok(receiver.clone())
                } else {
                    Err("second camera disappeared".to_owned())
                }
            })
            .expect_err("second subscription fails");
        drop(frames);

        assert!(error.contains("second camera disappeared"));
        assert_eq!(subscriptions, 2);
        assert!(
            capture.active.is_none(),
            "no encoder task set was published"
        );
        assert_eq!(capture.next_segment, 0, "the failed segment is retryable");
        assert!(
            std::fs::read_dir(&video_directory)
                .unwrap()
                .next()
                .is_none(),
            "no first-camera partial output was created"
        );
    }

    #[test]
    fn sanitized_stream_name_collisions_are_rejected_before_subscription() {
        let directory = tempfile::tempdir().unwrap();
        let episode_id = "episode_000001";
        std::fs::create_dir_all(directory.path().join("videos").join(episode_id)).unwrap();
        let mut capture = VideoCapture {
            episode_id: episode_id.to_owned(),
            cameras: Arc::new(CameraManager::new()),
            root: directory.path().to_path_buf(),
            specs: vec![
                CameraCaptureSpec {
                    stream_id: "arm/wrist".to_owned(),
                    camera_id: "camera-1".to_owned(),
                    width: 640,
                    height: 480,
                    fps: 30,
                },
                CameraCaptureSpec {
                    stream_id: "arm_wrist".to_owned(),
                    camera_id: "camera-2".to_owned(),
                    width: 640,
                    height: 480,
                    fps: 30,
                },
            ],
            ffmpeg: "ffmpeg".to_owned(),
            codec: "libx264".to_owned(),
            next_segment: 0,
            active: None,
            completed: Vec::new(),
        };
        let mut subscriptions = 0;

        let error = capture
            .start_segment_with(0.0, |_camera_id| {
                subscriptions += 1;
                let (_, receiver) = watch::channel(None::<Arc<Frame>>);
                Ok(receiver)
            })
            .expect_err("colliding outputs fail");

        assert!(error.contains("same output path"));
        assert_eq!(subscriptions, 0, "paths are validated before cameras open");
    }

    #[test]
    fn video_asset_manifests_round_trip_for_retries() {
        let directory = tempfile::tempdir().unwrap();
        let video_directory = directory.path().join("videos/episode_000001");
        std::fs::create_dir_all(&video_directory).unwrap();
        let path = video_directory.join("wrist.mp4");
        std::fs::write(&path, b"mp4").unwrap();
        let assets = vec![VideoAsset {
            stream_id: "follower.camera.wrist".to_owned(),
            camera_id: "camera-1".to_owned(),
            path,
            width: 640,
            height: 480,
            fps: 30,
            frames: 10,
            segment_index: 2,
            timeline_offset_seconds: Some(4.5),
        }];
        save_assets(directory.path(), "episode_000001", &assets).unwrap();
        let manifest = std::fs::read_to_string(video_directory.join(ASSET_MANIFEST)).unwrap();
        assert!(manifest.contains(r#""path": "videos/episode_000001/wrist.mp4""#));
        assert!(!manifest.contains(directory.path().to_string_lossy().as_ref()));
        let loaded = load_assets(directory.path(), "episode_000001").unwrap();
        assert_eq!(loaded[0].stream_id, assets[0].stream_id);
        assert_eq!(loaded[0].frames, 10);
        assert_eq!(loaded[0].metadata().duration_seconds, 10.0 / 30.0);
        assert_eq!(loaded[0].metadata().segment_index, 2);
        assert_eq!(loaded[0].metadata().timeline_offset_seconds, Some(4.5));
    }

    #[test]
    fn cwd_prefixed_legacy_manifest_resolves_against_the_episode_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("episodes");
        let video_directory = root.join("videos/episode_000001");
        std::fs::create_dir_all(&video_directory).unwrap();
        std::fs::write(video_directory.join("wrist.mp4"), b"mp4").unwrap();
        std::fs::write(
            video_directory.join(ASSET_MANIFEST),
            serde_json::to_vec(&serde_json::json!([{
                "stream_id": "follower.camera.wrist",
                "camera_id": "camera-1",
                "path": "episodes/videos/episode_000001/wrist.mp4",
                "width": 640,
                "height": 480,
                "fps": 30,
                "frames": 10
            }]))
            .unwrap(),
        )
        .unwrap();

        let loaded = load_assets(&root, "episode_000001").unwrap();
        assert_eq!(loaded[0].path, video_directory.join("wrist.mp4"));
    }

    #[test]
    fn legacy_video_manifests_do_not_invent_clock_alignment() {
        let asset: VideoAsset = serde_json::from_value(serde_json::json!({
            "stream_id": "follower.camera.wrist",
            "camera_id": "camera-1",
            "path": "/tmp/legacy.mp4",
            "width": 640,
            "height": 480,
            "fps": 30,
            "frames": 10
        }))
        .expect("legacy manifest");

        assert_eq!(asset.segment_index, 0);
        assert_eq!(asset.timeline_offset_seconds, None);
    }
}
