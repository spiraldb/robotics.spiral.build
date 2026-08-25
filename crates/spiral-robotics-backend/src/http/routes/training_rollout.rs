use super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/training/runs",
            get(list_training_runs).post(start_training),
        )
        .route("/training/runs/{id}", delete(delete_training))
        .route("/training/runs/{id}/stop", post(stop_training))
        .route("/rollout", get(get_rollout).post(start_rollout))
        .route("/rollout/stop", post(stop_rollout))
}

impl TrainingPort for TrainingManager {
    fn training_snapshot(&self) -> CapabilityTrainingSnapshot {
        TrainingManager::snapshot(self).into()
    }

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
        command: StartTrainingCommand,
    ) -> Result<CapabilityTrainingRun, OperationError> {
        TrainingManager::start(self, command.into())
            .await
            .map(Into::into)
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
    fn snapshot(&self) -> CapabilityRolloutSnapshot {
        RolloutManager::snapshot(self.0.as_ref()).into()
    }

    async fn start(
        &self,
        command: RolloutRequest,
    ) -> Result<CapabilityRolloutSnapshot, OperationError> {
        RolloutManager::start(&self.0, command.into())
            .await
            .map(Into::into)
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

impl From<RolloutSnapshot> for CapabilityRolloutSnapshot {
    fn from(snapshot: RolloutSnapshot) -> Self {
        let phase = match snapshot.phase() {
            spiral_robotics_api::rollout::RolloutPhase::Idle => CapabilityRolloutPhase::Idle,
            spiral_robotics_api::rollout::RolloutPhase::Loading => CapabilityRolloutPhase::Loading,
            spiral_robotics_api::rollout::RolloutPhase::WarmingUp => {
                CapabilityRolloutPhase::WarmingUp
            }
            spiral_robotics_api::rollout::RolloutPhase::Running => CapabilityRolloutPhase::Running,
            spiral_robotics_api::rollout::RolloutPhase::Stopping => {
                CapabilityRolloutPhase::Stopping
            }
            spiral_robotics_api::rollout::RolloutPhase::Failed => CapabilityRolloutPhase::Failed,
        };
        Self::new(
            crate::rollout::service::RolloutIdentity::new(
                snapshot.session_id().map(str::to_owned),
                snapshot.run_id().map(str::to_owned),
                snapshot.checkpoint_step(),
            )
            .with_target(
                snapshot.robot().map(str::to_owned),
                snapshot.duration_seconds(),
            ),
            crate::rollout::service::RolloutLifecycle::new(
                phase,
                snapshot.started_ms(),
                snapshot.finished_ms(),
            )
            .with_messages(
                snapshot.message().map(str::to_owned),
                snapshot.warning().map(str::to_owned),
            ),
            CapabilityRolloutMetrics::new(
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

async fn list_training_runs(State(state): State<AppState>) -> Json<TrainingSnapshot> {
    Json(training_application::list_training(state.training().as_ref()).into())
}

async fn start_training(
    State(state): State<AppState>,
    Json(request): Json<StartTrainingRequest>,
) -> Result<(StatusCode, Json<TrainingRun>), Failure> {
    let run =
        training_application::start_training(state.training().as_ref(), request.into()).await?;
    Ok((StatusCode::CREATED, Json(run.into())))
}

async fn stop_training(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    training_application::stop_training(state.training().as_ref(), &id)?;
    Ok(StatusCode::ACCEPTED)
}

async fn delete_training(
    State(state): State<AppState>,
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
    training_application::delete_training(
        state.training().as_ref(),
        activity,
        DeleteTrainingRunCommand::new(id),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_rollout(State(state): State<AppState>) -> Json<RolloutSnapshot> {
    Json(rollout_application::snapshot(&RolloutHttpAdapter::new(state.rollout().clone())).into())
}

async fn start_rollout(
    State(state): State<AppState>,
    Json(request): Json<StartRolloutRequest>,
) -> Result<(StatusCode, Json<RolloutSnapshot>), Failure> {
    let request = training_application::admit_rollout(state.training().as_ref(), request.into())?;
    let snapshot =
        rollout_application::start(&RolloutHttpAdapter::new(state.rollout().clone()), request)
            .await?;
    Ok((StatusCode::ACCEPTED, Json(snapshot.into())))
}

async fn stop_rollout(State(state): State<AppState>) -> Result<StatusCode, Failure> {
    rollout_application::stop(&RolloutHttpAdapter::new(state.rollout().clone())).await?;
    Ok(StatusCode::ACCEPTED)
}
