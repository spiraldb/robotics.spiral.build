//! The FFmpeg executable Studio runs: `SPIRAL_ROBOTICS_FFMPEG`, or `ffmpeg`
//! on `PATH`, and the ffprobe beside it. A LeRobot import probes and cuts its
//! cameras with them, and Break down decodes the pictures it samples with it.
use std::ffi::OsStr;
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};

/// The FFmpeg executable this process runs, as a command's program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfmpegExecutable(String);

impl FfmpegExecutable {
    const VARIABLE: &str = "SPIRAL_ROBOTICS_FFMPEG";

    pub fn from_environment() -> Self {
        Self(
            std::env::var(Self::VARIABLE)
                .ok()
                .filter(|executable| !executable.trim().is_empty())
                .unwrap_or_else(|| "ffmpeg".to_owned()),
        )
    }

    /// The ffprobe FFmpeg installs beside `ffmpeg`: in the same directory,
    /// or on `PATH` when `ffmpeg` is.
    pub(crate) fn ffprobe(&self) -> PathBuf {
        Path::new(&self.0).with_file_name("ffprobe")
    }
}

impl AsRef<OsStr> for FfmpegExecutable {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(&self.0)
    }
}

impl Display for FfmpegExecutable {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
