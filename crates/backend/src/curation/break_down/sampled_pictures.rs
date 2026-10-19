//! The pictures Break down samples from one episode's video, decoded by
//! FFmpeg: the episode's access units piped in in decode order, the pictures
//! at the sampled display-order positions selected as FFmpeg puts them in
//! display order, and each written back as a PPM picture, handed on as it
//! arrives, so no more than one decoded picture is held at a time.
use super::BreakdownFailure;
use image::codecs::pnm::PnmDecoder;
use image::{DynamicImage, RgbImage};
use spiral_robotics_storage::FfmpegExecutable;
use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::process::{Command, Stdio};

/// One episode's video, and the display-order positions of the pictures
/// sampled from it.
pub(super) struct SampledPictures {
    /// The episode's Annex B access units, in decode order.
    stream: Vec<u8>,
    /// Ascending.
    positions: Vec<usize>,
}

impl SampledPictures {
    pub(super) fn new(stream: Vec<u8>, positions: Vec<usize>) -> Self {
        Self { stream, positions }
    }

    /// Decode every sampled picture with `ffmpeg` and hand each to `each`,
    /// in order. It waits for FFmpeg, so it runs where blocking is allowed.
    /// FFmpeg that cannot start fails every episode alike; a video it cannot
    /// decode, or a picture `each` refuses, fails this one.
    pub(super) fn decode(
        self,
        ffmpeg: &FfmpegExecutable,
        mut each: impl FnMut(RgbImage) -> Result<(), String>,
    ) -> Result<(), BreakdownFailure> {
        let mut child = Command::new(ffmpeg)
            .args(self.arguments())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                BreakdownFailure::Run(format!("could not start {:?}: {error}", ffmpeg.to_string()))
            })?;
        let (Some(mut input), Some(output), Some(mut said)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(BreakdownFailure::Run(
                "FFmpeg started without its pipes".into(),
            ));
        };
        let stream = self.stream;
        let (handed, exited, said) = std::thread::scope(|scope| {
            // FFmpeg stops reading when it fails, and what it says then is
            // the reason.
            scope.spawn(move || input.write_all(&stream));
            let saying = scope.spawn(move || {
                let mut text = String::new();
                let _ = said.read_to_string(&mut text);
                text
            });
            let mut output = BufReader::new(output);
            let handed = Self::each_picture(&mut output, &mut each);
            if handed.is_err() {
                let _ = child.kill();
            }
            drop(output);
            let exited = child.wait();
            (handed, exited, saying.join().unwrap_or_default())
        });
        let failed = |reason: String| {
            let said = said.trim();
            BreakdownFailure::Episode(if said.is_empty() {
                format!("decoding the sampled pictures failed: {reason}")
            } else {
                format!("decoding the sampled pictures failed: {reason}: {said}")
            })
        };
        handed.map_err(failed)?;
        let status = exited.map_err(|error| failed(error.to_string()))?;
        if !status.success() {
            return Err(failed(format!("FFmpeg exited with {status}")));
        }
        Ok(())
    }

    /// FFmpeg's arguments: raw H.264 from standard input; the sampled
    /// pictures, by their place in display order, one out for each in; and
    /// each written as RGB PPM to standard output.
    fn arguments(&self) -> Vec<String> {
        let selected = self
            .positions
            .iter()
            .map(|position| format!("eq(n\\,{position})"))
            .collect::<Vec<_>>()
            .join("+");
        [
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "h264",
            "-i",
            "pipe:0",
            "-map",
            "0:v:0",
            "-vf",
            &format!("select={selected}"),
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "rgb24",
            "-c:v",
            "ppm",
            "-f",
            "image2pipe",
            "pipe:1",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    /// Hand on every picture FFmpeg writes, until it has written them all.
    fn each_picture(
        output: &mut impl BufRead,
        each: &mut impl FnMut(RgbImage) -> Result<(), String>,
    ) -> Result<(), String> {
        while let Some(picture) = Self::next_picture(output)? {
            each(picture)?;
        }
        Ok(())
    }

    /// The next picture FFmpeg wrote, or none once it is done.
    fn next_picture(output: &mut impl BufRead) -> Result<Option<RgbImage>, String> {
        let unreadable = |error: &dyn std::fmt::Display| format!("reading a picture: {error}");
        if output
            .fill_buf()
            .map_err(|error| unreadable(&error))?
            .is_empty()
        {
            return Ok(None);
        }
        let decoder = PnmDecoder::new(&mut *output).map_err(|error| unreadable(&error))?;
        let picture = DynamicImage::from_decoder(decoder).map_err(|error| unreadable(&error))?;
        Ok(Some(picture.into_rgb8()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curation::break_down::tests::TestVideo;

    #[tokio::test]
    async fn the_sampled_pictures_are_the_videos_own_at_their_display_positions() {
        let video = TestVideo::encode(20).await;
        let ffmpeg = FfmpegExecutable::from_environment();
        let mut sampled = Vec::new();
        SampledPictures::new(video.stream(), vec![0, 7, 19])
            .decode(&ffmpeg, |picture| {
                sampled.push(picture);
                Ok(())
            })
            .unwrap();
        let every = video.every_picture().await;
        assert_eq!(sampled.len(), 3);
        for (picture, position) in sampled.iter().zip([0, 7, 19]) {
            assert_eq!(picture, &every[position], "picture {position}");
        }
        assert_ne!(every[0], every[7], "the pictures differ");
    }
}
