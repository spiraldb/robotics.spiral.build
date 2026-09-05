//! Training and rollout commands exposed to fullstack delivery adapters.

use spiral_robotics_common::rollout::{RolloutConfig, RolloutSnapshot};
use spiral_robotics_common::training::{TrainingConfig, TrainingRun};

use crate::OperationError;
use crate::http::routes::AppState;
use crate::main_runtime::{MainRuntime, MainRuntimeFailure};
use crate::rollout::manager::RolloutManager;
use crate::training::service::{DeleteTrainingRunCommand, TrainingActions, TrainingRunActivity};

/// Narrow command facade shared with the Studio's server-function handlers.
#[derive(Clone)]
pub struct TrainingRolloutCommands {
    main_runtime: MainRuntime,
    state: AppState,
}

/// One command failure whose backend message is shown verbatim by Studio.
#[derive(Debug)]
pub struct TrainingRolloutFailure(String);

impl TrainingRolloutCommands {
    pub(crate) fn new(main_runtime: MainRuntime, state: AppState) -> Self {
        Self {
            main_runtime,
            state,
        }
    }

    pub fn for_session(&self, session_id: &str) -> Result<Self, TrainingRolloutFailure> {
        Ok(Self::new(
            self.main_runtime.clone(),
            self.state.for_session(session_id)?,
        ))
    }

    pub async fn start_training_run(
        &self,
        config: TrainingConfig,
    ) -> Result<TrainingRun, TrainingRolloutFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
            .execute(async move {
                let _activity = activity;
                if let Some(detail) = commands.state.bootstrap().catalog_unavailable() {
                    return Err(TrainingRolloutFailure(detail));
                }
                TrainingActions::new(commands.state.training().as_ref())
                    .start(config)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub async fn stop_training_run(&self, id: String) -> Result<(), TrainingRolloutFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
            .execute(async move {
                let _activity = activity;
                TrainingActions::new(commands.state.training().as_ref())
                    .stop(&id)
                    .await
                    .map_err(Into::into)
            })
            .await?
    }

    pub async fn delete_training_run(&self, id: String) -> Result<(), TrainingRolloutFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
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

    pub async fn start_rollout(
        &self,
        config: RolloutConfig,
    ) -> Result<RolloutSnapshot, TrainingRolloutFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
            .execute(async move {
                let _activity = activity;
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

    pub async fn stop_rollout(&self) -> Result<(), TrainingRolloutFailure> {
        let commands = self.clone();
        let activity = commands.state.admit()?;
        self.main_runtime
            .execute(async move {
                let _activity = activity;
                commands.state.rollout().stop().await.map_err(Into::into)
            })
            .await?
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

impl From<MainRuntimeFailure> for TrainingRolloutFailure {
    fn from(error: MainRuntimeFailure) -> Self {
        Self(error.to_string())
    }
}
