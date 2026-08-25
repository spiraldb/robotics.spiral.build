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
        capability_training_snapshot(TrainingManager::snapshot(self))
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
        TrainingManager::start(self, training_request(command))
            .await
            .map(capability_training_run)
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
        capability_rollout_snapshot(RolloutManager::snapshot(self.0.as_ref()))
    }

    async fn start(
        &self,
        command: RolloutRequest,
    ) -> Result<CapabilityRolloutSnapshot, OperationError> {
        RolloutManager::start(&self.0, rollout_request(command))
            .await
            .map(capability_rollout_snapshot)
            .map_err(OperationError::conflict)
    }

    async fn stop(&self) -> Result<(), OperationError> {
        RolloutManager::stop(self.0.as_ref())
            .await
            .map_err(OperationError::conflict)
    }
}

fn training_command(request: StartTrainingRequest) -> StartTrainingCommand {
    let policy = match request.policy() {
        ContractTrainingPolicy::Act => CapabilityTrainingPolicy::Act,
    };
    let request_act = request.act();
    let input = ActTrainingInputSettings::new(
        request_act.camera_streams().to_vec(),
        (request_act.image_width(), request_act.image_height()),
        request_act.video_decoder(),
    )
    .with_pretrained_backbone(request_act.pretrained_backbone());
    let model = ActTrainingModelSettings::new(
        request_act.hidden_dim(),
        request_act.latent_dim(),
        request_act.attention_heads(),
    )
    .with_feedforward(request_act.feedforward_dim(), request_act.kl_weight());
    let layers = ActTrainingLayerSettings::new(
        request_act.encoder_layers(),
        request_act.vae_encoder_layers(),
        request_act.decoder_layers(),
    );
    let execution = TrainingExecutionSettings::new(
        request.device(),
        TrainingProgressSettings::new(
            request.steps(),
            request.batch_size(),
            request.validation_split(),
        ),
        ActTrainingSettings::new(input, model, layers),
    )
    .with_schedule(
        TrainingOptimizerSettings::new(request.learning_rate(), request.seed()),
        TrainingCheckpointSettings::new(request.checkpoint_every(), request.chunk_size()),
    );
    StartTrainingCommand::new(
        TrainingCommandIdentity::new(request.run_name(), request.dataset_id(), policy),
        TrainingDataSettings::new(
            request.sql(),
            request.state_column(),
            request.action_column(),
        ),
        execution,
    )
}

fn training_request(command: StartTrainingCommand) -> StartTrainingRequest {
    let policy = match command.policy() {
        CapabilityTrainingPolicy::Act => ContractTrainingPolicy::Act,
    };
    let selection = spiral_robotics_api::training::TrainingDataSelection::new(
        command.run_name(),
        command.dataset_id(),
        command.query(),
    )
    .with_policy(policy)
    .with_columns(command.state_column(), command.action_column())
    .with_device(command.device());
    let optimization = spiral_robotics_api::training::TrainingOptimizationConfig::new(
        command.steps(),
        command.batch_size(),
        command.learning_rate(),
    )
    .with_validation_split(command.validation_split())
    .with_checkpoint_schedule(command.checkpoint_every(), command.chunk_size())
    .with_seed(command.seed());
    let act = command.act();
    let act_input = spiral_robotics_api::training::ActInputConfig::new(
        act.camera_streams().to_vec(),
        (act.image_width(), act.image_height()),
        act.video_decoder(),
    )
    .with_pretrained_backbone(act.pretrained_backbone());
    let layer_counts = spiral_robotics_api::training::ActLayerCounts::new(
        act.encoder_layers(),
        act.vae_encoder_layers(),
        act.decoder_layers(),
    );
    let architecture = spiral_robotics_api::training::ActArchitectureConfig::new(
        act.hidden_dim(),
        act.latent_dim(),
        layer_counts,
    );
    let attention = spiral_robotics_api::training::ActAttentionConfig::new(
        act.attention_heads(),
        act.feedforward_dim(),
        act.kl_weight(),
    );
    StartTrainingRequest::new(
        selection,
        optimization,
        spiral_robotics_api::training::ActTrainingConfig::new(act_input, architecture, attention),
    )
}

