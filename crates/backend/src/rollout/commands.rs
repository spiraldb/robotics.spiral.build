//! Rollout execution and snapshots for one project.
use crate::training::service::TrainingActions;
use crate::{BackendCommandError, project_session::context::ProjectContext};
use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
#[derive(Clone)]
pub(crate) struct RolloutCommands {
    state: ProjectContext,
}
impl From<ProjectContext> for RolloutCommands {
    fn from(state: ProjectContext) -> Self {
        Self { state }
    }
}
impl RolloutCommands {
    pub(crate) fn snapshot(&self) -> Result<RolloutSnapshot, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.rollout().snapshot())
    }
    pub(crate) async fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> Result<RolloutSnapshot, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands
                    .state
                    .training()
                    .ensure_loaded(_activity.clone())
                    .await?;
                let config = TrainingActions::new(commands.state.training().as_ref())
                    .admit_rollout(config)?;
                commands
                    .state
                    .rollout()
                    .start(config)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn stop_rollout(&self) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                commands.state.rollout().stop().await.map_err(Into::into)
            })
            .await?
    }
}
