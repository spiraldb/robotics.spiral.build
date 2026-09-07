//! Training runs and snapshots for one project.
use spiral_robotics_common::training::{TrainingConfig, TrainingRun, TrainingSnapshot};

use crate::BackendCommandError;
use crate::project_session::context::ProjectContext;
use crate::rollout::manager::RolloutManager;
use crate::training::service::{DeleteTrainingRunCommand, TrainingActions, TrainingRunActivity};

/// Project-bound operations; the context owns admission and backend task lifetime.
#[derive(Clone)]
pub(crate) struct TrainingCommands {
    state: ProjectContext,
}

impl TrainingCommands {
    pub(crate) fn snapshot(
        &self,
        selected: Option<String>,
    ) -> Result<TrainingSnapshot, BackendCommandError> {
        let _activity = self.state.admit()?;
        Ok(self.state.training().display_snapshot(selected.as_deref()))
    }
    pub(crate) async fn start_training_run(
        &self,
        config: TrainingConfig,
    ) -> Result<TrainingRun, BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                if let Some(detail) = commands.state.bootstrap().catalog_unavailable() {
                    return Err(BackendCommandError::from(detail));
                }
                TrainingActions::new(commands.state.training().as_ref())
                    .start(config)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn stop_training_run(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                TrainingActions::new(commands.state.training().as_ref())
                    .stop(&id)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub(crate) async fn delete_training_run(&self, id: String) -> Result<(), BackendCommandError> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.state
            .tasks()
            .execute(async move {
                let _activity = activity;
                let rollout = RolloutManager::snapshot(commands.state.rollout().as_ref());
                let activity = TrainingRunActivity::new(
                    rollout
                        .phase()
                        .active()
                        .then_some(rollout.run_id())
                        .flatten()
                        .map(str::to_owned),
                );
                TrainingActions::new(commands.state.training().as_ref())
                    .delete(activity, DeleteTrainingRunCommand::new(id))
                    .await
                    .map_err(Into::into)
            })
            .await?
    }
}

impl From<crate::project_session::context::ProjectContext> for TrainingCommands {
    fn from(state: crate::project_session::context::ProjectContext) -> Self {
        Self { state }
    }
}
