//! H.264 for recordings from an FFmpeg child process, one per camera segment.

use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use futures_util::future::BoxFuture;
use spiral_robotics_common::datasets::H264AccessUnit;
use spiral_robotics_connector::EncoderDriver;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;

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

impl EncoderDriver for FfmpegEncoder {
    fn encode(
        &self,
        fps: u32,
        mut frames: mpsc::Receiver<Bytes>,
        output: mpsc::UnboundedSender<Bytes>,
    ) -> BoxFuture<'static, Result<(), String>> {
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
                    "-color_range",
                    "tv",
                ])
                .args(H264AccessUnit::FFMPEG_OUTPUT_OPTIONS)
                .args(["-y", "pipe:1"])
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
            // FFmpeg's H.264 goes on as it comes out.
            let output_task = tokio::spawn(async move {
                let mut buffer = vec![0_u8; 64 * 1024];
                loop {
                    let read = stdout
                        .read(&mut buffer)
                        .await
                        .map_err(|error| format!("could not read FFmpeg H.264 output: {error}"))?;
                    if read == 0 {
                        return Ok::<_, String>(());
                    }
                    if output
                        .send(Bytes::copy_from_slice(&buffer[..read]))
                        .is_err()
                    {
                        return Err("the recording stopped taking H.264".to_owned());
                    }
                }
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
    use spiral_robotics_recording::AccessUnits;

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
                H264AccessUnit::FFMPEG_PACKET_FILTER,
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
        let mut units = AccessUnits::default();
        let mut frames = units.push(&output.stdout);
        frames.extend(units.finish());
        assert_eq!(frames.len(), 5);
        let keyframes = frames
            .iter()
            .map(|frame| H264AccessUnit::parse(frame).unwrap())
            .filter(|frame| frame.keyframe())
            .collect::<Vec<_>>();
        assert_eq!(keyframes.len(), 3);
        assert!(keyframes.iter().all(|frame| frame.codec().is_some()));
    }
}
