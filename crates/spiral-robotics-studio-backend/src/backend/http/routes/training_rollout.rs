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
    fn training_snapshot(&self) -> ApplicationTrainingSnapshot {
        application_training_snapshot(TrainingManager::snapshot(self))
    }

    fn training_run_status(&self, run_id: &str) -> Option<TrainingRunStatus> {
        let snapshot = TrainingManager::snapshot(self);
        snapshot
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .map(|run| TrainingRunStatus {
                active: run.phase.active(),
                checkpoints: run
                    .checkpoints
                    .iter()
                    .map(|checkpoint| TrainingCheckpointStatus {
                        step: checkpoint.step,
                        inference_ready: checkpoint.inference_ready,
                    })
                    .collect(),
            })
    }

    async fn start_training(
        &self,
        command: StartTrainingCommand,
    ) -> Result<ApplicationTrainingRun, ApplicationError> {
        TrainingManager::start(self, training_request(command))
            .await
            .map(application_training_run)
            .map_err(ApplicationError::conflict)
    }

    fn stop_training(&self, run_id: &str) -> Result<(), ApplicationError> {
        TrainingManager::stop(self, run_id).map_err(ApplicationError::conflict)
    }

    async fn delete_training(&self, run_id: String) -> Result<(), ApplicationError> {
        TrainingManager::delete(self, &run_id)
            .await
            .map_err(ApplicationError::External)
    }
}

pub(super) struct RolloutHttpAdapter(pub(super) Arc<RolloutManager>);

impl RolloutSessionPort for RolloutHttpAdapter {
    fn snapshot(&self) -> ApplicationRolloutSnapshot {
        application_rollout_snapshot(RolloutManager::snapshot(self.0.as_ref()))
    }

    async fn start(
        &self,
        command: RolloutRequest,
    ) -> Result<ApplicationRolloutSnapshot, ApplicationError> {
        RolloutManager::start(&self.0, rollout_request(command))
            .await
            .map(application_rollout_snapshot)
            .map_err(ApplicationError::conflict)
    }

    async fn stop(&self) -> Result<(), ApplicationError> {
        RolloutManager::stop(self.0.as_ref())
            .await
            .map_err(ApplicationError::conflict)
    }
}

fn training_command(request: StartTrainingRequest) -> StartTrainingCommand {
    let policy = match request.policy {
        ContractTrainingPolicy::Act => ApplicationTrainingPolicy::Act,
    };
    let act = ActTrainingSettings {
        camera_streams: request.act.camera_streams,
        image_width: request.act.image_width,
        image_height: request.act.image_height,
        video_decoder: request.act.video_decoder,
        pretrained_backbone: request.act.pretrained_backbone,
        hidden_dim: request.act.hidden_dim,
        latent_dim: request.act.latent_dim,
        encoder_layers: request.act.encoder_layers,
        vae_encoder_layers: request.act.vae_encoder_layers,
        decoder_layers: request.act.decoder_layers,
        attention_heads: request.act.attention_heads,
        feedforward_dim: request.act.feedforward_dim,
        kl_weight: request.act.kl_weight,
    };
    StartTrainingCommand {
        run_name: request.run_name,
        dataset_id: request.dataset_id,
        policy,
        query: request.sql,
        state_column: request.state_column,
        action_column: request.action_column,
        device: request.device,
        steps: request.steps,
        batch_size: request.batch_size,
        validation_split: request.validation_split,
        learning_rate: request.learning_rate,
        checkpoint_every: request.checkpoint_every,
        chunk_size: request.chunk_size,
        seed: request.seed,
        act,
    }
}

fn training_request(command: StartTrainingCommand) -> StartTrainingRequest {
    let policy = match command.policy {
        ApplicationTrainingPolicy::Act => ContractTrainingPolicy::Act,
    };
    StartTrainingRequest {
        run_name: command.run_name,
        dataset_id: command.dataset_id,
        policy,
        sql: command.query,
        state_column: command.state_column,
        action_column: command.action_column,
        device: command.device,
        steps: command.steps,
        batch_size: command.batch_size,
        validation_split: command.validation_split,
        learning_rate: command.learning_rate,
        checkpoint_every: command.checkpoint_every,
        chunk_size: command.chunk_size,
        seed: command.seed,
        act: spiral_robotics_contracts::api::ActTrainingConfig {
            camera_streams: command.act.camera_streams,
            image_width: command.act.image_width,
            image_height: command.act.image_height,
            video_decoder: command.act.video_decoder,
            pretrained_backbone: command.act.pretrained_backbone,
            hidden_dim: command.act.hidden_dim,
            latent_dim: command.act.latent_dim,
            encoder_layers: command.act.encoder_layers,
            vae_encoder_layers: command.act.vae_encoder_layers,
            decoder_layers: command.act.decoder_layers,
            attention_heads: command.act.attention_heads,
            feedforward_dim: command.act.feedforward_dim,
            kl_weight: command.act.kl_weight,
        },
    }
}

