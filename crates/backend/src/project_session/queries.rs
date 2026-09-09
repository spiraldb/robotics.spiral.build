//! Read-only projections of one immutable project session.
use crate::project_session::context::ProjectContext;
use spiral_robotics_common::project::ProjectStatus;

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
    fn for_session(&self, id: &str) -> Result<Self, crate::BackendCommandError> {
        Ok(Self::from(self.state.for_session(id)?))
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
        let next = fixture.replace("project_b");
        assert_eq!(queries.status().session(), next.info());
        assert!(queries.for_session(first.session().id()).is_err());
        assert!(
            crate::robots::commands::DeviceCommands::from(scoped.state.clone())
                .telemetry()
                .is_err()
        );
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
                .await
                .is_err()
        );
        assert!(
            crate::rollout::commands::RolloutCommands::from(scoped.state.clone())
                .snapshot()
                .await
                .is_err()
        );
        fixture.projects.hardware().shutdown().await.unwrap();
    }
}
