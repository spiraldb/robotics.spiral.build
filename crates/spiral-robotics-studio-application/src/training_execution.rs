//! Application-owned admission and stop workflow for a remote training job.
//!
//! Concrete adapters resolve credentials, persist run history, and launch the
//! remote job. This module owns effect ordering and compensation around those
//! boundaries; runtime execution and checkpoint writes belong to the worker.

use crate::error::ApplicationError;
use crate::training::{
    StartTrainingCommand, TrainingReadiness, TrainingRun, TrainingRunPhase, TrainingSource,
};

const STARTING_MESSAGE: &str = "Opening SpiralDB and starting the remote training job…";
const STOPPING_MESSAGE: &str = "Finishing the current Spiral read or optimizer step…";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrainingRunIdentity {
    pub id: String,
    pub created_ms: i64,
}

/// Immutable input handed to the remote-job adapter after admission.
pub struct TrainingLaunch {
    pub run_id: String,
    pub command: StartTrainingCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunStateChange {
    pub phase: TrainingRunPhase,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
}

pub trait TrainingRunPort: Send + Sync {
    fn runtime_is_active(&self, run_id: &str) -> bool;
    fn transition_run(&self, run_id: &str, change: RunStateChange) -> Result<(), String>;
    fn signal_runtime_stop(&self, run_id: &str);
    fn mark_persistence_failure(&self, run_id: &str, error: &str);
    fn cleanup_runtime(&self, run_id: &str);
    fn now_ms(&self) -> i64;
}

pub trait TrainingCoordinatorPort: TrainingRunPort {
    async fn refresh_training_readiness(&self) -> TrainingReadiness;
    fn resolve_training_source(&self, dataset_id: &str)
    -> Result<TrainingSource, ApplicationError>;
    async fn validate_training_query(
        &self,
        command: &StartTrainingCommand,
    ) -> Result<(), ApplicationError>;
    fn next_training_identity(&self) -> TrainingRunIdentity;
    fn admit_training_run(&self, run: TrainingRun) -> Result<(), ApplicationError>;
    fn launch_training_runtime(&self, launch: TrainingLaunch) -> Result<(), ApplicationError>;
}

pub async fn coordinate_training_start<P>(
    port: &P,
    command: StartTrainingCommand,
) -> Result<TrainingRun, ApplicationError>
where
    P: TrainingCoordinatorPort + ?Sized,
{
    let readiness = port.refresh_training_readiness().await;
    if !readiness.ready {
        return Err(ApplicationError::Conflict(readiness.summary));
    }
    let source = port.resolve_training_source(&command.dataset_id)?;
    port.validate_training_query(&command).await?;
    let identity = port.next_training_identity();
    let run = TrainingRun {
        id: identity.id.clone(),
        config: command.clone(),
        source,
        phase: TrainingRunPhase::Starting,
        current_step: 0,
        started_ms: identity.created_ms,
        finished_ms: None,
        message: Some(STARTING_MESSAGE.to_owned()),
        metrics: Vec::new(),
        checkpoints: Vec::new(),
        logs: Vec::new(),
    };
    port.admit_training_run(run.clone())?;
    let launch = TrainingLaunch {
        run_id: run.id.clone(),
        command,
    };
    if let Err(error) = port.launch_training_runtime(launch) {
        fail_run(port, &run.id, error.message().to_owned());
        port.cleanup_runtime(&run.id);
        return Err(error);
    }
    Ok(run)
}

pub fn coordinate_training_stop<P>(port: &P, run_id: &str) -> Result<(), ApplicationError>
where
    P: TrainingRunPort + ?Sized,
{
    if !port.runtime_is_active(run_id) {
        return Err(ApplicationError::Conflict(format!(
            "training run {run_id:?} is not active"
        )));
    }
    let transition = port.transition_run(
        run_id,
        RunStateChange {
            phase: TrainingRunPhase::Stopping,
            finished_ms: None,
            message: Some(STOPPING_MESSAGE.to_owned()),
        },
    );
    port.signal_runtime_stop(run_id);
    if let Err(error) = transition {
        port.mark_persistence_failure(run_id, &error);
        return Err(ApplicationError::Conflict(error));
    }
    Ok(())
}

fn fail_run<P>(port: &P, run_id: &str, message: String)
where
    P: TrainingRunPort + ?Sized,
{
    if let Err(error) = port.transition_run(
        run_id,
        RunStateChange {
            phase: TrainingRunPhase::Failed,
            finished_ms: Some(port.now_ms()),
            message: Some(message),
        },
    ) {
        port.mark_persistence_failure(run_id, &error);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::training::{ActTrainingSettings, TrainingPolicy};

    struct FakePort {
        events: Mutex<Vec<String>>,
        readiness: TrainingReadiness,
        active: bool,
        fail: Option<&'static str>,
    }

    impl FakePort {
        fn ready() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                readiness: TrainingReadiness {
                    ready: true,
                    spiral_ready: true,
                    runtime: None,
                    summary: "ready".to_owned(),
                },
                active: true,
                fail: None,
            }
        }

        fn with_failure(fail: &'static str) -> Self {
            Self {
                fail: Some(fail),
                ..Self::ready()
            }
        }

        fn push(&self, event: impl Into<String>) {
            self.events.lock().unwrap().push(event.into());
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl TrainingRunPort for FakePort {
        fn runtime_is_active(&self, run_id: &str) -> bool {
            self.push(format!("active:{run_id}"));
            self.active
        }
        fn transition_run(&self, run_id: &str, change: RunStateChange) -> Result<(), String> {
            self.push(format!("transition:{run_id}:{:?}", change.phase));
            if self.fail == Some("transition") {
                return Err("transition failed".to_owned());
            }
            Ok(())
        }
        fn signal_runtime_stop(&self, run_id: &str) {
            self.push(format!("stop:{run_id}"));
        }
        fn mark_persistence_failure(&self, run_id: &str, error: &str) {
            self.push(format!("persistence:{run_id}:{error}"));
        }
        fn cleanup_runtime(&self, run_id: &str) {
            self.push(format!("cleanup:{run_id}"));
        }
        fn now_ms(&self) -> i64 {
            42
        }
    }

    impl TrainingCoordinatorPort for FakePort {
        async fn refresh_training_readiness(&self) -> TrainingReadiness {
            self.push("readiness");
            self.readiness.clone()
        }
        fn resolve_training_source(
            &self,
            dataset_id: &str,
        ) -> Result<TrainingSource, ApplicationError> {
            self.push(format!("resolve:{dataset_id}"));
            Ok(TrainingSource {
                dataset_id: dataset_id.to_owned(),
            })
        }
        async fn validate_training_query(
            &self,
            _command: &StartTrainingCommand,
        ) -> Result<(), ApplicationError> {
            self.push("validate");
            if self.fail == Some("validate") {
                return Err(ApplicationError::Conflict("query failed".to_owned()));
            }
            Ok(())
        }
        fn next_training_identity(&self) -> TrainingRunIdentity {
            self.push("identity");
            TrainingRunIdentity {
                id: "run-1".to_owned(),
                created_ms: 7,
            }
        }
        fn admit_training_run(&self, run: TrainingRun) -> Result<(), ApplicationError> {
            self.push(format!("admit:{}", run.id));
            if self.fail == Some("admit") {
                return Err(ApplicationError::Conflict("admission failed".to_owned()));
            }
            Ok(())
        }
        fn launch_training_runtime(&self, launch: TrainingLaunch) -> Result<(), ApplicationError> {
            self.push(format!("launch:{}", launch.run_id));
            if self.fail == Some("launch") {
                return Err(ApplicationError::Internal("launch failed".to_owned()));
            }
            Ok(())
        }
    }

    fn command() -> StartTrainingCommand {
        StartTrainingCommand {
            run_name: "run".to_owned(),
            dataset_id: "dataset".to_owned(),
            policy: TrainingPolicy::Act,
            query: "SELECT * FROM episodes".to_owned(),
            state_column: "state".to_owned(),
            action_column: "action".to_owned(),
            device: "cpu".to_owned(),
            steps: 10,
            batch_size: 1,
            validation_split: 0.1,
            learning_rate: 0.001,
            checkpoint_every: 5,
            chunk_size: 4,
            seed: 1,
            act: ActTrainingSettings {
                camera_streams: vec!["front".to_owned()],
                image_width: 32,
                image_height: 32,
                video_decoder: "auto".to_owned(),
                pretrained_backbone: false,
                hidden_dim: 64,
                latent_dim: 4,
                encoder_layers: 1,
                vae_encoder_layers: 1,
                decoder_layers: 1,
                attention_heads: 1,
                feedforward_dim: 64,
                kl_weight: 1.0,
            },
        }
    }

    #[tokio::test]
    async fn start_orders_all_effects_before_launch() {
        let port = FakePort::ready();
        let run = coordinate_training_start(&port, command()).await.unwrap();
        assert_eq!(run.id, "run-1");
        assert_eq!(
            port.events(),
            [
                "readiness",
                "resolve:dataset",
                "validate",
                "identity",
                "admit:run-1",
                "launch:run-1",
            ]
        );
    }

    #[tokio::test]
    async fn failed_readiness_short_circuits_start() {
        let mut port = FakePort::ready();
        port.readiness.ready = false;
        port.readiness.summary = "not ready".to_owned();
        assert!(coordinate_training_start(&port, command()).await.is_err());
        assert_eq!(port.events(), ["readiness"]);
    }

    #[tokio::test]
    async fn launch_failure_terminalizes_the_admitted_run() {
        let port = FakePort::with_failure("launch");
        assert!(coordinate_training_start(&port, command()).await.is_err());
        assert_eq!(
            port.events(),
            [
                "readiness",
                "resolve:dataset",
                "validate",
                "identity",
                "admit:run-1",
                "launch:run-1",
                "transition:run-1:Failed",
                "cleanup:run-1",
            ]
        );
    }

    #[test]
    fn stop_signals_runtime_even_when_state_persistence_fails() {
        let port = FakePort::with_failure("transition");
        assert!(coordinate_training_stop(&port, "run-1").is_err());
        assert_eq!(
            port.events(),
            [
                "active:run-1",
                "transition:run-1:Stopping",
                "stop:run-1",
                "persistence:run-1:transition failed",
            ]
        );
    }

    #[test]
    fn inactive_stop_has_no_transition_or_cancel_effect() {
        let port = FakePort {
            active: false,
            ..FakePort::ready()
        };
        assert!(coordinate_training_stop(&port, "run-1").is_err());
        assert_eq!(port.events(), ["active:run-1"]);
    }
}
