//! A LeRobot camera's frames of one episode as the H.264 Studio records: one
//! access unit per frame, no B-frames, the parameter sets at every keyframe,
//! the first frame a keyframe, so every episode decodes from its first frame.
//! ffprobe reads each of the repository's video files once. H.264 4:2:0
//! without B-frames is already that H.264 from any keyframe on: FFmpeg copies
//! an episode's frames from its first keyframe and encodes again only the
//! frames before it, in the file's profile and level, since Studio plays an
//! episode in one codec. FFmpeg decodes any other video (AV1, B-frames) and
//! encodes it again whole, with a keyframe every 30 frames.
use crate::FfmpegExecutable;
use serde::Deserialize;
use spiral_robotics_common::datasets::H264AccessUnit;
use spiral_robotics_recording::AccessUnits;
use std::path::Path;
use std::process::Stdio;

/// The FFmpeg executable cameras are cut with, and its ffprobe.
pub(super) struct CameraEncoder {
    executable: FfmpegExecutable,
}

/// A camera's video file on local disk, and what ffprobe read of it.
pub(super) struct CameraVideo {
    file: tempfile::NamedTempFile,
    /// How FFmpeg copies the file's frames, when it can.
    copyable: Option<CopyableH264>,
}

impl CameraVideo {
    pub(super) fn path(&self) -> &Path {
        self.file.path()
    }
}

/// A video whose frames are Studio's H.264 from each keyframe on: H.264
/// 4:2:0 without B-frames, in a profile libx264 encodes.
#[derive(Debug, PartialEq)]
struct CopyableH264 {
    /// The libx264 profile and level that encode the frames before a
    /// keyframe in the codec of the frames copied from it.
    profile: &'static str,
    level: i64,
    /// When each keyframe is, in seconds, in order.
    keyframes: Vec<f64>,
}

impl CopyableH264 {
    /// What ffprobe prints of a video: its first video stream and that
    /// stream's packets.
    const PROBE_ENTRIES: &str =
        "stream=codec_name,pix_fmt,has_b_frames,profile,level:packet=pts_time,flags";

    /// `probed`'s video when FFmpeg can copy its frames, `None` when FFmpeg
    /// must encode them again.
    fn of(probed: &ProbedVideo) -> Result<Option<Self>, String> {
        let [stream] = probed.streams.as_slice() else {
            return Err("ffprobe found no video stream".into());
        };
        if stream.codec_name.as_deref() != Some("h264")
            || stream.pix_fmt.as_deref() != Some("yuv420p")
            || stream.has_b_frames != Some(0)
        {
            return Ok(None);
        }
        let profile = match stream.profile.as_deref() {
            Some("Constrained Baseline") => "baseline",
            Some("Main") => "main",
            Some("High") => "high",
            _ => return Ok(None),
        };
        let Some(level) = stream.level.filter(|level| *level > 0) else {
            return Ok(None);
        };
        let keyframes = probed
            .packets
            .iter()
            .filter(|packet| packet.flags.starts_with('K'))
            .map(|packet| {
                packet
                    .pts_time
                    .parse::<f64>()
                    .map_err(|_| format!("ffprobe times a keyframe {:?}", packet.pts_time))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Self {
            profile,
            level,
            keyframes,
        }))
    }

    /// When the first keyframe at or after `seconds` is, if any.
    fn keyframe_from(&self, seconds: f64) -> Option<f64> {
        let index = self
            .keyframes
            .partition_point(|keyframe| *keyframe < seconds);
        self.keyframes.get(index).copied()
    }
}

/// What ffprobe prints of a video, as [`CopyableH264::PROBE_ENTRIES`] asks.
#[derive(Deserialize)]
struct ProbedVideo {
    #[serde(default)]
    streams: Vec<ProbedStream>,
    #[serde(default)]
    packets: Vec<ProbedPacket>,
}

#[derive(Deserialize)]
struct ProbedStream {
    codec_name: Option<String>,
    pix_fmt: Option<String>,
    has_b_frames: Option<u32>,
    profile: Option<String>,
    level: Option<i64>,
}

#[derive(Deserialize)]
struct ProbedPacket {
    /// Empty when ffprobe gives the packet no time.
    #[serde(default)]
    pts_time: String,
    flags: String,
}

/// An episode's frames within a video: when the first one is, how many
/// frames the video has a second, and how many to take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FrameSpan {
    from_seconds: f64,
    fps: f64,
    frames: usize,
}