fn rollout_command(request: StartRolloutRequest) -> RolloutRequest {
    RolloutRequest::new(
        request.run_id().to_owned(),
        request.checkpoint_step(),
        request.robot().to_owned(),
    )
    .with_duration(request.duration_seconds())
}

fn rollout_request(command: RolloutRequest) -> StartRolloutRequest {
    StartRolloutRequest::new(
        command.run_id().to_owned(),
        command.checkpoint_step(),
        command.robot().to_owned(),
    )
    .with_duration(command.duration_seconds())
}

fn capability_training_phase(
    phase: spiral_robotics_api::training::TrainingRunPhase,
) -> CapabilityTrainingRunPhase {
    match phase {
        spiral_robotics_api::training::TrainingRunPhase::Starting => {
            CapabilityTrainingRunPhase::Starting
        }
        spiral_robotics_api::training::TrainingRunPhase::Running => {
            CapabilityTrainingRunPhase::Running
        }
        spiral_robotics_api::training::TrainingRunPhase::Stopping => {
            CapabilityTrainingRunPhase::Stopping
        }
        spiral_robotics_api::training::TrainingRunPhase::Completed => {
            CapabilityTrainingRunPhase::Completed
        }
        spiral_robotics_api::training::TrainingRunPhase::Stopped => {
            CapabilityTrainingRunPhase::Stopped
        }
        spiral_robotics_api::training::TrainingRunPhase::Failed => {
            CapabilityTrainingRunPhase::Failed
        }
    }
}

fn contract_training_phase(
    phase: CapabilityTrainingRunPhase,
) -> spiral_robotics_api::training::TrainingRunPhase {
    match phase {
        CapabilityTrainingRunPhase::Starting => {
            spiral_robotics_api::training::TrainingRunPhase::Starting
        }
        CapabilityTrainingRunPhase::Running => {
            spiral_robotics_api::training::TrainingRunPhase::Running
        }
        CapabilityTrainingRunPhase::Stopping => {
            spiral_robotics_api::training::TrainingRunPhase::Stopping
        }
        CapabilityTrainingRunPhase::Completed => {
            spiral_robotics_api::training::TrainingRunPhase::Completed
        }
        CapabilityTrainingRunPhase::Stopped => {
            spiral_robotics_api::training::TrainingRunPhase::Stopped
        }
        CapabilityTrainingRunPhase::Failed => {
            spiral_robotics_api::training::TrainingRunPhase::Failed
        }
    }
}

pub(super) fn capability_training_run(run: TrainingRun) -> CapabilityTrainingRun {
    let metrics = run
        .metrics()
        .iter()
        .map(|metric| {
            let progress = TrainingMetricProgress::new(
                metric.step(),
                metric.train_loss(),
                metric.validation_loss(),
            )
            .with_event(metric.learning_rate(), metric.event().map(str::to_owned));
            let throughput = TrainingMetricThroughput::new(
                metric.transfer_bytes_per_second(),
                metric.examples_per_second(),
            );
            let resources = TrainingMetricResources::new(
                metric.cpu_percent(),
                metric.gpu_percent(),
                metric.memory_bytes(),
            );
            let timings = TrainingMetricTimings::new(
                metric.loader_ms(),
                metric.arrow_encode_ms(),
                metric.upload_ms(),
            )
            .with_execution(metric.device_copy_ms(), metric.model_ms())
            .elapsed(metric.elapsed_seconds());
            CapabilityTrainingMetric::new(progress, throughput, resources).with_timings(timings)
        })
        .collect();
    let checkpoints = run
        .checkpoints()
        .iter()
        .map(|checkpoint| {
            CapabilityTrainingCheckpoint::new(checkpoint.step(), checkpoint.created_ms())
                .with_validation_loss(checkpoint.validation_loss())
                .with_artifact(checkpoint.byte_size(), checkpoint.inference_ready())
                .with_rollout(
                    checkpoint.recommended_action_steps(),
                    checkpoint.sample_hz(),
                )
        })
        .collect();
    CapabilityTrainingRun::new(
        TrainingRunIdentity::new(
            run.id(),
            training_command(run.config().clone()),
            CapabilityTrainingSource::new(run.source().dataset_id()),
        ),
        TrainingRunState::new(
            capability_training_phase(run.phase()),
            run.current_step(),
            run.started_ms(),
        )
        .finished(run.finished_ms(), run.message().map(str::to_owned)),
        TrainingRunHistory::new(metrics, checkpoints, run.logs().to_vec()),
    )
}

