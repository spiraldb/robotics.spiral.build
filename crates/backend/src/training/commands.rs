//! Training runs and snapshots for one project.
use spiral_robotics_common::training::{TrainingConfig, TrainingRun, TrainingSnapshot};

use crate::BackendCommandError;
use crate::project_session::context::ProjectContext;

/// Project-bound operations; the context owns admission and backend task lifetime.
#[derive(Clone)]
pub(crate) struct TrainingCommands {
    state: ProjectContext,
}

impl TrainingCommands {
    pub(crate) async fn snapshot(&self) -> Result<TrainingSnapshot, BackendCommandError> {
        self.state
            .execute(move |state, activity| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded(activity).await?;
                Ok(state.training().catalog())
            })
            .await
    }
    pub(crate) async fn run(&self, id: String) -> Result<TrainingRun, BackendCommandError> {
        let activity = self.state.admit()?;
        self.state.preferences().load().await?;
        self.state.training().ensure_loaded(activity).await?;
        self.state.training().run(&id).map_err(Into::into)
    }

    pub(crate) async fn start_training_run(
        &self,
        config: TrainingConfig,
    ) -> Result<TrainingRun, BackendCommandError> {
        self.state
            .execute(move |state, activity| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded(activity).await?;
                state.require_catalog_ready()?;
                state
                    .training()
                    .start_validated(config)
                    .await
                    .map_err(Into::into)
            })
            .await
    }

    pub(crate) async fn stop_training_run(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state, activity| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded(activity).await?;
                state
                    .training()
                    .stop(&id)
                    .await
                    .map_err(|error| crate::OperationError::conflict(error).into())
            })
            .await
    }

    pub(crate) async fn delete_training_run(&self, id: String) -> Result<(), BackendCommandError> {
        self.state
            .execute(move |state, activity| async move {
                state.preferences().load().await?;
                state.training().ensure_loaded(activity).await?;
                let rollout = state.rollout().snapshot();
                let active_rollout = rollout
                    .phase()
                    .active()
                    .then_some(rollout.run_id())
                    .flatten();
                state
                    .training()
                    .delete_inactive(&id, active_rollout)
                    .await
                    .map_err(Into::into)
            })
            .await
    }
}

impl From<crate::project_session::context::ProjectContext> for TrainingCommands {
    fn from(state: crate::project_session::context::ProjectContext) -> Self {
        Self { state }
    }
}
