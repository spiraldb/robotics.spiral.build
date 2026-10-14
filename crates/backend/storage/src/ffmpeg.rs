//! The FFmpeg executable Studio runs: `SPIRAL_ROBOTICS_FFMPEG`, or `ffmpeg`
//! on `PATH`. A LeRobot import encodes its cameras with it, and Break down
//! decodes the pictures it samples with it.
use std::ffi::OsStr;
use std::fmt::{self, Display, Formatter};

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
