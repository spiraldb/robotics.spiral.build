//! Capability-owned admission and stop workflow for a remote training job.
//!
//! Concrete adapters resolve credentials, persist run history, and launch the
//! remote job. This module owns effect ordering and compensation around those
//! boundaries; runtime execution and checkpoint writes belong to the worker.

use crate::OperationError;
use spiral_robotics_api::training::{
    TrainingConfig, TrainingReadiness, TrainingRun, TrainingRunPhase, TrainingSource,
};

const STARTING_MESSAGE: &str = "Opening SpiralDB and starting the remote training job…";
const STOPPING_MESSAGE: &str = "Finishing the current Spiral read or optimizer step…";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrainingRunIdentity {
    id: String,
    created_ms: i64,
}

/// Immutable input handed to the remote-job adapter after admission.
pub(crate) struct TrainingLaunch {
    run_id: String,
    command: TrainingConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunStateChange {
    phase: TrainingRunPhase,
    finished_ms: Option<i64>,
    message: Option<String>,
}

impl TrainingLaunch {
    pub(crate) fn new(run_id: impl Into<String>, command: TrainingConfig) -> Self {
        Self {
            run_id: run_id.into(),
            command,
        }
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn into_parts(self) -> (String, TrainingConfig) {
        (self.run_id, self.command)
    }
}

impl TrainingRunIdentity {
    pub(crate) fn new(id: impl Into<String>, created_ms: i64) -> Self {
        Self {
            id: id.into(),
            created_ms,
        }
    }
}

impl RunStateChange {
    pub(crate) const fn phase(&self) -> TrainingRunPhase {
        self.phase
    }
    pub(crate) const fn finished_ms(&self) -> Option<i64> {
        self.finished_ms
    }
    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

pub(crate) trait TrainingRunPort: Send + Sync {
    fn runtime_is_active(&self, run_id: &str) -> bool;
    async fn transition_run(&self, run_id: &str, change: RunStateChange) -> Result<(), String>;
    fn signal_runtime_stop(&self, run_id: &str);
    fn mark_persistence_failure(&self, run_id: &str, error: &str);
    fn cleanup_runtime(&self, run_id: &str);
    fn now_ms(&self) -> i64;

    async fn fail_run(&self, run_id: &str, message: String) {
        if let Err(error) = self
            .transition_run(
                run_id,
                RunStateChange {
                    phase: TrainingRunPhase::Failed,
                    finished_ms: Some(self.now_ms()),
                    message: Some(message),
                },
            )
            .await
        {
            self.mark_persistence_failure(run_id, &error);
        }
    }

    async fn coordinate_training_stop(&self, run_id: &str) -> Result<(), OperationError> {
        if !self.runtime_is_active(run_id) {
            return Err(OperationError::Conflict(format!(
                "training run {run_id:?} is not active"
            )));
        }
        let transition = self
            .transition_run(
                run_id,
                RunStateChange {
                    phase: TrainingRunPhase::Stopping,
                    finished_ms: None,
                    message: Some(STOPPING_MESSAGE.to_owned()),
                },
            )
            .await;
        self.signal_runtime_stop(run_id);
        if let Err(error) = transition {
            self.mark_persistence_failure(run_id, &error);
            return Err(OperationError::Conflict(error));
        }
        Ok(())
    }
}

pub(crate) trait TrainingCoordinatorPort: TrainingRunPort {
    async fn refresh_training_readiness(&self) -> TrainingReadiness;
    fn resolve_training_source(&self, dataset_id: &str) -> Result<TrainingSource, OperationError>;
    async fn validate_training_query(&self, command: &TrainingConfig)
    -> Result<(), OperationError>;
    fn next_training_identity(&self) -> TrainingRunIdentity;
    async fn admit_training_run(&self, run: TrainingRun) -> Result<(), OperationError>;
    fn launch_training_runtime(&self, launch: TrainingLaunch) -> Result<(), OperationError>;

