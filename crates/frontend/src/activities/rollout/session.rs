//! State owned by the mounted Rollout workspace and shared by its panels.
use super::RecordingClient;
use super::next_episode::{ControlKind, NextEpisode};
use super::policy::{PolicyControl, PolicyReadiness};
use super::recording_client::EpisodeStorage;
use super::signals::SignalStore;
use super::transport::TimelineController;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::robots::{HardwareCommands, HardwareViewState, RobotSelection};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;

/// The episode Record starts next, and the finished ones waiting to be stored.
#[derive(Clone, Copy, PartialEq)]
struct RolloutEpisodes {
    next: NextEpisode,
    storage: EpisodeStorage,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct RolloutSession {
    project: StudioProjectState,
    telemetry: HardwareViewState,
    selection: RobotSelection,
    episodes: RolloutEpisodes,
    policy: PolicyControl,
    timeline: TimelineController,
    recording_signals: Signal<SignalStore>,
}
impl RolloutSession {
    pub(super) fn use_session(
        project: StudioProjectState,
        telemetry: HardwareViewState,
        selection: RobotSelection,
    ) -> Self {
        let is_active = telemetry.is_active_signal();
        let episodes = RolloutEpisodes {
            next: NextEpisode::use_next_episode(),
            storage: EpisodeStorage::use_storage(project, is_active),
        };
        let policy = PolicyControl::use_control(is_active);
        let timeline = TimelineController::use_controller();
        let recording_signals = use_signal(SignalStore::default);
        let session = use_hook(|| Self {
            project,
            telemetry,
            selection,
            episodes,
            policy,
            timeline,
            recording_signals,
        });
        use_effect(move || {
            if !is_active() {
                timeline.stop_preview();
            }
        });
        let recording_present = use_memo(move || telemetry.recording().is_some());
        let request = QueryPolicy::new(QueryDomain::Recording, 200).use_when(
            move || is_active() && recording_present(),
            move || RecordingClient::current().recording_window(recording_signals.peek().cursor()),
        );
        let mut recording_signals = recording_signals;
        use_effect(move || {
            let response = request.read();
            if request.finished()
                && let Some(Ok(window)) = response.as_ref()
                && window.changes(&recording_signals.peek())
            {
                recording_signals.write().apply_window(window.clone());
            }
        });
        session
    }
    pub(super) fn project(self) -> StudioProjectState {
        self.project
    }
    pub(super) fn telemetry(self) -> HardwareViewState {
        self.telemetry
    }
    pub(super) fn selection(self) -> RobotSelection {
        self.selection
    }
    pub(super) fn commands(self) -> HardwareCommands {
        HardwareCommands::new(self.project, self.telemetry)
    }
    pub(super) fn next_episode(self) -> NextEpisode {
        self.episodes.next
    }
    pub(super) fn episode_storage(self) -> EpisodeStorage {
        self.episodes.storage
    }
    pub(super) fn policy(self) -> PolicyControl {
        self.policy
    }
    pub(super) fn timeline_controller(self) -> TimelineController {
        self.timeline
    }
    pub(super) fn recording_signals(self) -> MappedSignal<SignalStore, Signal<SignalStore>> {
        self.recording_signals.map(|value| value)
    }

    /// What Record starts next: the control source the operator last started,
    /// or, before they have, Teleop on a bench with a leader and Policy on one
    /// without.
    pub(super) fn record_control(self) -> ControlKind {
        let teleop_ready = self
            .telemetry
            .snapshot()
            .read()
            .default_teleoperation_pair()
            .is_some();
        let default = if teleop_ready || self.policy.selected().is_none() {
            ControlKind::Teleop
        } else {
            ControlKind::Policy
        };
        self.episodes.next.control(default)
    }

    /// Why the chosen checkpoint cannot drive the follower now, if it cannot.
    pub(super) fn policy_blocker(self) -> Option<String> {
        let setup = self.telemetry.snapshot();
        let setup = setup.read();
        PolicyReadiness::new(
            self.policy.selected().as_ref(),
            &setup,
            self.telemetry.controls_available(),
        )
        .blocker()
        .or_else(|| {
            self.policy.duration_seconds().is_none().then(|| {
                format!(
                    "Set a whole-number duration from 1 to {} seconds.",
                    PolicyControl::MAX_DURATION_SECONDS
                )
            })
        })
    }
}
