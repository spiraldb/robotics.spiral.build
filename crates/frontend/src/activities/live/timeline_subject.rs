//! Timeline subjects describe content independently of the activity displaying it.

use spiral_robotics_common::recording::{EpisodeInfo, RecordingProgress};

/// The semantic subject of the canonical Timeline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TimelineSubject {
    Empty { label: String, reason: String },
    Recording(RecordingProgress),
    Episode { episode: EpisodeInfo },
}

/// Stable identity carried by every time operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TimelineTarget {
    Recording(String),
    Episode(String),
}

impl TimelineSubject {
    pub(crate) fn empty(label: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Empty {
            label: label.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn episode_id(&self) -> Option<&str> {
        match self {
            Self::Empty { .. } => None,
            Self::Recording(progress) => Some(progress.episode()),
            Self::Episode { episode, .. } => Some(episode.id()),
        }
    }

    pub(crate) fn target(&self) -> Option<TimelineTarget> {
        match self {
            Self::Empty { .. } => None,
            Self::Recording(progress) => {
                Some(TimelineTarget::Recording(progress.episode().to_owned()))
            }
            Self::Episode { episode, .. } => Some(TimelineTarget::Episode(episode.id().to_owned())),
        }
    }

    pub(crate) fn episode(&self) -> Option<&EpisodeInfo> {
        match self {
            Self::Episode { episode, .. } => Some(episode),
            Self::Empty { .. } | Self::Recording(_) => None,
        }
    }

    pub(crate) fn duration_seconds(&self) -> f64 {
        match self {
            Self::Empty { .. } => 0.0,
            Self::Recording(progress) => progress.elapsed_seconds(),
            Self::Episode { episode, .. } => episode.duration_seconds(),
        }
        .max(0.0)
    }

    pub(crate) fn frames(&self) -> u64 {
        match self {
            Self::Empty { .. } => 0,
            Self::Recording(progress) => progress.frames(),
            Self::Episode { episode, .. } => episode.frames(),
        }
    }

    pub(crate) fn recording(&self) -> Option<&RecordingProgress> {
        match self {
            Self::Recording(progress) => Some(progress),
            Self::Empty { .. } | Self::Episode { .. } => None,
        }
    }

    pub(crate) fn is_paused_recording(&self) -> bool {
        self.recording().is_some_and(|progress| progress.paused())
    }

    pub(crate) fn available(&self) -> bool {
        match self {
            Self::Empty { .. } => false,
            Self::Recording(progress) => progress.error().is_none(),
            Self::Episode { episode, .. } => episode.usable(),
        }
    }

    pub(crate) fn problem(&self) -> Option<&str> {
        match self {
            Self::Empty { reason, .. } => Some(reason),
            Self::Recording(progress) => progress.error(),
            Self::Episode { episode, .. } => episode.problem(),
        }
    }

    /// The episode whose recorded data should be loaded for the current view.
    pub(crate) fn recorded_episode(&self) -> Option<&str> {
        match self {
            Self::Episode { episode, .. } => Some(episode.id()),
            Self::Recording(progress) if progress.paused() => Some(progress.episode()),
            Self::Empty { .. } | Self::Recording(_) => None,
        }
    }
}

impl From<EpisodeInfo> for TimelineSubject {
    fn from(episode: EpisodeInfo) -> Self {
        Self::Episode { episode }
    }
}

impl TimelineTarget {
    pub(crate) fn episode(&self) -> &str {
        match self {
            Self::Recording(episode) | Self::Episode(episode) => episode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::recording::{RecordingClock, RecordingIdentity, RecordingState};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn capture_pause_and_resume_change_media_source_without_changing_transport_identity() {
        let recording = |paused| {
            TimelineSubject::Recording(RecordingProgress::new(
                RecordingIdentity::new("take-1", "default", vec!["follower".to_owned()]),
                RecordingClock::new(60, 1.0, 2.0),
                RecordingState::new(paused, None, None),
            ))
        };
        let active = recording(false);
        let paused = recording(true);
        assert_eq!(active.target(), paused.target());
        assert_eq!(active.recorded_episode(), None);
        assert_eq!(paused.recorded_episode(), Some("take-1"));
        assert_eq!(paused.duration_seconds(), 1.0);
        assert_eq!(paused.frames(), 60);
        let completed =
            TimelineSubject::from(EpisodeInfo::new("take-1", "default").with_timeline(60, 1.0));
        assert_eq!(completed.recorded_episode(), Some("take-1"));
        assert_ne!(
            completed.target(),
            paused.target(),
            "finalization must disarm recording-scoped robot synchronization"
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_empty_timeline_cannot_be_armed_or_replayed() {
        let subject = TimelineSubject::empty("Inspection", "Select a recording.");
        assert!(subject.target().is_none());
        assert!(subject.recorded_episode().is_none());
        assert!(!subject.available());
        assert_eq!(subject.problem(), Some("Select a recording."));
    }
}
