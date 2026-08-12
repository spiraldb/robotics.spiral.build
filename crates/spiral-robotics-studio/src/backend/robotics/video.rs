//! Server-side camera recording into browser-playable MP4 assets.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::backend::robotics::cameras::{CameraManager, Frame};

#[derive(Clone, Debug)]
pub struct CameraCaptureSpec {
    pub stream_id: String,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VideoAsset {
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
pub struct VideoAssetMetadata {
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
    pub fn metadata(&self) -> VideoAssetMetadata {
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

pub fn save_assets(root: &Path, episode_id: &str, assets: &[VideoAsset]) -> Result<(), String> {
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
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("could not publish {}: {error}", path.display()))
}

pub fn load_assets(root: &Path, episode_id: &str) -> Result<Vec<VideoAsset>, String> {
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

pub struct VideoCapture {
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
pub struct VideoEncoderSettings {
    pub executable: String,
    pub codec: String,
}

impl VideoCapture {
    pub async fn start(
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
        let directory = root.join("videos").join(episode_id);
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
        capture.start_segment(0.0)?;
        Ok(Some(capture))
    }

    pub fn episode_id(&self) -> &str {
        &self.episode_id
    }

    fn start_segment(&mut self, timeline_offset_seconds: f64) -> Result<(), String> {
        if self.active.is_some() {
            return Ok(());
        }
        let index = self.next_segment;
        let directory = self.root.join("videos").join(&self.episode_id);
        let (control, _) = watch::channel(CaptureState::Recording);
        let mut tasks = Vec::with_capacity(self.specs.len());
        for spec in self.specs.iter().cloned() {
            let frames = self.cameras.subscribe(&spec.camera_id)?;
            let output = directory.join(format!(
                "{}.segment-{index:03}.mp4",
                safe_file_stem(&spec.stream_id)
            ));
            tasks.push(tokio::spawn(record_camera(
                spec,
                frames,
                control.subscribe(),
                CameraSegment {
                    ffmpeg: self.ffmpeg.clone(),
                    codec: self.codec.clone(),
                    output,
                    index,
                    timeline_offset_seconds,
                },
            )));
        }
        self.next_segment += 1;
        self.active = Some(VideoCaptureRun { control, tasks });
        Ok(())
    }

    /// Stop feeding the current encoders immediately. Their finalized MP4s are
    /// collected after the joint-writer pause barrier has completed.
    pub fn request_pause(&self) {
        if let Some(active) = &self.active {
            active.control.send_replace(CaptureState::Finish);
        }
    }

    /// Finalize and publish a provisional segment manifest for paused review.
    pub async fn finish_pause(&mut self) -> Vec<String> {
        let failures = self.finish_active().await;
        if let Err(error) = save_assets(&self.root, &self.episode_id, &self.completed) {
            return failures.into_iter().chain([error]).collect();
        }
        failures
    }

    /// Open the next active interval under the same episode and camera set.
    pub fn resume(&mut self, timeline_offset_seconds: f64) -> Result<(), String> {
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

    pub async fn finish(mut self) -> (Vec<VideoAsset>, Vec<String>) {
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
            "-pix_fmt",
            "yuv420p",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_names_cannot_escape_the_video_directory() {
        assert_eq!(safe_file_stem("arm/wrist ../"), "arm_wrist____");
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