fn rollout_command(request: StartRolloutRequest) -> RolloutRequest {
    RolloutRequest {
        run_id: request.run_id,
        checkpoint_step: request.checkpoint_step,
        robot: request.robot,
        duration_seconds: request.duration_seconds,
    }
}

fn rollout_request(command: RolloutRequest) -> StartRolloutRequest {
    StartRolloutRequest {
        run_id: command.run_id,
        checkpoint_step: command.checkpoint_step,
        robot: command.robot,
        duration_seconds: command.duration_seconds,
    }
}

fn application_training_phase(
    phase: spiral_robotics_contracts::api::TrainingRunPhase,
) -> ApplicationTrainingRunPhase {
    match phase {
        spiral_robotics_contracts::api::TrainingRunPhase::Starting => {
            ApplicationTrainingRunPhase::Starting
        }
        spiral_robotics_contracts::api::TrainingRunPhase::Running => {
            ApplicationTrainingRunPhase::Running
        }
        spiral_robotics_contracts::api::TrainingRunPhase::Stopping => {
            ApplicationTrainingRunPhase::Stopping
        }
        spiral_robotics_contracts::api::TrainingRunPhase::Completed => {
            ApplicationTrainingRunPhase::Completed
        }
        spiral_robotics_contracts::api::TrainingRunPhase::Stopped => {
            ApplicationTrainingRunPhase::Stopped
        }
        spiral_robotics_contracts::api::TrainingRunPhase::Failed => {
            ApplicationTrainingRunPhase::Failed
        }
    }
}

fn contract_training_phase(
    phase: ApplicationTrainingRunPhase,
) -> spiral_robotics_contracts::api::TrainingRunPhase {
    match phase {
        ApplicationTrainingRunPhase::Starting => {
            spiral_robotics_contracts::api::TrainingRunPhase::Starting
        }
        ApplicationTrainingRunPhase::Running => {
            spiral_robotics_contracts::api::TrainingRunPhase::Running
        }
        ApplicationTrainingRunPhase::Stopping => {
            spiral_robotics_contracts::api::TrainingRunPhase::Stopping
        }
        ApplicationTrainingRunPhase::Completed => {
            spiral_robotics_contracts::api::TrainingRunPhase::Completed
        }
        ApplicationTrainingRunPhase::Stopped => {
            spiral_robotics_contracts::api::TrainingRunPhase::Stopped
        }
        ApplicationTrainingRunPhase::Failed => {
            spiral_robotics_contracts::api::TrainingRunPhase::Failed
        }
    }
}

pub(super) fn application_training_run(run: TrainingRun) -> ApplicationTrainingRun {
    ApplicationTrainingRun {
        id: run.id,
        config: training_command(run.config),
        source: ApplicationTrainingSource {
            dataset_id: run.source.dataset_id,
        },
        phase: application_training_phase(run.phase),
        current_step: run.current_step,
        started_ms: run.started_ms,
        finished_ms: run.finished_ms,
        message: run.message,
        metrics: run
            .metrics
            .into_iter()
            .map(|metric| ApplicationTrainingMetric {
                step: metric.step,
                train_loss: metric.train_loss,
                validation_loss: metric.validation_loss,
                learning_rate: metric.learning_rate,
                transfer_bytes_per_second: metric.transfer_bytes_per_second,
                examples_per_second: metric.examples_per_second,
                cpu_percent: metric.cpu_percent,
                gpu_percent: metric.gpu_percent,
                memory_bytes: metric.memory_bytes,
                loader_ms: metric.loader_ms,
                batch_encode_ms: metric.arrow_encode_ms,
                upload_ms: metric.upload_ms,
                device_copy_ms: metric.device_copy_ms,
                model_ms: metric.model_ms,
                elapsed_seconds: metric.elapsed_seconds,
                event: metric.event,
            })
            .collect(),
        checkpoints: run
            .checkpoints
            .into_iter()
            .map(|checkpoint| ApplicationTrainingCheckpoint {
                step: checkpoint.step,
                created_ms: checkpoint.created_ms,
                validation_loss: checkpoint.validation_loss,
                byte_size: checkpoint.byte_size,
                inference_ready: checkpoint.inference_ready,
                recommended_action_steps: checkpoint.recommended_action_steps,
                sample_hz: checkpoint.sample_hz,
            })
            .collect(),
        logs: run.logs,
    }
}

