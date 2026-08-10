//! Server-side camera recording into browser-playable MP4 assets.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use crate::camera_manager::{CameraManager, Frame};

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
}

const ASSET_MANIFEST: &str = "assets.json";

pub fn save_assets(root: &Path, episode_id: &str, assets: &[VideoAsset]) -> Result<(), String> {
    let directory = root.join("videos").join(episode_id);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    let path = directory.join(ASSET_MANIFEST);
    let temporary = directory.join("assets.json.tmp");
    let bytes = serde_json::to_vec_pretty(assets)
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
    let assets: Vec<VideoAsset> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    for asset in &assets {
        if !asset.path.starts_with(root) || !asset.path.is_file() {
            return Err(format!(
                "video manifest contains an unavailable asset {}",
                asset.path.display()
            ));
        }
    }
    Ok(assets)
}

pub struct VideoCapture {
    episode_id: String,
    cancel: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<Result<VideoAsset, String>>>,
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
        let (cancel, _) = watch::channel(false);
        let mut tasks = Vec::with_capacity(specs.len());
        for spec in specs {
            let frames = cameras.subscribe(&spec.camera_id)?;
            let output = directory.join(format!("{}.mp4", safe_file_stem(&spec.stream_id)));
            tasks.push(tokio::spawn(record_camera(
                spec,
                frames,
                cancel.subscribe(),
                ffmpeg.to_owned(),
                codec.to_owned(),
                output,
            )));
        }
        Ok(Some(Self {
            episode_id: episode_id.to_owned(),
            cancel,
            tasks,
        }))
    }

    pub fn episode_id(&self) -> &str {
        &self.episode_id
    }

    pub async fn finish(self) -> (Vec<VideoAsset>, Vec<String>) {
        self.cancel.send_replace(true);
        let mut assets = Vec::new();
        let mut failures = Vec::new();
        for task in self.tasks {
            match task.await {
                Ok(Ok(asset)) => assets.push(asset),
                Ok(Err(error)) => failures.push(error),
                Err(error) => failures.push(format!("camera recording task failed: {error}")),
            }
        }
        (assets, failures)
    }
}

async fn record_camera(
    spec: CameraCaptureSpec,
    mut frames: watch::Receiver<Option<Arc<Frame>>>,
    mut cancel: watch::Receiver<bool>,
    ffmpeg: String,
    codec: String,
    output: PathBuf,
) -> Result<VideoAsset, String> {
    let rate = spec.fps.clamp(1, 120).to_string();
    let mut child = tokio::process::Command::new(&ffmpeg)
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
            &codec,
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-y",
        ])
        .arg(&output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not start {ffmpeg:?}: {error}"))?;
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
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
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
    let bytes = tokio::fs::metadata(&output)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if written == 0 || bytes == 0 {
        let _ = tokio::fs::remove_file(&output).await;
        return Err(format!(
            "camera {} delivered no video frames",
            spec.stream_id
        ));
    }
    Ok(VideoAsset {
        stream_id: spec.stream_id,
        camera_id: spec.camera_id,
        path: output,
        width: spec.width,
        height: spec.height,
        fps: spec.fps,
        frames: written,
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
        }];
        save_assets(directory.path(), "episode_000001", &assets).unwrap();
        let loaded = load_assets(directory.path(), "episode_000001").unwrap();
        assert_eq!(loaded[0].stream_id, assets[0].stream_id);
        assert_eq!(loaded[0].frames, 10);
    }
}
