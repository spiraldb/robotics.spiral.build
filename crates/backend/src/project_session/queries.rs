//! Read-only projections of one immutable project session.
use crate::{BackendCommandError, project_session::context::ProjectContext};
use spiral_robotics_common::project::ProjectStatus;
use spiral_robotics_common::robots::TelemetryFrame;
use spiral_robotics_common::status::StudioSnapshot;

#[derive(Clone)]
pub(crate) struct ProjectQueries {
    state: ProjectContext,
}
impl ProjectQueries {
    pub(crate) fn status(&self) -> ProjectStatus {
        let project = self.state.projects().active();
        ProjectStatus::new(project.info().clone(), project.readiness().current())
    }
    #[cfg(test)]
    fn for_session(&self, id: &str) -> Result<Self, BackendCommandError> {
        Ok(Self::from(self.state.for_session(id)?))
    }
    pub(crate) async fn setup(&self) -> Result<StudioSnapshot, BackendCommandError> {
        let state = self.state.clone();
        let activity = state.admit()?;
        self.state
            .tasks()
            .read(async move {
                let _activity = activity;
                crate::project_session::setup::ProjectSetup::new(&state)
                    .snapshot()
                    .await
            })
            .await
            .map_err(Into::into)
    }
    pub(crate) fn telemetry(&self) -> Result<TelemetryFrame, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.hardware().state().telemetry().clone())
    }
}

impl From<crate::project_session::context::ProjectContext> for ProjectQueries {
    fn from(state: crate::project_session::context::ProjectContext) -> Self {
        Self { state }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_session::tests::ProjectFixture;
    #[tokio::test]
    async fn reads_are_bound_to_one_session_and_status_tracks_readiness_without_catalog_io() {
        let fixture = ProjectFixture::new();
        let queries = ProjectQueries::from(ProjectContext::new(fixture.projects.clone()));
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
            crate::recording::commands::RecordingCommands::from(scoped.state.clone())
                .recording_signals(
                    spiral_robotics_common::recording_signals::RecordingCursor::default()
                )
                .is_err()
        );
        assert!(
            crate::training::commands::TrainingCommands::from(scoped.state.clone())
                .snapshot(None)
                .is_err()
        );
        assert!(
            crate::rollout::commands::RolloutCommands::from(scoped.state.clone())
                .snapshot()
                .is_err()
        );
        fixture.projects.hardware().shutdown().await.unwrap();
    }
}
