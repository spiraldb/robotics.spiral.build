//! Admission of hardware commands. Stop/release actions use the project's safety path.
use super::HardwareViewState;
use crate::shell::state::StudioProjectState;
use std::future::Future;

#[derive(Clone, Copy)]
pub(crate) struct HardwareCommands {
    project: StudioProjectState,
    telemetry: HardwareViewState,
}
impl HardwareCommands {
    pub(crate) fn new(project: StudioProjectState, telemetry: HardwareViewState) -> Self {
        Self { project, telemetry }
    }
    pub(crate) fn run(
        self,
        success: impl Into<String>,
        future: impl Future<Output = Result<(), String>> + 'static,
    ) {
        if !self.telemetry.controls_available() {
            self.project.report("Controls are paused because telemetry is unavailable or delayed. No command was sent.");
            return;
        }
        self.project.run_utility(success, future);
    }
}