pub(super) fn contract_training_run(run: CapabilityTrainingRun) -> TrainingRun {
    let metrics = run
        .metrics()
        .iter()
        .map(|metric| {
            let progress = spiral_robotics_api::training::TrainingMetricProgress::new(
                metric.step(),
                metric.train_loss(),
                metric.learning_rate(),
            )
            .with_validation_loss(metric.validation_loss())
            .with_timeline(metric.elapsed_seconds(), metric.event().map(str::to_owned));
            let resources = spiral_robotics_api::training::TrainingResourceMetrics::new(
                metric.transfer_bytes_per_second(),
                metric.examples_per_second(),
            )
            .with_utilization(metric.cpu_percent(), metric.gpu_percent())
            .with_memory_bytes(metric.memory_bytes());
            let timings = spiral_robotics_api::training::TrainingStageTimings::new(
                metric.loader_ms(),
                metric.batch_encode_ms(),
                metric.upload_ms(),
            )
            .with_runtime(metric.device_copy_ms(), metric.model_ms());
            spiral_robotics_api::training::TrainingMetricPoint::new(progress, resources, timings)
        })
        .collect();
    let checkpoints = run
        .checkpoints()
        .iter()
        .map(|checkpoint| {
            spiral_robotics_api::training::TrainingCheckpoint::new(
                checkpoint.step(),
                checkpoint.created_ms(),
            )
            .with_validation_loss(checkpoint.validation_loss())
            .with_artifact(checkpoint.byte_size(), checkpoint.inference_ready())
            .with_rollout(
                checkpoint.recommended_action_steps(),
                checkpoint.sample_hz(),
            )
        })
        .collect();
    TrainingRun::new(
        spiral_robotics_api::training::TrainingRunIdentity::new(
            run.id(),
            training_request(run.config().clone()),
            spiral_robotics_api::training::TrainingSource::new(run.source().dataset_id()),
        ),
        spiral_robotics_api::training::TrainingRunLifecycle::new(
            contract_training_phase(run.phase()),
            run.current_step(),
            run.started_ms(),
        )
        .with_completion(run.finished_ms(), run.message().map(str::to_owned)),
        spiral_robotics_api::training::TrainingRunHistory::new(
            metrics,
            checkpoints,
            run.logs().to_vec(),
        ),
    )
}

pub(super) fn capability_training_snapshot(
    snapshot: TrainingSnapshot,
) -> CapabilityTrainingSnapshot {
    CapabilityTrainingSnapshot::new(
        CapabilityTrainingReadiness::new(
            snapshot.readiness().ready(),
            snapshot.readiness().spiral_ready(),
            snapshot.readiness().summary(),
        )
        .with_runtime(snapshot.readiness().runtime().map(|runtime| {
            crate::training::service::TrainingRuntimeInfo::new(
                runtime.id(),
                runtime.display_name(),
                runtime.runtime(),
            )
            .with_devices(runtime.devices().to_vec())
        })),
        snapshot
            .runs()
            .iter()
            .cloned()
            .map(capability_training_run)
            .collect(),
    )
}

