//! Studio recording producer: capture buffer, MCAP encoding, and local file publication.

mod capture;
mod episode_schema;
mod mcap_recording;
mod mcap_schema;
mod recording_file;
pub use mcap_recording::CameraRecording;
pub(crate) use mcap_recording::McapRecording;
pub(crate) use recording_file::RecordingFile;
mod identity;

pub use capture::*;
pub use identity::EpisodeId;

#[cfg(test)]
pub(crate) struct EpisodeFixture {
    start: EpisodeStart,
    duration_seconds: f64,
    samples: Vec<Sample>,
}

#[cfg(test)]
impl EpisodeFixture {
    pub(crate) fn new(episode_id: &str, dataset_id: &str) -> Self {
        Self {
            start: EpisodeStart::new(episode_id.to_owned(), dataset_id.to_owned())
                .with_source("transfer-e2e".to_owned()),
            duration_seconds: 0.0,
            samples: Vec::new(),
        }
    }

    pub(crate) fn with_timing(mut self, started_ms: i64, sample_hz: f64) -> Self {
        self.start = self.start.with_timing(started_ms, sample_hz);
        self
    }

    pub(crate) fn with_arms(
        mut self,
        arms: Vec<spiral_robotics_common::recording::EpisodeArm>,
    ) -> Self {
        self.start = self.start.with_arms(arms, Vec::new());
        self
    }

    pub(crate) const fn with_duration_seconds(mut self, duration_seconds: f64) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }

    pub(crate) fn with_samples(mut self, samples: Vec<Sample>) -> Self {
        self.samples = samples;
        self
    }

    pub(crate) fn into_episode(self) -> FullEpisode {
        FullEpisode::from_recording(
            self.start,
            self.samples,
            EpisodeCompletion::new(self.duration_seconds, Vec::new(), Vec::new()),
        )
    }
}