pub(super) fn contract_training_run(run: ApplicationTrainingRun) -> TrainingRun {
    TrainingRun {
        id: run.id,
        config: training_request(run.config),
        source: spiral_robotics_contracts::api::TrainingSource {
            dataset_id: run.source.dataset_id,
        },
        phase: contract_training_phase(run.phase),
        current_step: run.current_step,
        started_ms: run.started_ms,
        finished_ms: run.finished_ms,
        message: run.message,
        metrics: run
            .metrics
            .into_iter()
            .map(
                |metric| spiral_robotics_contracts::api::TrainingMetricPoint {
                    step: metric.step,
                    train_loss: metric.train_loss,
                    validation_loss: metric.validation_loss,
                    learning_rate: metric.learning_rate,
                    transfer_bytes_per_second: metric.transfer_bytes_per_second,
                    examples_per_second: metric.examples_per_second,
                    cpu_percent: metric.cpu_percent,
                    gpu_percent: metric.gpu_percent,
                    memory_bytes: metric.memory_bytes,
                    loader_ms: metric.loader_ms,
                    arrow_encode_ms: metric.batch_encode_ms,
                    upload_ms: metric.upload_ms,
                    device_copy_ms: metric.device_copy_ms,
                    model_ms: metric.model_ms,
                    elapsed_seconds: metric.elapsed_seconds,
                    event: metric.event,
                },
            )
            .collect(),
        checkpoints: run
            .checkpoints
            .into_iter()
            .map(
                |checkpoint| spiral_robotics_contracts::api::TrainingCheckpoint {
                    step: checkpoint.step,
                    created_ms: checkpoint.created_ms,
                    validation_loss: checkpoint.validation_loss,
                    byte_size: checkpoint.byte_size,
                    inference_ready: checkpoint.inference_ready,
                    recommended_action_steps: checkpoint.recommended_action_steps,
                    sample_hz: checkpoint.sample_hz,
                },
            )
            .collect(),
        logs: run.logs,
    }
}

pub(super) fn application_training_snapshot(
    snapshot: TrainingSnapshot,
) -> ApplicationTrainingSnapshot {
    ApplicationTrainingSnapshot {
        readiness: ApplicationTrainingReadiness {
            ready: snapshot.readiness.ready,
            spiral_ready: snapshot.readiness.spiral_ready,
            runtime: snapshot.readiness.runtime.map(|runtime| {
                spiral_robotics_studio_application::training::TrainingRuntimeInfo {
                    id: runtime.id,
                    display_name: runtime.display_name,
                    runtime: runtime.runtime,
                    devices: runtime.devices,
                }
            }),
            summary: snapshot.readiness.summary,
        },
        runs: snapshot
            .runs
            .into_iter()
            .map(application_training_run)
            .collect(),
    }
}

pub(super) fn contract_training_snapshot(
    snapshot: ApplicationTrainingSnapshot,
) -> TrainingSnapshot {
    TrainingSnapshot {
        readiness: spiral_robotics_contracts::api::TrainingReadiness {
            ready: snapshot.readiness.ready,
            spiral_ready: snapshot.readiness.spiral_ready,
            runtime: snapshot.readiness.runtime.map(|runtime| {
                spiral_robotics_contracts::api::TrainingRuntimeInfo {
                    id: runtime.id,
                    display_name: runtime.display_name,
                    runtime: runtime.runtime,
                    devices: runtime.devices,
                }
            }),
            summary: snapshot.readiness.summary,
        },
        runs: snapshot
            .runs
            .into_iter()
            .map(contract_training_run)
            .collect(),
    }
}

pub(super) fn application_rollout_snapshot(
    snapshot: RolloutSnapshot,
) -> ApplicationRolloutSnapshot {
    ApplicationRolloutSnapshot {
        phase: match snapshot.phase {
            spiral_robotics_contracts::api::RolloutPhase::Idle => ApplicationRolloutPhase::Idle,
            spiral_robotics_contracts::api::RolloutPhase::Loading => {
                ApplicationRolloutPhase::Loading
            }
            spiral_robotics_contracts::api::RolloutPhase::WarmingUp => {
                ApplicationRolloutPhase::WarmingUp
            }
            spiral_robotics_contracts::api::RolloutPhase::Running => {
                ApplicationRolloutPhase::Running
            }
            spiral_robotics_contracts::api::RolloutPhase::Stopping => {
                ApplicationRolloutPhase::Stopping
            }
            spiral_robotics_contracts::api::RolloutPhase::Failed => ApplicationRolloutPhase::Failed,
        },
        session_id: snapshot.session_id,
        run_id: snapshot.run_id,
        checkpoint_step: snapshot.checkpoint_step,
        robot: snapshot.robot,
        duration_seconds: snapshot.duration_seconds,
        started_ms: snapshot.started_ms,
        finished_ms: snapshot.finished_ms,
        message: snapshot.message,
        warning: snapshot.warning,
        metrics: ApplicationRolloutMetrics {
            inference_count: snapshot.metrics.inference_count,
            command_count: snapshot.metrics.command_count,
            missed_deadlines: snapshot.metrics.missed_deadlines,
            last_latency_ms: snapshot.metrics.last_latency_ms,
            last_model_ms: snapshot.metrics.last_model_ms,
            mean_latency_ms: snapshot.metrics.mean_latency_ms,
            max_latency_ms: snapshot.metrics.max_latency_ms,
        },
    }
}

