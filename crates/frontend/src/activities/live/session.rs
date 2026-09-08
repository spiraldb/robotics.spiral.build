//! State owned by the mounted Live workspace and shared by its panels.
use super::RecordingClient;
use crate::activities::data::dataset_catalog::DatasetChoices;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::panels::timeline::{signals::SignalStore, state::TimelineController};
use crate::shell::robot_workspace::RobotWorkspace;
use dioxus::prelude::*;

#[derive(Clone, Copy)]
pub(crate) struct LiveSession {
    robot: RobotWorkspace,
    datasets: DatasetChoices,
    timeline: TimelineController,
    recording_signals: Signal<SignalStore>,
}
impl std::ops::Deref for LiveSession {
    type Target = RobotWorkspace;
    fn deref(&self) -> &Self::Target {
        &self.robot
    }
}
impl LiveSession {
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn use_provider(is_active: ReadSignal<bool>) -> Self {
        let robot = RobotWorkspace::use_provider(is_active);
        let datasets = DatasetChoices::use_provider();
        let timeline = TimelineController::use_controller();
        let recording_signals = use_signal(SignalStore::default);
        let session = use_context_provider(|| Self {
            robot,
            datasets,
            timeline,
            recording_signals,
        });
        use_effect(move || {
            if !is_active() {
                timeline.stop_preview();
            }
        });
        let recording_present = use_memo(move || robot.recording().is_some());
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
    pub(crate) fn robot(self) -> RobotWorkspace {
        self.robot
    }
    pub(crate) fn selected_dataset(self) -> Signal<String> {
        self.datasets.selected()
    }
    pub(crate) fn timeline_controller(self) -> TimelineController {
        self.timeline
    }
    pub(crate) fn recording_signals(self) -> MappedSignal<SignalStore, Signal<SignalStore>> {
        self.recording_signals.map(|value| value)
    }
}
