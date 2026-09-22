//! Training runs and snapshots for one project.
use spiral_robotics_common::training::{TrainingConfig, TrainingRun, TrainingSnapshot};

use crate::BackendCommandError;
use crate::projects::context::ProjectContext;

/// Project-bound operations; the context owns backend task lifetime.
#[derive(Clone)]
pub(crate) struct TrainingCommands {
    state: ProjectContext,
}

impl TrainingCommands {
    pub(crate) async fn snapshot(&self) -> Result<TrainingSnapshot, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded().await?;
                Ok(state.training().catalog())
            })
            .await
    }
    pub(super) async fn run(&self, id: String) -> Result<TrainingRun, BackendCommandError> {
        self.state.preferences().load().await?;
        self.state.training().ensure_loaded().await?;
        self.state.training().run(&id).map_err(Into::into)
    }

    pub(super) async fn start_training_run(
        &self,
        config: TrainingConfig,
    ) -> Result<TrainingRun, BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded().await?;
                state.training().start(config).await.map_err(Into::into)
            })
            .await
    }

    pub(super) async fn stop_training_run(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded().await?;
                state.training().stop(&id).await.map_err(Into::into)
            })
            .await
    }

    pub(super) async fn delete_training_run(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded().await?;
                let rollout = state.rollout().snapshot();
                let active_rollout = rollout
                    .phase()
                    .active()
                    .then_some(rollout.run_id())
                    .flatten();
                state
                    .training()
                    .delete(&id, active_rollout)
                    .await
                    .map_err(Into::into)
            })
            .await
    }
}

impl From<crate::projects::context::ProjectContext> for TrainingCommands {
    fn from(state: crate::projects::context::ProjectContext) -> Self {
        Self { state }
    }
}
