use super::*;

impl AppState {
    pub(super) fn training_rollout_router() -> Router<Self> {
        Router::new()
            .route("/training/runs", post(Self::start_training_run))
            .route("/training/runs/{id}", delete(Self::delete_training_run))
            .route("/training/runs/{id}/stop", post(Self::stop_training_run))
            .route("/rollout", post(Self::start_rollout))
            .route("/rollout/stop", post(Self::stop_rollout))
    }
}

impl TrainingPort for TrainingManager {
    fn training_run_status(&self, run_id: &str) -> Option<TrainingRunStatus> {
        let snapshot = TrainingManager::snapshot(self);
        snapshot
            .runs()
            .iter()
            .find(|run| run.id() == run_id)
            .map(|run| {
                TrainingRunStatus::new(
                    run.phase().active(),
                    run.checkpoints()
                        .iter()
                        .map(|checkpoint| {
                            TrainingCheckpointStatus::new(
                                checkpoint.step(),
                                checkpoint.inference_ready(),
                            )
                        })
                        .collect(),
                )
            })
    }

    async fn start_training(
        &self,
        command: StartTrainingRequest,
    ) -> Result<TrainingRun, OperationError> {
        TrainingManager::start(self, command)
            .await
            .map_err(OperationError::conflict)
    }

    fn stop_training(&self, run_id: &str) -> Result<(), OperationError> {
        TrainingManager::stop(self, run_id).map_err(OperationError::conflict)
    }

    async fn delete_training(&self, run_id: String) -> Result<(), OperationError> {
        TrainingManager::delete(self, &run_id)
            .await
            .map_err(OperationError::External)
    }
}

pub(super) struct RolloutHttpAdapter(Arc<RolloutManager>);

impl RolloutHttpAdapter {
    pub(super) fn new(rollout_manager: Arc<RolloutManager>) -> Self {
        Self(rollout_manager)
    }
}

impl RolloutSessionPort for RolloutHttpAdapter {
    fn snapshot(&self) -> RolloutSnapshot {
        RolloutManager::snapshot(self.0.as_ref())
    }

    async fn start(
        &self,
        command: RolloutRequest,
    ) -> Result<RolloutSnapshot, OperationError> {
        RolloutManager::start(&self.0, command.into())
            .await
            .map_err(OperationError::conflict)
    }

    async fn stop(&self) -> Result<(), OperationError> {
        RolloutManager::stop(self.0.as_ref())
            .await
            .map_err(OperationError::conflict)
    }
}

impl From<StartRolloutRequest> for RolloutRequest {
    fn from(request: StartRolloutRequest) -> Self {
        Self::new(
            request.run_id().to_owned(),
            request.checkpoint_step(),
            request.robot().to_owned(),
        )
        .with_duration(request.duration_seconds())
    }
}

impl From<RolloutRequest> for StartRolloutRequest {
    fn from(command: RolloutRequest) -> Self {
        Self::new(
            command.run_id().to_owned(),
            command.checkpoint_step(),
            command.robot().to_owned(),
        )
        .with_duration(command.duration_seconds())
    }
}

impl From<CapabilityRolloutSnapshot> for RolloutSnapshot {
    fn from(snapshot: CapabilityRolloutSnapshot) -> Self {
        let phase = match snapshot.phase() {
            CapabilityRolloutPhase::Idle => spiral_robotics_api::rollout::RolloutPhase::Idle,
            CapabilityRolloutPhase::Loading => spiral_robotics_api::rollout::RolloutPhase::Loading,
            CapabilityRolloutPhase::WarmingUp => {
                spiral_robotics_api::rollout::RolloutPhase::WarmingUp
            }
            CapabilityRolloutPhase::Running => spiral_robotics_api::rollout::RolloutPhase::Running,
            CapabilityRolloutPhase::Stopping => {
                spiral_robotics_api::rollout::RolloutPhase::Stopping
            }
            CapabilityRolloutPhase::Failed => spiral_robotics_api::rollout::RolloutPhase::Failed,
        };
        Self::new(
            spiral_robotics_api::rollout::RolloutIdentity::new(
                snapshot.session_id().map(str::to_owned),
                snapshot.run_id().map(str::to_owned),
                snapshot.robot().map(str::to_owned),
            )
            .with_checkpoint(snapshot.checkpoint_step())
            .with_duration(snapshot.duration_seconds()),
            spiral_robotics_api::rollout::RolloutLifecycle::new(phase, snapshot.started_ms())
                .with_finished_ms(snapshot.finished_ms())
                .with_messages(
                    snapshot.message().map(str::to_owned),
                    snapshot.warning().map(str::to_owned),
                ),
            spiral_robotics_api::rollout::RolloutMetrics::new(
                snapshot.metrics().inference_count(),
                snapshot.metrics().command_count(),
                snapshot.metrics().missed_deadlines(),
            )
            .with_latest_timings(
                snapshot.metrics().last_latency_ms(),
                snapshot.metrics().last_model_ms(),
            )
            .with_latency_summary(
                snapshot.metrics().mean_latency_ms(),
                snapshot.metrics().max_latency_ms(),
            ),
        )
    }
}

impl AppState {
    async fn start_training_run(
        State(state): State<Self>,
        Json(request): Json<StartTrainingRequest>,
    ) -> Result<(StatusCode, Json<TrainingRun>), Failure> {
        let run = TrainingActions::new(state.training().as_ref())
            .start(request)
            .await?;
        Ok((StatusCode::CREATED, Json(run)))
    }

    async fn stop_training_run(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        TrainingActions::new(state.training().as_ref()).stop(&id)?;
        Ok(StatusCode::ACCEPTED)
    }

    async fn delete_training_run(
        State(state): State<Self>,
        Path(id): Path<String>,
    ) -> Result<StatusCode, Failure> {
        let rollout = RolloutManager::snapshot(state.rollout().as_ref());
        let activity = TrainingRunActivity::new(
            rollout
                .phase()
                .active()
                .then_some(rollout.run_id())
                .flatten()
                .map(str::to_owned),
        );
        TrainingActions::new(state.training().as_ref())
            .delete(activity, DeleteTrainingRunCommand::new(id))
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn start_rollout(
        State(state): State<Self>,
        Json(request): Json<StartRolloutRequest>,
    ) -> Result<(StatusCode, Json<RolloutSnapshot>), Failure> {
        let request =
            TrainingActions::new(state.training().as_ref()).admit_rollout(request.into())?;
        let adapter = RolloutHttpAdapter::new(state.rollout().clone());
        let snapshot = RolloutActions::new(&adapter).start(request).await?;
        Ok((StatusCode::ACCEPTED, Json(snapshot)))
    }

    async fn stop_rollout(State(state): State<Self>) -> Result<StatusCode, Failure> {
        let adapter = RolloutHttpAdapter::new(state.rollout().clone());
        RolloutActions::new(&adapter).stop().await?;
        Ok(StatusCode::ACCEPTED)
    }
}
