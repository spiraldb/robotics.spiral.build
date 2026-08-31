//! Training and rollout commands exposed to fullstack delivery adapters.

use std::sync::Arc;

use spiral_robotics_api::rollout::{RolloutConfig, RolloutSnapshot};
use spiral_robotics_api::training::{TrainingConfig, TrainingRun};

use crate::OperationError;
use crate::rollout::manager::RolloutManager;
use crate::training::manager::TrainingManager;
use crate::training::service::{DeleteTrainingRunCommand, TrainingActions, TrainingRunActivity};

/// Narrow command facade shared with the Studio's server-function handlers.
#[derive(Clone)]
pub struct TrainingRolloutCommands {
    training: Arc<TrainingManager>,
    rollout: Arc<RolloutManager>,
}

/// One command failure whose backend message is shown verbatim by Studio.
#[derive(Debug)]
pub struct TrainingRolloutFailure(String);

impl TrainingRolloutCommands {
    pub(crate) fn new(training: Arc<TrainingManager>, rollout: Arc<RolloutManager>) -> Self {
        Self { training, rollout }
    }

    pub async fn start_training_run(
        &self,
        config: TrainingConfig,
    ) -> Result<TrainingRun, TrainingRolloutFailure> {
        TrainingActions::new(self.training.as_ref())
            .start(config)
            .await
            .map_err(Into::into)
    }

    pub async fn stop_training_run(&self, id: String) -> Result<(), TrainingRolloutFailure> {
        TrainingActions::new(self.training.as_ref())
            .stop(&id)
            .await
            .map_err(Into::into)
    }

    pub async fn delete_training_run(&self, id: String) -> Result<(), TrainingRolloutFailure> {
        let rollout = RolloutManager::snapshot(self.rollout.as_ref());
        let activity = TrainingRunActivity::new(
            rollout
                .phase()
                .active()
                .then_some(rollout.run_id())
                .flatten()
                .map(str::to_owned),
        );
        TrainingActions::new(self.training.as_ref())
            .delete(activity, DeleteTrainingRunCommand::new(id))
            .await
            .map_err(Into::into)
    }

    pub async fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> Result<RolloutSnapshot, TrainingRolloutFailure> {
        let config = TrainingActions::new(self.training.as_ref()).admit_rollout(config)?;
        self.rollout.start(config).await.map_err(Into::into)
    }

    pub async fn stop_rollout(&self) -> Result<(), TrainingRolloutFailure> {
        self.rollout.stop().await.map_err(Into::into)
    }
}

impl std::fmt::Display for TrainingRolloutFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<OperationError> for TrainingRolloutFailure {
    fn from(error: OperationError) -> Self {
        Self(error.to_string())
    }
}

impl From<String> for TrainingRolloutFailure {
    fn from(message: String) -> Self {
        Self(message)
    }
}
