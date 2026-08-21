//! Product-level activity, projection, and data-source contracts.
//!
//! These types describe behavioral ownership rather than Dioxus layout. The
//! Live and Data workbenches emit semantic intent; one controller derives an
//! [`ActivityProjection`]; renderers consume that projection without guessing
//! whether absent data means Live, loading, or failure.

use spiral_robotics_contracts::api::{EpisodeSummary, RecordingProgress};

/// The Studio's five primary activities.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Activity {
    #[default]
    Live,
    Data,
    Workflow,
    Training,
    Rollout,
}

impl Activity {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Data => "Data",
            Self::Workflow => "Workflow",
            Self::Training => "Training",
            Self::Rollout => "Rollout",
        }
    }
}

/// The semantic subject of the canonical Timeline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TimelineSubject {
    Empty {
        activity: Activity,
        reason: String,
    },
    Recording(RecordingProgress),
    Episode {
        activity: Activity,
        episode: EpisodeSummary,
    },
}

/// Stable identity carried by every time operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TimelineTarget {
    Recording(String),
    Episode(String),
}

impl TimelineSubject {
    pub(crate) fn empty(activity: Activity) -> Self {
        let reason = match activity {
            Activity::Live => "Record an episode to begin using the shared clock.",
            Activity::Data => "Select an episode to inspect its shared clock.",
            Activity::Workflow => {
                "Workflow lineage uses Spiral relations instead of the episode clock."
            }
            Activity::Training => {
                "Training metrics use optimization steps instead of the episode clock."
            }
            Activity::Rollout => {
                "Rollout inference uses live observations instead of the episode clock."
            }
        };
        Self::Empty {
            activity,
            reason: reason.to_owned(),
        }
    }

    pub(crate) const fn activity(&self) -> Activity {
        match self {
            Self::Empty { activity, .. } | Self::Episode { activity, .. } => *activity,
            Self::Recording(_) => Activity::Live,
        }
    }

    pub(crate) fn episode_id(&self) -> Option<&str> {
        match self {
            Self::Empty { .. } => None,
            Self::Recording(progress) => Some(&progress.episode),
            Self::Episode { episode, .. } => Some(&episode.id),
        }
    }

    pub(crate) fn target(&self) -> Option<TimelineTarget> {
        match self {
            Self::Empty { .. } => None,
            Self::Recording(progress) => Some(TimelineTarget::Recording(progress.episode.clone())),
            Self::Episode { episode, .. } => Some(TimelineTarget::Episode(episode.id.clone())),
        }
    }

    pub(crate) fn episode(&self) -> Option<&EpisodeSummary> {
        match self {
            Self::Episode { episode, .. } => Some(episode),
            Self::Empty { .. } | Self::Recording(_) => None,
        }
    }

    pub(crate) fn duration_seconds(&self) -> f64 {
        match self {
            Self::Empty { .. } => 0.0,
            Self::Recording(progress) => progress.elapsed_seconds,
            Self::Episode { episode, .. } => episode.duration_seconds,
        }
        .max(0.0)
    }

    pub(crate) fn frames(&self) -> u64 {
        match self {
            Self::Empty { .. } => 0,
            Self::Recording(progress) => progress.frames,
            Self::Episode { episode, .. } => episode.frames,
        }
    }

    pub(crate) fn recording(&self) -> Option<&RecordingProgress> {
        match self {
            Self::Recording(progress) => Some(progress),
            Self::Empty { .. } | Self::Episode { .. } => None,
        }
    }

    pub(crate) fn is_paused_recording(&self) -> bool {
        self.recording().is_some_and(|progress| progress.paused)
    }

    pub(crate) fn available(&self) -> bool {
        match self {
            Self::Empty { .. } => false,
            Self::Recording(progress) => progress.error.is_none(),
            Self::Episode { episode, .. } => episode.usable(),
        }
    }

    pub(crate) fn problem(&self) -> Option<&str> {
        match self {
            Self::Empty { reason, .. } => Some(reason),
            Self::Recording(progress) => progress.error.as_deref(),
            Self::Episode { episode, .. } => episode.problem.as_deref(),
        }
    }