pub(super) fn contract_rollout_snapshot(snapshot: ApplicationRolloutSnapshot) -> RolloutSnapshot {
    RolloutSnapshot {
        phase: match snapshot.phase {
            ApplicationRolloutPhase::Idle => spiral_robotics_contracts::api::RolloutPhase::Idle,
            ApplicationRolloutPhase::Loading => {
                spiral_robotics_contracts::api::RolloutPhase::Loading
            }
            ApplicationRolloutPhase::WarmingUp => {
                spiral_robotics_contracts::api::RolloutPhase::WarmingUp
            }
            ApplicationRolloutPhase::Running => {
                spiral_robotics_contracts::api::RolloutPhase::Running
            }
            ApplicationRolloutPhase::Stopping => {
                spiral_robotics_contracts::api::RolloutPhase::Stopping
            }
            ApplicationRolloutPhase::Failed => spiral_robotics_contracts::api::RolloutPhase::Failed,
        },
        session_id: snapshot.session_id,
        run_id: snapshot.run_id,
        checkpoint_step: snapshot.checkpoint_step,
        robot: snapshot.robot,
        duration_seconds: snapshot.duration_seconds,
        started_ms: snapshot.started_ms,
        finished_ms: snapshot.finished_ms,
        message: snapshot.message,
        warning: snapshot.warning,
        metrics: spiral_robotics_contracts::api::RolloutMetrics {
            inference_count: snapshot.metrics.inference_count,
            command_count: snapshot.metrics.command_count,
            missed_deadlines: snapshot.metrics.missed_deadlines,
            last_latency_ms: snapshot.metrics.last_latency_ms,
            last_model_ms: snapshot.metrics.last_model_ms,
            mean_latency_ms: snapshot.metrics.mean_latency_ms,
            max_latency_ms: snapshot.metrics.max_latency_ms,
        },
    }
}

async fn list_training_runs(State(state): State<AppState>) -> Json<TrainingSnapshot> {
    Json(contract_training_snapshot(
        training_application::list_training(state.training.as_ref()),
    ))
}

async fn start_training(
    State(state): State<AppState>,
    Json(request): Json<StartTrainingRequest>,
) -> Result<(StatusCode, Json<TrainingRun>), Failure> {
    let run =
        training_application::start_training(state.training.as_ref(), training_command(request))
            .await?;
    Ok((StatusCode::CREATED, Json(contract_training_run(run))))
}

async fn stop_training(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    training_application::stop_training(state.training.as_ref(), &id)?;
    Ok(StatusCode::ACCEPTED)
}

async fn delete_training(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, Failure> {
    let rollout = RolloutManager::snapshot(state.rollout.as_ref());
    let activity = TrainingRunActivity {
        rollout: rollout.phase.active().then_some(rollout.run_id).flatten(),
    };
    training_application::delete_training(
        state.training.as_ref(),
        activity,
        DeleteTrainingRunCommand { run_id: id },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_rollout(State(state): State<AppState>) -> Json<RolloutSnapshot> {
    Json(contract_rollout_snapshot(rollout_application::snapshot(
        &RolloutHttpAdapter(state.rollout.clone()),
    )))
}

async fn start_rollout(
    State(state): State<AppState>,
    Json(request): Json<StartRolloutRequest>,
) -> Result<(StatusCode, Json<RolloutSnapshot>), Failure> {
    let request =
        training_application::admit_rollout(state.training.as_ref(), rollout_command(request))?;
    let snapshot =
        rollout_application::start(&RolloutHttpAdapter(state.rollout.clone()), request).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(contract_rollout_snapshot(snapshot)),
    ))
}

async fn stop_rollout(State(state): State<AppState>) -> Result<StatusCode, Failure> {
    rollout_application::stop(&RolloutHttpAdapter(state.rollout.clone())).await?;
    Ok(StatusCode::ACCEPTED)
}
