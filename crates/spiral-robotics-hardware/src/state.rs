//! Hardware-only published-state values.

/// Narrow durable result returned to the hardware thread after storage commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedRecording {
    episode: String,
    frames: u64,
}

impl CompletedRecording {
    pub fn new(episode: impl Into<String>, frames: u64) -> Self {
        Self {
            episode: episode.into(),
            frames,
        }
    }

    pub fn episode(&self) -> &str {
        &self.episode
    }

    pub(crate) const fn frames(&self) -> u64 {
        self.frames
    }
}
