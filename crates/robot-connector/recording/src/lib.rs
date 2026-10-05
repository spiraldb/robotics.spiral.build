//! Studio's episode recordings: the capture buffer a control loop writes, and
//! the self-contained MCAP file each take becomes. Studio datasets ingest these
//! files unchanged.

mod capture;
mod episode_schema;
mod mcap_recording;
mod mcap_schema;
pub use mcap_recording::{CameraRecording, McapRecording};
mod identity;

include!(concat!(env!("OUT_DIR"), "/storage_schema_version.rs"));

pub use capture::*;
pub use identity::EpisodeId;

/// A finished episode built directly, for tests that need a recording file.
#[cfg(any(test, feature = "test-support"))]
pub struct EpisodeFixture {
    start: EpisodeStart,
    duration_seconds: f64,
    samples: Vec<Sample>,
}

#[cfg(any(test, feature = "test-support"))]
impl EpisodeFixture {
    pub fn new(episode_id: &str, dataset_id: &str) -> Self {
        Self {
            start: EpisodeStart::new(episode_id.to_owned(), dataset_id.to_owned())
                .with_source("transfer-e2e".to_owned()),
            duration_seconds: 0.0,
            samples: Vec::new(),
        }
    }

    pub fn with_timing(mut self, started_ms: i64, sample_hz: f64) -> Self {
        self.start = self.start.with_timing(started_ms, sample_hz);
        self
    }

    pub fn with_arms(mut self, arms: Vec<spiral_robotics_common::recording::EpisodeArm>) -> Self {
        self.start = self.start.with_arms(arms, Vec::new());
        self
    }

    pub const fn with_duration_seconds(mut self, duration_seconds: f64) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }

    pub fn with_samples(mut self, samples: Vec<Sample>) -> Self {
        self.samples = samples;
        self
    }

    pub fn into_episode(self) -> FullEpisode {
        FullEpisode::from_recording(
            self.start,
            self.samples,
            EpisodeCompletion::new(self.duration_seconds, Vec::new(), Vec::new()),
        )
    }
}
