//! A LeRobot camera's frames of one episode, cut out of the repository's
//! video by FFmpeg and encoded again as the H.264 Studio records: one access
//! unit per frame, no B-frames, a keyframe every 30 frames with its
//! parameter sets, so every episode decodes from its first frame whatever
//! codec and keyframe spacing the repository used.
use crate::FfmpegExecutable;
use spiral_robotics_common::datasets::H264AccessUnit;
use spiral_robotics_recording::AccessUnits;
use std::path::Path;
use std::process::Stdio;

/// The FFmpeg executable cameras are encoded with.
pub(super) struct CameraEncoder {
    executable: FfmpegExecutable,
}

/// An episode's frames within a video: the second to seek to and how many
/// frames to take from there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FrameSpan {
    seek_seconds: f64,
    frames: usize,
}

impl FrameSpan {
    /// `frames` frames from the one at `from_seconds`, in a video of `fps`
    /// frames a second. The seek lands half a frame early, so a start time
    /// rounded a hair late still takes that frame and never the one before.
    pub(super) fn starting(from_seconds: f64, fps: f64, frames: usize) -> Self {
        Self {
            seek_seconds: (from_seconds - 0.5 / fps).max(0.0),
            frames,
        }
    }
}

/// One span of a video file on local disk. A cut reads the file it seeks
/// in, never one over HTTPS: a repository's video holds hundreds of
/// episodes, and every FFmpeg reading it remotely fetched and parsed its
/// index again, seconds per episode against milliseconds from disk.
pub(super) struct CameraCut<'a> {
    video: &'a Path,
    span: FrameSpan,
}

impl<'a> CameraCut<'a> {
    pub(super) fn new(video: &'a Path, span: FrameSpan) -> Self {
        Self { video, span }
    }

    /// FFmpeg's arguments: read the span, decode it whatever its codec, and
    /// write it as Studio's H.264 to standard output.
    fn arguments(&self) -> Vec<String> {
        let mut arguments = ["-hide_banner", "-loglevel", "error", "-nostdin"]
            .map(str::to_owned)
            .to_vec();
        arguments.extend([
            "-ss".into(),
            format!("{:.6}", self.span.seek_seconds),
            "-i".into(),
            self.video.display().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-an".into(),
            "-frames:v".into(),
            self.span.frames.to_string(),
            // One frame out for every frame in, whatever their timestamps.
            "-fps_mode".into(),
            "passthrough".into(),
            "-c:v".into(),
            "libx264".into(),
        ]);
        arguments.extend(H264AccessUnit::FFMPEG_OUTPUT_OPTIONS.map(str::to_owned));
        arguments.push("pipe:1".into());
        arguments
    }
}

impl CameraEncoder {
    pub(super) fn from_environment() -> Self {
        Self {
            executable: FfmpegExecutable::from_environment(),
        }
    }

    /// `cut`'s frames, exactly one access unit each, the first a keyframe.
    pub(super) async fn encode(&self, cut: &CameraCut<'_>) -> Result<Vec<Vec<u8>>, String> {
        let output = tokio::process::Command::new(&self.executable)
            .args(cut.arguments())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start {:?}: {error}", self.executable.to_string()))?
            .wait_with_output()
            .await
            .map_err(|error| format!("FFmpeg failed: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "FFmpeg failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Self::access_units(&output.stdout, cut.span.frames)
    }

    /// `encoded` split at each access unit delimiter, which must make
    /// exactly `frames` access units of Studio's H.264.
    fn access_units(encoded: &[u8], frames: usize) -> Result<Vec<Vec<u8>>, String> {
        let mut splitter = AccessUnits::default();
        let mut units = splitter.push(encoded);
        units.extend(splitter.finish());
        if units.len() != frames {
            return Err(format!(
                "FFmpeg encoded {} access units for {frames} frames",
                units.len()
            ));
        }
        for (frame, unit) in units.iter().enumerate() {
            let parsed =
                H264AccessUnit::parse(unit).map_err(|error| format!("frame {frame}: {error}"))?;
            if frame == 0 && !parsed.keyframe() {
                return Err("FFmpeg's first frame is no keyframe".into());
            }
        }
        Ok(units)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_seeks_half_a_frame_early_and_never_before_the_start() {
        let span = FrameSpan::starting(10.1, 30.0, 266);
        assert!((span.seek_seconds - (10.1 - 1.0 / 60.0)).abs() < 1e-9);
        assert_eq!(FrameSpan::starting(0.0, 30.0, 892).seek_seconds, 0.0);
    }

    #[test]
    fn ffmpeg_reads_the_span_of_the_local_copy_and_writes_studio_h264() {
        let video = Path::new("/tmp/file-000.mp4");
        let cut = CameraCut::new(video, FrameSpan::starting(1.0, 30.0, 45));
        let arguments = cut.arguments().join(" ");
        assert!(arguments.contains("-ss 0.983333 -i /tmp/file-000.mp4"));
        assert!(arguments.contains(
            "-frames:v 45 -fps_mode passthrough -c:v libx264 -pix_fmt yuv420p -bf 0 -g 30"
        ));
        assert!(arguments.ends_with("-f h264 pipe:1"));
    }

    #[test]
    fn every_frame_is_one_access_unit_and_the_first_a_keyframe() {
        let delimiter = [0, 0, 0, 1, 9, 0xf0];
        let parameters = [0, 0, 0, 1, 0x67, 0x64, 0, 0x1f, 0, 0, 1, 0x68, 0xee];
        let idr = [0, 0, 1, 0x65, 0x88];
        let predicted = [0, 0, 1, 0x41, 0x9a];
        let keyframe = [delimiter.as_slice(), &parameters, &idr].concat();
        let next = [delimiter.as_slice(), &predicted].concat();
        let stream = [keyframe.clone(), next.clone(), next.clone()].concat();
        let units = CameraEncoder::access_units(&stream, 3).unwrap();
        assert_eq!(units, [keyframe, next.clone(), next.clone()]);
        assert!(CameraEncoder::access_units(&stream, 4).is_err());
        assert!(CameraEncoder::access_units(&[next.clone(), next].concat(), 2).is_err());
    }

    /// Cuts frames out of a video FFmpeg made with B-frames and a keyframe
    /// every 10 frames, from a frame that is no keyframe, as most episodes of
    /// sim-bottles-mjwarp-v1 start.
    #[tokio::test]
    async fn an_episode_starting_between_keyframes_decodes_from_its_first_frame() {
        let directory = tempfile::tempdir().unwrap();
        let video = directory.path().join("video.mp4");
        let encoder = CameraEncoder::from_environment();
        let made = tokio::process::Command::new(&encoder.executable)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x48:rate=30:duration=2",
            ])
            .args([
                "-c:v", "libx264", "-bf", "2", "-g", "10", "-pix_fmt", "yuv420p", "-y",
            ])
            .arg(&video)
            .output()
            .await
            .expect("FFmpeg with libx264, as SPIRAL_ROBOTICS_FFMPEG or on PATH");
        assert!(
            made.status.success(),
            "{}",
            String::from_utf8_lossy(&made.stderr)
        );
        let cut = CameraCut::new(&video, FrameSpan::starting(13.0 / 30.0, 30.0, 37));
        let units = encoder.encode(&cut).await.unwrap();
        assert_eq!(units.len(), 37);
        let keyframes = units
            .iter()
            .enumerate()
            .filter(|(_, unit)| H264AccessUnit::holds_idr(unit))
            .map(|(frame, _)| frame)
            .collect::<Vec<_>>();
        assert_eq!(keyframes, [0, 30]);
    }
}