pub(super) fn contract_training_snapshot(snapshot: CapabilityTrainingSnapshot) -> TrainingSnapshot {
    let runtime = snapshot.readiness().runtime().map(|runtime| {
        spiral_robotics_api::training::TrainingRuntimeInfo::new(
            runtime.id().to_owned(),
            runtime.display_name().to_owned(),
            runtime.runtime().to_owned(),
        )
        .with_devices(runtime.devices().to_vec())
    });
    let readiness = spiral_robotics_api::training::TrainingReadiness::new(
        snapshot.readiness().ready(),
        snapshot.readiness().spiral_ready(),
        snapshot.readiness().summary(),
    )
    .with_runtime(runtime);
    TrainingSnapshot::new(
        readiness,
        snapshot
            .runs()
            .iter()
            .cloned()
            .map(contract_training_run)
            .collect(),
    )
}

pub(super) fn capability_rollout_snapshot(snapshot: RolloutSnapshot) -> CapabilityRolloutSnapshot {
    let phase = match snapshot.phase() {
        spiral_robotics_api::rollout::RolloutPhase::Idle => CapabilityRolloutPhase::Idle,
        spiral_robotics_api::rollout::RolloutPhase::Loading => CapabilityRolloutPhase::Loading,
        spiral_robotics_api::rollout::RolloutPhase::WarmingUp => CapabilityRolloutPhase::WarmingUp,
        spiral_robotics_api::rollout::RolloutPhase::Running => CapabilityRolloutPhase::Running,
        spiral_robotics_api::rollout::RolloutPhase::Stopping => CapabilityRolloutPhase::Stopping,
        spiral_robotics_api::rollout::RolloutPhase::Failed => CapabilityRolloutPhase::Failed,
    };
    CapabilityRolloutSnapshot::new(
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

pub(super) fn contract_rollout_snapshot(snapshot: CapabilityRolloutSnapshot) -> RolloutSnapshot {
    let phase = match snapshot.phase() {
        CapabilityRolloutPhase::Idle => spiral_robotics_api::rollout::RolloutPhase::Idle,
        CapabilityRolloutPhase::Loading => spiral_robotics_api::rollout::RolloutPhase::Loading,
        CapabilityRolloutPhase::WarmingUp => spiral_robotics_api::rollout::RolloutPhase::WarmingUp,
        CapabilityRolloutPhase::Running => spiral_robotics_api::rollout::RolloutPhase::Running,
        CapabilityRolloutPhase::Stopping => spiral_robotics_api::rollout::RolloutPhase::Stopping,
        CapabilityRolloutPhase::Failed => spiral_robotics_api::rollout::RolloutPhase::Failed,
    };
    RolloutSnapshot::new(
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

async fn list_training_runs(State(state): State<AppState>) -> Json<TrainingSnapshot> {
    Json(contract_training_snapshot(
        training_application::list_training(state.training().as_ref()),
    ))
}

async fn start_training(
    State(state): State<AppState>,
    Json(request): Json<StartTrainingRequest>,
) -> Result<(StatusCode, Json<TrainingRun>), Failure> {
    let run =
        training_application::start_training(state.training().as_ref(), training_command(request))
            .await?;
    Ok((StatusCode::CREATED, Json(contract_training_run(run))))
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
    Json(contract_rollout_snapshot(rollout_application::snapshot(
        &RolloutHttpAdapter::new(state.rollout().clone()),
    )))
}

async fn start_rollout(
    State(state): State<AppState>,
    Json(request): Json<StartRolloutRequest>,
) -> Result<(StatusCode, Json<RolloutSnapshot>), Failure> {
    let request =
        training_application::admit_rollout(state.training().as_ref(), rollout_command(request))?;
    let snapshot =
        rollout_application::start(&RolloutHttpAdapter::new(state.rollout().clone()), request)
            .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(contract_rollout_snapshot(snapshot)),
    ))
}

async fn stop_rollout(State(state): State<AppState>) -> Result<StatusCode, Failure> {
    rollout_application::stop(&RolloutHttpAdapter::new(state.rollout().clone())).await?;
    Ok(StatusCode::ACCEPTED)
}
