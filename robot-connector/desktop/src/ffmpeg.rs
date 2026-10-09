//! H.264 for recordings from an FFmpeg child process, one per camera segment.

use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use futures_util::future::BoxFuture;
use spiral_robotics_connector::VideoEncoder;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;

/// Hard ceiling for one camera segment's encoded bytes. Camera bytes are
/// transient transaction state until the episode is committed to Spiral,
/// never a local persistence layer.
const MAX_VIDEO_SEGMENT_BYTES: usize = 512 * 1024 * 1024;
/// Repeat the parameter sets at every keyframe and delimit access units, so a
/// recording decodes from any keyframe.
const H264_PACKET_FILTER: &str = "dump_extra=freq=keyframe,h264_metadata=aud=insert";
/// How long FFmpeg gets to flush its last frames once its input closes.
const FINISH_TIMEOUT: Duration = Duration::from_secs(10);

/// Encodes with the FFmpeg executable and codec named on the command line.
pub(crate) struct FfmpegEncoder {
    executable: String,
    codec: String,
}

impl FfmpegEncoder {
    pub(crate) fn new(executable: String, codec: String) -> Result<Self, String> {
        if executable.trim().is_empty() {
            return Err("--ffmpeg cannot be empty".to_owned());
        }
        if codec.trim().is_empty() {
            return Err("--video-codec cannot be empty".to_owned());
        }
        Ok(Self { executable, codec })
    }
}

impl VideoEncoder for FfmpegEncoder {
    fn encode(
        &self,
        fps: u32,
        mut frames: mpsc::Receiver<Bytes>,
    ) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        let executable = self.executable.clone();
        let codec = self.codec.clone();
        Box::pin(async move {
            let rate = fps.to_string();
            let mut child = tokio::process::Command::new(&executable)
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
                .map_err(|error| format!("could not start {executable:?}: {error}"))?;
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
                            "segment exceeded the {} MiB in-memory transaction limit",
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

            while let Some(frame) = frames.recv().await {
                stdin
                    .write_all(&frame)
                    .await
                    .map_err(|error| format!("could not write to FFmpeg: {error}"))?;
            }
            drop(stdin);

            let status = match tokio::time::timeout(FINISH_TIMEOUT, child.wait()).await {
                Ok(result) => result.map_err(|error| format!("FFmpeg wait failed: {error}"))?,
                Err(_) => {
                    let _ = child.kill().await;
                    return Err("FFmpeg did not finish".to_owned());
                }
            };
            let stderr = stderr_task.await.unwrap_or_default();
            if !status.success() {
                return Err(if stderr.is_empty() {
                    "FFmpeg failed".to_owned()
                } else {
                    format!("FFmpeg failed: {stderr}")
                });
            }
            output_task
                .await
                .map_err(|error| format!("FFmpeg output task failed: {error}"))?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_recording::CameraRecording;

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
}
