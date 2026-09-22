//! Rollout execution and snapshots for one project.
use crate::{BackendCommandError, projects::context::ProjectContext};
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
    pub(crate) async fn runs(&self) -> Result<Vec<RolloutSnapshot>, BackendCommandError> {
        let commands = self.clone();
        self.state
            .tasks()
            .read(async move { commands.state.rollout().runs().await.map_err(Into::into) })
            .await?
    }
    pub(super) async fn run(&self, id: String) -> Result<RolloutSnapshot, BackendCommandError> {
        self.state.rollout().run(&id).await.map_err(Into::into)
    }

    pub(super) async fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> Result<RolloutSnapshot, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded().await?;
                let config = state.training().admit_rollout(config)?;
                state.rollout().start(config).await.map_err(Into::into)
            })
            .await
    }

    pub(super) async fn stop_rollout(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(
                move |state| async move { state.rollout().stop(&id).await.map_err(Into::into) },
            )
            .await
    }
}
