//! State owned by the mounted Live workspace and shared by its panels.
use super::RecordingClient;
use super::signals::SignalStore;
use super::transport::TimelineController;
use super::workbench::CaptureDestination;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::robots::{HardwareCommands, HardwareViewState, RobotSelection};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct LiveSession {
    project: StudioProjectState,
    telemetry: HardwareViewState,
    selection: RobotSelection,
    capture: CaptureDestination,
    timeline: TimelineController,
    recording_signals: Signal<SignalStore>,
}
impl LiveSession {
    pub(super) fn use_session(
        project: StudioProjectState,
        telemetry: HardwareViewState,
        selection: RobotSelection,
    ) -> Self {
        let is_active = telemetry.is_active_signal();
        let capture = CaptureDestination::use_destination();
        let timeline = TimelineController::use_controller();
        let recording_signals = use_signal(SignalStore::default);
        let session = use_hook(|| Self {
            project,
            telemetry,
            selection,
            capture,
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
    pub(super) fn capture(self) -> CaptureDestination {
        self.capture
    }
    pub(super) fn timeline_controller(self) -> TimelineController {
        self.timeline
    }
    pub(super) fn recording_signals(self) -> MappedSignal<SignalStore, Signal<SignalStore>> {
        self.recording_signals.map(|value| value)
    }
}