    /// The episode whose recorded data should be loaded for the current view.
    pub(crate) fn recorded_episode(&self) -> Option<&str> {
        match self {
            Self::Episode { episode, .. } => Some(&episode.id),
            Self::Recording(progress) if progress.paused => Some(&progress.episode),
            Self::Empty { .. } | Self::Recording(_) => None,
        }
    }
}

/// What belongs in the primary visualization panel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PrimaryVisualization {
    LiveScene,
    RecordedScene(Box<EpisodeSummary>),
    EmptyEpisodes,
}

/// Which semantic source owns the activity's camera registry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CameraRegistrySource {
    LiveAssignments,
    RecordedEpisode(Box<EpisodeSummary>),
    None,
}

/// One coherent rendering consequence for the active semantic state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActivityProjection {
    pub(crate) activity: Activity,
    pub(crate) primary: PrimaryVisualization,
    pub(crate) cameras: CameraRegistrySource,
    pub(crate) timeline: TimelineSubject,
}

impl ActivityProjection {
    pub(crate) fn derive(
        activity: Activity,
        recording: Option<RecordingProgress>,
        data_episode_context: Option<EpisodeSummary>,
    ) -> Self {
        match activity {
            Activity::Live => {
                let timeline = recording
                    .map(TimelineSubject::Recording)
                    .unwrap_or_else(|| TimelineSubject::empty(activity));
                Self {
                    activity,
                    primary: PrimaryVisualization::LiveScene,
                    cameras: CameraRegistrySource::LiveAssignments,
                    timeline,
                }
            }
            Activity::Data => match data_episode_context {
                Some(episode) => Self {
                    activity,
                    primary: PrimaryVisualization::RecordedScene(Box::new(episode.clone())),
                    cameras: CameraRegistrySource::RecordedEpisode(Box::new(episode.clone())),
                    timeline: TimelineSubject::Episode { activity, episode },
                },
                None => Self {
                    activity,
                    primary: PrimaryVisualization::EmptyEpisodes,
                    cameras: CameraRegistrySource::None,
                    timeline: TimelineSubject::empty(activity),
                },
            },
            Activity::Training => Self {
                activity,
                primary: PrimaryVisualization::EmptyEpisodes,
                cameras: CameraRegistrySource::None,
                timeline: TimelineSubject::empty(activity),
            },
            Activity::Workflow => Self {
                activity,
                primary: PrimaryVisualization::EmptyEpisodes,
                cameras: CameraRegistrySource::None,
                timeline: TimelineSubject::empty(activity),
            },
            Activity::Rollout => Self {
                activity,
                primary: PrimaryVisualization::LiveScene,
                cameras: CameraRegistrySource::LiveAssignments,
                timeline: TimelineSubject::empty(activity),
            },
        }
    }
}

/// Explicit recorded-source lifecycle. `previous` may preserve the last frame
/// of the same episode while the next time sample loads, but it never carries a
/// Live value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LoadState<T> {
    Loading { previous: Option<T> },
    Ready(T),
    Unavailable { reason: String },
    Failed { reason: String, previous: Option<T> },
}

impl<T> LoadState<T> {
    pub(crate) fn renderable(&self) -> Option<&T> {
        match self {
            Self::Loading { previous } | Self::Failed { previous, .. } => previous.as_ref(),
            Self::Ready(value) => Some(value),
            Self::Unavailable { .. } => None,
        }
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Loading { .. } | Self::Ready(_) => None,
            Self::Unavailable { reason } | Self::Failed { reason, .. } => Some(reason),
        }
    }

    pub(crate) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }

    pub(crate) fn map<U>(self, mut transform: impl FnMut(T) -> U) -> LoadState<U> {
        match self {
            Self::Loading { previous } => LoadState::Loading {
                previous: previous.map(&mut transform),
            },
            Self::Ready(value) => LoadState::Ready(transform(value)),
            Self::Unavailable { reason } => LoadState::Unavailable { reason },
            Self::Failed { reason, previous } => LoadState::Failed {
                reason,
                previous: previous.map(transform),
            },
        }
    }
}