    async fn coordinate_training_start(
        &self,
        command: TrainingConfig,
    ) -> Result<TrainingRun, OperationError> {
        let readiness = self.refresh_training_readiness().await;
        if !readiness.ready() {
            return Err(OperationError::Conflict(readiness.summary().to_owned()));
        }
        let source = self.resolve_training_source(command.dataset_id())?;
        self.validate_training_query(&command).await?;
        let identity = self.next_training_identity();
        let run = TrainingRun::starting(identity.id.clone(), command.clone())
            .with_source(source)
            .with_started_ms(identity.created_ms)
            .with_completion(None, Some(STARTING_MESSAGE.to_owned()));
        self.admit_training_run(run.clone()).await?;
        let launch = TrainingLaunch::new(run.id(), command);
        if let Err(error) = self.launch_training_runtime(launch) {
            self.fail_run(run.id(), error.message().to_owned()).await;
            self.cleanup_runtime(run.id());
            return Err(error);
        }
        Ok(run)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
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
                readiness: TrainingReadiness::new(true, true, "ready"),
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
        async fn transition_run(&self, run_id: &str, change: RunStateChange) -> Result<(), String> {
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
        ) -> Result<TrainingSource, OperationError> {
            self.push(format!("resolve:{dataset_id}"));
            Ok(TrainingSource::new(dataset_id))
        }
        async fn validate_training_query(
            &self,
            _command: &TrainingConfig,
        ) -> Result<(), OperationError> {
            self.push("validate");
            if self.fail == Some("validate") {
                return Err(OperationError::Conflict("query failed".to_owned()));
            }
            Ok(())
        }
        fn next_training_identity(&self) -> TrainingRunIdentity {
            self.push("identity");
            TrainingRunIdentity::new("run-1", 7)
        }
        async fn admit_training_run(&self, run: TrainingRun) -> Result<(), OperationError> {
            self.push(format!("admit:{}", run.id()));
            if self.fail == Some("admit") {
                return Err(OperationError::Conflict("admission failed".to_owned()));
            }
            Ok(())
        }
        fn launch_training_runtime(&self, launch: TrainingLaunch) -> Result<(), OperationError> {
            self.push(format!("launch:{}", launch.run_id()));
            if self.fail == Some("launch") {
                return Err(OperationError::Internal("launch failed".to_owned()));
            }
            Ok(())
        }
    }

    fn command() -> TrainingConfig {
        let mut config = TrainingConfig::for_dataset("run", "dataset", "SELECT * FROM episodes");
        config.set_state_column("state");
        config.set_action_column("action");
        config.set_device("cpu");
        config.set_steps(10);
        config.set_batch_size(1);
        config.set_validation_split(0.1);
        config.set_learning_rate(0.001);
        config.set_checkpoint_every(5);
        config.set_chunk_size(4);
        *config.act_mut() = spiral_robotics_api::training::ActTrainingConfig::new(
            spiral_robotics_api::training::ActInputConfig::new(
                vec!["front".to_owned()],
                (32, 32),
                "auto",
            ),
            spiral_robotics_api::training::ActArchitectureConfig::new(
                64,
                1,
                spiral_robotics_api::training::ActLayerCounts::new(1, 1, 1),
            ),
            spiral_robotics_api::training::ActAttentionConfig::new(4, 64, 1.0),
        );
        config
    }

    #[tokio::test]
    async fn start_orders_all_effects_before_launch() {
        let port = FakePort::ready();
        let run = port.coordinate_training_start(command()).await.unwrap();
        assert_eq!(run.id(), "run-1");
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
        port.readiness = TrainingReadiness::new(false, true, "not ready");
        assert!(port.coordinate_training_start(command()).await.is_err());
        assert_eq!(port.events(), ["readiness"]);
    }

    #[tokio::test]
    async fn launch_failure_terminalizes_the_admitted_run() {
        let port = FakePort::with_failure("launch");
        assert!(port.coordinate_training_start(command()).await.is_err());
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

    #[tokio::test]
    async fn stop_signals_runtime_even_when_state_persistence_fails() {
        let port = FakePort::with_failure("transition");
        assert!(port.coordinate_training_stop("run-1").await.is_err());
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

    #[tokio::test]
    async fn inactive_stop_has_no_transition_or_cancel_effect() {
        let port = FakePort {
            active: false,
            ..FakePort::ready()
        };
        assert!(port.coordinate_training_stop("run-1").await.is_err());
        assert_eq!(port.events(), ["active:run-1"]);
    }
}