impl FrameSpan {
    /// `frames` frames from the one at `from_seconds`, in a video of `fps`
    /// frames a second.
    pub(super) fn starting(from_seconds: f64, fps: f64, frames: usize) -> Self {
        Self {
            from_seconds,
            fps,
            frames,
        }
    }

    /// Where FFmpeg seeks to decode the span: half a frame early, so a
    /// start time rounded a hair late still takes that frame and never the
    /// one before. Decoding drops every frame before the seek.
    fn decoding_seek(&self) -> f64 {
        (self.from_seconds - 0.5 / self.fps).max(0.0)
    }

    /// Where FFmpeg seeks to copy the span, which starts at a keyframe: half
    /// a frame late, so it lands on that keyframe and never the one before.
    /// Copying keeps every frame from the keyframe it lands on.
    fn copying_seek(&self) -> f64 {
        self.from_seconds + 0.5 / self.fps
    }

    /// The span's frames before `seconds`, and those from it on.
    fn split_at(self, seconds: f64) -> (Self, Self) {
        let before = ((seconds - self.from_seconds) * self.fps)
            .round()
            .clamp(0.0, self.frames as f64) as usize;
        (
            Self {
                frames: before,
                ..self
            },
            Self {
                from_seconds: seconds,
                frames: self.frames - before,
                ..self
            },
        )
    }
}

/// One span of a video file on local disk. A cut reads the file it seeks
/// in, never one over HTTPS: a repository's video holds hundreds of
/// episodes, and every FFmpeg reading it remotely fetched and parsed its
/// index again, seconds per episode against milliseconds from disk.
pub(super) struct CameraCut<'a> {
    video: &'a CameraVideo,
    /// The frames FFmpeg encodes again: all of them, unless the video is
    /// copyable and the span holds a keyframe.
    encoded: FrameSpan,
    /// The frames FFmpeg copies, from the span's first keyframe on.
    copied: Option<FrameSpan>,
}

impl<'a> CameraCut<'a> {
    /// The bitstream filters that make copied H.264 Studio's access units:
    /// the parameter sets, which MP4 holds apart, written before every
    /// keyframe, and every access unit opened by a delimiter.
    const COPY_PACKET_FILTER: &str = "h264_mp4toannexb,h264_metadata=aud=insert";

    pub(super) fn new(video: &'a CameraVideo, span: FrameSpan) -> Self {
        let keyframe = video
            .copyable
            .as_ref()
            .and_then(|copyable| copyable.keyframe_from(span.decoding_seek()));
        let (encoded, copied) = match keyframe {
            Some(keyframe) => {
                let (encoded, copied) = span.split_at(keyframe);
                (encoded, Some(copied))
            }
            None => (span, None),
        };
        Self {
            video,
            encoded,
            copied,
        }
    }

    /// How many frames the cut makes.
    fn frames(&self) -> usize {
        self.encoded.frames + self.copied.map_or(0, |copied| copied.frames)
    }

    /// The FFmpeg runs whose outputs, one after the other, are the cut: the
    /// frames encoded again, then those copied, leaving out a run of no
    /// frames.
    fn runs(&self) -> Vec<Vec<String>> {
        let mut runs = Vec::new();
        if self.encoded.frames > 0 {
            runs.push(self.encoding_arguments());
        }
        if let Some(copied) = self.copied.filter(|copied| copied.frames > 0) {
            runs.push(self.copying_arguments(copied));
        }
        runs
    }

    /// FFmpeg's arguments to read `frames` frames of the video from `seek`.
    fn reading(&self, seek: f64, frames: usize) -> Vec<String> {
        let mut arguments = ["-hide_banner", "-loglevel", "error", "-nostdin"]
            .map(str::to_owned)
            .to_vec();
        arguments.extend([
            "-ss".into(),
            format!("{seek:.6}"),
            "-i".into(),
            self.video.path().display().to_string(),
            "-map".into(),
            "0:v:0".into(),
            "-an".into(),
            "-frames:v".into(),
            frames.to_string(),
        ]);
        arguments
    }

    /// FFmpeg's arguments to decode the encoded frames whatever their codec
    /// and write them as Studio's H.264 to standard output, in the codec of
    /// the copied frames that follow.
    fn encoding_arguments(&self) -> Vec<String> {
        let mut arguments = self.reading(self.encoded.decoding_seek(), self.encoded.frames);
        // One frame out for every frame in, whatever their timestamps.
        arguments.extend(["-fps_mode", "passthrough", "-c:v", "libx264"].map(str::to_owned));
        if let Some(copyable) = &self.video.copyable {
            arguments.extend([
                "-profile:v".into(),
                copyable.profile.into(),
                "-level:v".into(),
                copyable.level.to_string(),
            ]);
        }
        arguments.extend(H264AccessUnit::FFMPEG_OUTPUT_OPTIONS.map(str::to_owned));
        arguments.push("pipe:1".into());
        arguments
    }

