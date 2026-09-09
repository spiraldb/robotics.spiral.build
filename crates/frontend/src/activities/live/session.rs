//! State owned by the mounted Live workspace and shared by its panels.
use super::RecordingClient;
use crate::activities::data::dataset_catalog::DatasetChoices;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::panels::timeline::{signals::SignalStore, state::TimelineController};
use crate::robots::{HardwareCommands, HardwareViewState, RobotSelection};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct LiveSession {
    project: StudioProjectState,
    telemetry: HardwareViewState,
    selection: RobotSelection,
    datasets: DatasetChoices,
    timeline: TimelineController,
    recording_signals: Signal<SignalStore>,
}
impl LiveSession {
    pub(crate) fn use_session(
        project: StudioProjectState,
        telemetry: HardwareViewState,
        selection: RobotSelection,
    ) -> Self {
        let is_active = telemetry.is_active_signal();
        let datasets = DatasetChoices::use_choices();
        let timeline = TimelineController::use_controller();
        let recording_signals = use_signal(SignalStore::default);
        let session = use_hook(|| Self {
            project,
            telemetry,
            selection,
            datasets,
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
    pub(crate) fn project(self) -> StudioProjectState {
        self.project
    }
    pub(crate) fn telemetry(self) -> HardwareViewState {
        self.telemetry
    }
    pub(crate) fn selection(self) -> RobotSelection {
        self.selection
    }
    pub(crate) fn commands(self) -> HardwareCommands {
        HardwareCommands::new(self.project, self.telemetry)
    }
    pub(crate) fn datasets(self) -> DatasetChoices {
        self.datasets
    }
    pub(crate) fn timeline_controller(self) -> TimelineController {
        self.timeline
    }
    pub(crate) fn recording_signals(self) -> MappedSignal<SignalStore, Signal<SignalStore>> {
        self.recording_signals.map(|value| value)
    }
}
