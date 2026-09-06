//! Read-only projections of one immutable project session.
use crate::{BackendCommandError, http::routes::AppState, main_runtime::MainRuntime};
use spiral_robotics_common::project::ProjectStatus;
use spiral_robotics_common::recording_signals::{RecordingCursor, RecordingWindow};
use spiral_robotics_common::robots::TelemetryFrame;
use spiral_robotics_common::rollout::RolloutSnapshot;
use spiral_robotics_common::status::StudioSnapshot;
use spiral_robotics_common::training::TrainingSnapshot;

#[derive(Clone)]
pub struct ProjectQueries {
    state: AppState,
    runtime: MainRuntime,
}
impl ProjectQueries {
    pub(crate) fn new(state: AppState, runtime: MainRuntime) -> Self {
        Self { state, runtime }
    }
    pub fn status(&self) -> ProjectStatus {
        let project = self.state.projects().active();
        ProjectStatus::new(project.info().clone(), project.readiness().current())
    }
    pub fn for_session(&self, id: &str) -> Result<Self, BackendCommandError> {
        Ok(Self::new(self.state.for_session(id)?, self.runtime.clone()))
    }
    pub async fn setup(&self) -> Result<StudioSnapshot, BackendCommandError> {
        let state = self.state.clone();
        let activity = state.admit()?;
        self.runtime
            .read(async move {
                let _activity = activity;
                state.snapshot().await
            })
            .await
            .map_err(Into::into)
    }
    pub fn telemetry(&self) -> Result<TelemetryFrame, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.hardware().state().telemetry().clone())
    }
    pub fn recording_signals(
        &self,
        cursor: RecordingCursor,
    ) -> Result<RecordingWindow, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.hardware().recording_signals(cursor))
    }
    pub fn training(
        &self,
        selected: Option<String>,
    ) -> Result<TrainingSnapshot, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.training().display_snapshot(selected.as_deref()))
    }
    pub fn rollout(&self) -> Result<RolloutSnapshot, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.rollout().snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_session::tests::ProjectFixture;
    #[tokio::test]
    async fn reads_are_bound_to_one_session_and_status_tracks_readiness_without_catalog_io() {
        let fixture = ProjectFixture::new();
        let queries = ProjectQueries::new(
            AppState::new(fixture.projects.clone()),
            MainRuntime::current(),
        );
        let first = queries.status();
        let scoped = queries.for_session(first.session().id()).unwrap();
        fixture
            .projects
            .active()
            .readiness()
            .bootstrapping("Changing project");
        let pending = queries.status();
        assert_eq!(pending.session(), first.session());
        assert!(!pending.readiness().catalog_available());
        assert!(scoped.setup().await.is_ok());
        let next = fixture.replace("project_b");
        assert_eq!(queries.status().session(), next.info());
        assert!(queries.for_session(first.session().id()).is_err());
        assert!(scoped.telemetry().is_err());
        assert!(
            scoped
                .recording_signals(RecordingCursor::default())
                .is_err()
        );
        assert!(scoped.training(None).is_err());
        assert!(scoped.rollout().is_err());
        fixture.projects.hardware().shutdown().await.unwrap();
    }
}
