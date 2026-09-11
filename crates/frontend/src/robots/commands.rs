//! Admission of hardware commands. Stop/release actions use the project's safety path.
use super::HardwareViewState;
use crate::shell::state::StudioProjectState;
use dioxus::dioxus_core::CapturedError;
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
        let success = success.into();
        crate::client::project::ProjectClient::current().spawn(async move {
            let _ = self.execute(success, future).await;
        });
    }

    pub(crate) async fn execute(
        self,
        success: impl Into<String>,
        future: impl Future<Output = Result<(), String>>,
    ) -> Result<(), CapturedError> {
        if !self.telemetry.controls_available() {
            let message = "Controls are paused because telemetry is unavailable or delayed. No command was sent.";
            self.project.report(message);
            return Err(CapturedError::from_display(message));
        }
        self.project.execute_utility(success, future).await
    }
}