/// A recorded value tied to the identity and time it was requested for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecordedSource<T> {
    pub(crate) episode: String,
    pub(crate) seconds: f64,
    pub(crate) state: LoadState<T>,
}

impl<T> RecordedSource<T> {
    pub(crate) fn renderable(&self) -> Option<&T> {
        self.state.renderable()
    }

    pub(crate) fn map<U>(self, transform: impl FnMut(T) -> U) -> RecordedSource<U> {
        RecordedSource {
            episode: self.episode,
            seconds: self.seconds,
            state: self.state.map(transform),
        }
    }
}

/// Domain identity associated with a camera panel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CameraIdentity {
    Live {
        robot: String,
        role: String,
        device: String,
    },
    Recorded {
        episode: String,
        stream: String,
        label: String,
    },
}

/// Studio meaning associated with a generic panel id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PanelRole {
    Workbench,
    PrimaryVisualization,
    Camera(CameraIdentity),
    Timeline,
    RolloutInference,
    Placeholder,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(id: &str) -> EpisodeSummary {
        EpisodeSummary {
            dataset_id: "default".to_owned(),
            id: id.to_owned(),
            arms: Vec::new(),
            frames: 120,
            duration_seconds: 2.0,
            recording_gaps: Vec::new(),
            sample_hz: 60.0,
            started_ms: 0,
            source: "Teleoperation".to_owned(),
            max_follow_error_degrees: None,
            discarded: false,
            problem: None,
        }
    }

    #[test]
    fn an_episode_selection_derives_scene_cameras_and_timeline_together() {
        let selected = episode("episode-42");
        let projection = ActivityProjection::derive(Activity::Data, None, Some(selected.clone()));

        assert_eq!(
            projection.primary,
            PrimaryVisualization::RecordedScene(Box::new(selected.clone()))
        );
        assert_eq!(
            projection.cameras,
            CameraRegistrySource::RecordedEpisode(Box::new(selected.clone()))
        );
        assert_eq!(projection.timeline.episode_id(), Some(selected.id.as_str()));
    }

    #[test]
    fn query_state_does_not_replace_episode_context() {
        let selected = episode("episode-7");
        let projection = ActivityProjection::derive(Activity::Data, None, Some(selected.clone()));

        assert_eq!(
            projection.primary,
            PrimaryVisualization::RecordedScene(Box::new(selected.clone()))
        );
        assert_eq!(
            projection.cameras,
            CameraRegistrySource::RecordedEpisode(Box::new(selected.clone()))
        );
        assert_eq!(projection.timeline.episode_id(), Some(selected.id.as_str()));
    }

    #[test]
    fn an_empty_data_activity_has_one_explicit_empty_timeline_subject() {
        let projection = ActivityProjection::derive(Activity::Data, None, None);
        assert!(matches!(
            projection.timeline,
            TimelineSubject::Empty {
                activity: Activity::Data,
                ..
            }
        ));
        assert_eq!(projection.cameras, CameraRegistrySource::None);
    }

    #[test]
    fn rollout_projects_live_scene_and_follower_camera_context() {
        let projection = ActivityProjection::derive(Activity::Rollout, None, None);
        assert_eq!(projection.primary, PrimaryVisualization::LiveScene);
        assert_eq!(projection.cameras, CameraRegistrySource::LiveAssignments);
        assert!(matches!(
            projection.timeline,
            TimelineSubject::Empty {
                activity: Activity::Rollout,
                ..
            }
        ));
    }

    #[test]
    fn workflow_is_a_relation_workspace_without_episode_projection() {
        let projection = ActivityProjection::derive(Activity::Workflow, None, None);
        assert_eq!(projection.activity, Activity::Workflow);
        assert_eq!(projection.primary, PrimaryVisualization::EmptyEpisodes);
        assert_eq!(projection.cameras, CameraRegistrySource::None);
        assert!(matches!(
            projection.timeline,
            TimelineSubject::Empty {
                activity: Activity::Workflow,
                ..
            }
        ));
    }
}