    /// FFmpeg's arguments to copy `copied` to standard output as Studio's
    /// H.264, decoding nothing.
    fn copying_arguments(&self, copied: FrameSpan) -> Vec<String> {
        let mut arguments = self.reading(copied.copying_seek(), copied.frames);
        arguments.extend(
            [
                "-c:v",
                "copy",
                "-bsf:v",
                Self::COPY_PACKET_FILTER,
                "-f",
                "h264",
                "pipe:1",
            ]
            .map(str::to_owned),
        );
        arguments
    }
}

impl CameraEncoder {
    pub(super) fn from_environment() -> Self {
        Self {
            executable: FfmpegExecutable::from_environment(),
        }
    }

    /// `file`, a camera's video file on local disk, as ffprobe reads it.
    pub(super) async fn probe(&self, file: tempfile::NamedTempFile) -> Result<CameraVideo, String> {
        let mut arguments = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            CopyableH264::PROBE_ENTRIES,
            "-of",
            "json",
        ]
        .map(str::to_owned)
        .to_vec();
        arguments.push(file.path().display().to_string());
        let printed = Self::output(&self.executable.ffprobe(), arguments).await?;
        let probed = serde_json::from_slice(&printed)
            .map_err(|error| format!("reading ffprobe's output: {error}"))?;
        Ok(CameraVideo {
            copyable: CopyableH264::of(&probed)?,
            file,
        })
    }

    /// `cut`'s frames, exactly one access unit each, the first a keyframe,
    /// all of one codec.
    pub(super) async fn encode(&self, cut: &CameraCut<'_>) -> Result<Vec<Vec<u8>>, String> {
        let program = Path::new(&self.executable);
        let outputs = futures::future::try_join_all(
            cut.runs()
                .into_iter()
                .map(|arguments| Self::output(program, arguments)),
        )
        .await?;
        Self::access_units(&outputs.concat(), cut.frames())
    }

    /// What `program` writes to standard output when run with `arguments`.
    async fn output(program: &Path, arguments: Vec<String>) -> Result<Vec<u8>, String> {
        let name = program.file_name().unwrap_or(program.as_os_str()).display();
        let output = tokio::process::Command::new(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start {}: {error}", program.display()))?
            .wait_with_output()
            .await
            .map_err(|error| format!("{name} failed: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "{name} failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(output.stdout)
    }

    /// `encoded` split at each access unit delimiter, which must make
    /// exactly `frames` access units of Studio's H.264, all of one codec,
    /// since Studio plays an episode's camera in the codec its keyframes
    /// share.
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
        let mut first_codec = None::<String>;
        for (frame, unit) in units.iter().enumerate() {
            let parsed =
                H264AccessUnit::parse(unit).map_err(|error| format!("frame {frame}: {error}"))?;
            if frame == 0 && !parsed.keyframe() {
                return Err("FFmpeg's first frame is no keyframe".into());
            }
            match (&first_codec, parsed.codec()) {
                (None, codec) => first_codec = codec.map(str::to_owned),
                (Some(first), Some(codec)) if first != codec => {
                    return Err(format!("frame {frame} is {codec}, frame 0 {first}"));
                }
                _ => {}
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
        assert!((span.decoding_seek() - (10.1 - 1.0 / 60.0)).abs() < 1e-9);
        assert_eq!(FrameSpan::starting(0.0, 30.0, 892).decoding_seek(), 0.0);
    }

    #[test]
    fn a_span_splits_at_a_keyframe_its_copy_seeks_past() {
        let span = FrameSpan::starting(29.733333, 30.0, 434);
        let (encoded, copied) = span.split_at(30.0);
        assert_eq!(encoded, FrameSpan::starting(29.733333, 30.0, 8));
        assert_eq!(copied, FrameSpan::starting(30.0, 30.0, 426));
        assert!((copied.copying_seek() - (30.0 + 1.0 / 60.0)).abs() < 1e-9);
        // A keyframe past the span leaves nothing to copy.
        assert_eq!(span.split_at(50.0).0, span);
        assert_eq!(span.split_at(50.0).1.frames, 0);
    }

    /// What ffprobe prints of a video of `codec` and `has_b_frames`, a
    /// keyframe every 10 frames.
    fn probed(codec: &str, profile: &str, has_b_frames: u32) -> ProbedVideo {
        let packets = (0..25)
            .map(|frame| {
                let flags = if frame % 10 == 0 { "K__" } else { "___" };
                format!(
                    r#"{{"pts_time": "{:.6}", "flags": "{flags}"}}"#,
                    frame as f64 / 30.0
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        serde_json::from_str(&format!(
            r#"{{"packets": [{packets}], "streams": [{{"codec_name": "{codec}", "profile": "{profile}",
            "has_b_frames": {has_b_frames}, "pix_fmt": "yuv420p", "level": 30}}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn only_h264_without_b_frames_is_copied() {
        let copyable = CopyableH264::of(&probed("h264", "High", 0))
            .unwrap()
            .unwrap();
        assert_eq!((copyable.profile, copyable.level), ("high", 30));
        assert_eq!(copyable.keyframes.len(), 3);
        // ffprobe prints times to the microsecond.
        assert_eq!(copyable.keyframe_from(0.3), Some(0.333333));
        assert_eq!(copyable.keyframe_from(0.7), None);
        assert_eq!(CopyableH264::of(&probed("h264", "High", 2)).unwrap(), None);
        assert_eq!(CopyableH264::of(&probed("av1", "Main", 0)).unwrap(), None);
        assert_eq!(
            CopyableH264::of(&probed("h264", "High 10", 0)).unwrap(),
            None
        );
        assert!(
            CopyableH264::of(&ProbedVideo {
                streams: vec![],
                packets: vec![],
            })
            .is_err()
        );
    }

    fn video(copyable: Option<CopyableH264>) -> CameraVideo {
        CameraVideo {
            file: tempfile::NamedTempFile::new().unwrap(),
            copyable,
        }
    }

    #[test]
    fn ffmpeg_encodes_a_span_whole_unless_it_can_copy_it() {
        let encoded = video(None);
        let path = encoded.path().display().to_string();
        let runs = CameraCut::new(&encoded, FrameSpan::starting(1.0, 30.0, 45)).runs();
        let [run] = runs.as_slice() else {
            panic!("{runs:?}");
        };
        let arguments = run.join(" ");
        assert!(arguments.contains(&format!("-ss 0.983333 -i {path}")));
        assert!(arguments.contains(
            "-frames:v 45 -fps_mode passthrough -c:v libx264 -pix_fmt yuv420p -bf 0 -g 30"
        ));
        assert!(arguments.ends_with("-f h264 pipe:1"));

        let copied = video(Some(CopyableH264 {
            profile: "high",
            level: 30,
            keyframes: vec![0.0, 10.0 / 30.0, 20.0 / 30.0, 1.0, 40.0 / 30.0],
        }));
        let path = copied.path().display().to_string();
        let cut = CameraCut::new(&copied, FrameSpan::starting(25.0 / 30.0, 30.0, 45));
        assert_eq!(cut.frames(), 45);
        let runs = cut
            .runs()
            .iter()
            .map(|run| run.join(" "))
            .collect::<Vec<_>>();
        let [encoding, copying] = runs.as_slice() else {
            panic!("{runs:?}");
        };
        assert!(encoding.contains(&format!("-ss 0.816667 -i {path}")));
        assert!(encoding.contains("-frames:v 5 -fps_mode passthrough -c:v libx264 -profile:v high -level:v 30 -pix_fmt yuv420p"));
        assert!(copying.contains(&format!("-ss 1.016667 -i {path}")));
        assert!(copying.ends_with(
            "-frames:v 40 -c:v copy -bsf:v h264_mp4toannexb,h264_metadata=aud=insert -f h264 pipe:1"
        ));
        // A span starting at a keyframe is copied whole.
        let runs = CameraCut::new(&copied, FrameSpan::starting(1.0, 30.0, 45)).runs();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].join(" ").contains("-frames:v 45 -c:v copy"));
    }

    #[test]
    fn every_frame_is_one_access_unit_of_one_codec_and_the_first_a_keyframe() {
        let delimiter = [0, 0, 0, 1, 9, 0xf0];
        let parameters = [0, 0, 0, 1, 0x67, 0x64, 0, 0x1f, 0, 0, 1, 0x68, 0xee];
        let other_parameters = [0, 0, 0, 1, 0x67, 0x4d, 0x40, 0x1f, 0, 0, 1, 0x68, 0xee];
        let idr = [0, 0, 1, 0x65, 0x88];
        let predicted = [0, 0, 1, 0x41, 0x9a];
        let keyframe = [delimiter.as_slice(), &parameters, &idr].concat();
        let next = [delimiter.as_slice(), &predicted].concat();
        let stream = [keyframe.clone(), next.clone(), next.clone()].concat();
        let units = CameraEncoder::access_units(&stream, 3).unwrap();
        assert_eq!(units, [keyframe.clone(), next.clone(), next.clone()]);
        assert!(CameraEncoder::access_units(&stream, 4).is_err());
        assert!(CameraEncoder::access_units(&[next.clone(), next.clone()].concat(), 2).is_err());
        let other = [delimiter.as_slice(), &other_parameters, &idr].concat();
        assert_eq!(
            CameraEncoder::access_units(&[keyframe, next, other].concat(), 3),
            Err("frame 2 is avc1.4d401f, frame 0 avc1.64001f".into())
        );
    }

    /// A two-second 64×48 video libx264 made with `b_frames` B-frames and a
    /// keyframe every 10 frames, as ffprobe reads it.
    async fn video_with_b_frames(encoder: &CameraEncoder, b_frames: u8) -> CameraVideo {
        let file = tempfile::NamedTempFile::new().unwrap();
        let made = tokio::process::Command::new(&encoder.executable)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x48:rate=30:duration=2",
                "-c:v",
                "libx264",
                "-bf",
                &b_frames.to_string(),
                "-g",
                "10",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "mp4",
                "-y",
            ])
            .arg(file.path())
            .output()
            .await
            .expect("FFmpeg with libx264, as SPIRAL_ROBOTICS_FFMPEG or on PATH");
        assert!(
            made.status.success(),
            "{}",
            String::from_utf8_lossy(&made.stderr)
        );
        encoder.probe(file).await.unwrap()
    }

    /// Each frame's luma, as FFmpeg decodes what it reads with `input`.
    async fn luma_frames(encoder: &CameraEncoder, input: &[String]) -> Vec<Vec<u8>> {
        let decoded = tokio::process::Command::new(&encoder.executable)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(input)
            .args([
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "gray",
                "pipe:1",
            ])
            .output()
            .await
            .unwrap();
        assert!(decoded.status.success());
        decoded.stdout.chunks(64 * 48).map(<[u8]>::to_vec).collect()
    }

    /// Cuts frames out of a video FFmpeg made with B-frames, from a frame
    /// that is no keyframe: FFmpeg encodes them again, a keyframe first.
    #[tokio::test]
    async fn an_episode_starting_between_keyframes_decodes_from_its_first_frame() {
        let encoder = CameraEncoder::from_environment();
        let video = video_with_b_frames(&encoder, 2).await;
        assert_eq!(video.copyable, None);
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

    /// Cuts frames out of H.264 without B-frames from a frame that is no
    /// keyframe, as most episodes of sim-bottles-mjwarp-v1 start: the
    /// frames before its first keyframe are encoded again, and those from
    /// it on are the source's own.
    #[tokio::test]
    async fn an_episode_is_copied_from_its_first_keyframe() {
        let encoder = CameraEncoder::from_environment();
        let video = video_with_b_frames(&encoder, 0).await;
        assert_eq!(
            video
                .copyable
                .as_ref()
                .map(|copyable| copyable.keyframes.len()),
            Some(6)
        );
        let cut = CameraCut::new(&video, FrameSpan::starting(13.0 / 30.0, 30.0, 37));
        let units = encoder.encode(&cut).await.unwrap();
        let keyframes = units
            .iter()
            .enumerate()
            .filter(|(_, unit)| H264AccessUnit::holds_idr(unit))
            .map(|(frame, _)| frame)
            .collect::<Vec<_>>();
        assert_eq!(keyframes, [0, 7, 17, 27]);

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cut.h264");
        std::fs::write(&path, units.concat()).unwrap();
        let cut = luma_frames(&encoder, &["-i".into(), path.display().to_string()]).await;
        let source = luma_frames(
            &encoder,
            &[
                "-ss".into(),
                format!("{:.6}", 12.5 / 30.0),
                "-i".into(),
                video.path().display().to_string(),
                "-frames:v".into(),
                "37".into(),
            ],
        )
        .await;
        assert_eq!(cut.len(), 37);
        assert_eq!(cut[7..], source[7..]);
        // Each frame encoded again is nearer its own source frame than the
        // next one.
        let distance = |left: &[u8], right: &[u8]| {
            left.iter()
                .zip(right)
                .map(|(left, right)| u64::from(left.abs_diff(*right)))
                .sum::<u64>()
        };
        for frame in 0..7 {
            assert!(
                distance(&cut[frame], &source[frame]) < distance(&cut[frame], &source[frame + 1])
            );
        }
    }
}
