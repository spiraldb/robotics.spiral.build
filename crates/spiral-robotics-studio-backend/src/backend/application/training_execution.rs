//! Application-owned execution workflow for an injected training backend.
//!
//! Concrete adapters resolve credentials, validate ADBC queries, persist run
//! history, launch tasks, and publish checkpoint bytes. This module owns the
//! ordering, compensation, event acceptance, and terminal-state policy around
//! those effects.

use crate::backend::application::error::ApplicationError;
use crate::backend::application::training::{
    StartTrainingCommand, TrainingMetric, TrainingReadiness, TrainingRun, TrainingRunPhase,
    TrainingSource,
};

const STARTING_MESSAGE: &str = "Opening the Spiral ADBC stream and training backend…";
const STOPPING_MESSAGE: &str =
    "Finishing the current Arrow read or optimizer step and publishing a final checkpoint…";
const DRAINED_CHECKPOINT_ERROR: &str = "trainer completed before checkpoint publication";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrainingRunIdentity {
    pub id: String,
    pub created_ms: i64,
}

/// A plain source identity paired with an adapter-owned launch capability.
pub(crate) struct ResolvedTrainingSource<T> {
    pub source: TrainingSource,
    pub handle: T,
}

/// Immutable input handed back to the runtime adapter after admission.
pub(crate) struct TrainingLaunch<T> {
    pub run_id: String,
    pub command: StartTrainingCommand,
    pub source: TrainingSource,
    pub source_handle: T,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunStateChange {
    pub phase: TrainingRunPhase,
    pub finished_ms: Option<i64>,
    pub message: Option<String>,
}

pub(crate) trait TrainingRunPort: Send + Sync {
    fn runtime_is_active(&self, run_id: &str) -> bool;

    fn transition_run(&self, run_id: &str, change: RunStateChange) -> Result<(), String>;

    fn signal_runtime_stop(&self, run_id: &str);

    fn mark_persistence_failure(&self, run_id: &str, error: &str);

    fn cleanup_runtime(&self, run_id: &str);

    fn now_ms(&self) -> i64;
}

/// Effects required by start admission. `SourceHandle` is deliberately opaque:
/// it is minted by `resolve_training_source` and consumed only by
/// `launch_training_runtime`, so credentials and database types cannot escape
/// into policy.
pub(crate) trait TrainingCoordinatorPort: TrainingRunPort {
    type SourceHandle;

    async fn refresh_training_readiness(&self) -> TrainingReadiness;

    fn resolve_training_source(
        &self,
        dataset_id: &str,
    ) -> Result<ResolvedTrainingSource<Self::SourceHandle>, ApplicationError>;

    async fn validate_training_query(
        &self,
        handle: &Self::SourceHandle,
        command: &StartTrainingCommand,
    ) -> Result<(), ApplicationError>;

    fn next_training_identity(&self) -> TrainingRunIdentity;

    fn register_training_source(
        &self,
        run_id: &str,
        source: &TrainingSource,
    ) -> Result<(), ApplicationError>;

    fn unregister_training_source(&self, run_id: &str, source: &TrainingSource);

    fn admit_training_run(&self, run: TrainingRun) -> Result<(), ApplicationError>;

    fn launch_training_runtime(
        &self,
        launch: TrainingLaunch<Self::SourceHandle>,
    ) -> Result<(), ApplicationError>;
}

pub(crate) async fn coordinate_training_start<P>(
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

    let resolved = port.resolve_training_source(&command.dataset_id)?;
    port.validate_training_query(&resolved.handle, &command)
        .await?;
    let identity = port.next_training_identity();
    let run = TrainingRun {
        id: identity.id.clone(),
        config: command.clone(),
        source: resolved.source.clone(),
        phase: TrainingRunPhase::Starting,
        current_step: 0,
        started_ms: identity.created_ms,
        finished_ms: None,
        message: Some(STARTING_MESSAGE.to_owned()),
        metrics: Vec::new(),
        checkpoints: Vec::new(),
        logs: Vec::new(),
    };

    port.register_training_source(&run.id, &run.source)?;
    if let Err(error) = port.admit_training_run(run.clone()) {
        port.unregister_training_source(&run.id, &run.source);
        return Err(error);
    }

    let launch = TrainingLaunch {
        run_id: run.id.clone(),
        command,
        source: resolved.source,
        source_handle: resolved.handle,
    };
    if let Err(error) = port.launch_training_runtime(launch) {
        fail_run(port, &run.id, error.message().to_owned());
        port.cleanup_runtime(&run.id);
        return Err(error);
    }
    Ok(run)
}

pub(crate) fn coordinate_training_stop<P>(port: &P, run_id: &str) -> Result<(), ApplicationError>
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
    // Preserve the safety property that a stop request reaches the worker even
    // when durable state publication fails.
    port.signal_runtime_stop(run_id);
    if let Err(error) = transition {
        port.mark_persistence_failure(run_id, &error);
        return Err(ApplicationError::Conflict(error));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeEventWindow {
    Live,
    Draining,
}

pub(crate) enum TrainingRuntimeEvent<T> {
    Configured(Box<StartTrainingCommand>),
    Started(String),
    Metric(TrainingMetric),
    Checkpoint(T),
    Log(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TrainingRuntimeOutcome {
    pub steps: u64,
    pub stopped: bool,
}

pub(crate) struct TrainingRuntimeSession {
    run_id: String,
    resolved_command: StartTrainingCommand,
    source: TrainingSource,
    publication_failure: Option<String>,
}

impl TrainingRuntimeSession {
    pub(crate) fn new(
        run_id: String,
        command: StartTrainingCommand,
        source: TrainingSource,
    ) -> Self {
        Self {
            run_id,
            resolved_command: command,
            source,
            publication_failure: None,
        }
    }
}

/// Runtime-side effects. `CheckpointEnvelope` may contain native tensors or an
/// acknowledgement channel, but is closed-loop between the runtime adapter and
/// its checkpoint publisher/rejector.
pub(crate) trait TrainingRuntimePort: TrainingRunPort {
    type CheckpointEnvelope;

    fn run_phase(&self, run_id: &str) -> Option<TrainingRunPhase>;

    fn persist_resolved_configuration(
        &self,
        run_id: &str,
        command: &StartTrainingCommand,
    ) -> Result<(), String>;

    fn record_metric(&self, run_id: &str, metric: TrainingMetric);

    fn record_log(&self, run_id: &str, message: String);

    async fn publish_runtime_checkpoint(
        &self,
        run_id: &str,
        command: &StartTrainingCommand,
        source: &TrainingSource,
        checkpoint: Self::CheckpointEnvelope,
    ) -> Result<(), String>;

    fn reject_runtime_checkpoint(&self, checkpoint: Self::CheckpointEnvelope, error: String);
}

pub(crate) async fn accept_runtime_event<P, T>(
    port: &P,
    session: &mut TrainingRuntimeSession,
    event: TrainingRuntimeEvent<T>,
    window: RuntimeEventWindow,
) where
    P: TrainingRuntimePort<CheckpointEnvelope = T> + ?Sized,
{
    if let Some(error) = session.publication_failure.clone() {
        if let TrainingRuntimeEvent::Checkpoint(checkpoint) = event {
            port.reject_runtime_checkpoint(checkpoint, error);
        }
        return;
    }

    match event {
        TrainingRuntimeEvent::Checkpoint(checkpoint) if window == RuntimeEventWindow::Draining => {
            let error = DRAINED_CHECKPOINT_ERROR.to_owned();
            port.reject_runtime_checkpoint(checkpoint, error.clone());
            fail_run(port, &session.run_id, error.clone());
            session.publication_failure = Some(error);
        }
        TrainingRuntimeEvent::Configured(command) => {
            session.resolved_command = *command;
            if let Err(error) =
                port.persist_resolved_configuration(&session.run_id, &session.resolved_command)
            {
                persistence_failure(port, session, error);
            }
        }
        TrainingRuntimeEvent::Started(message) => {
            if port.run_phase(&session.run_id) == Some(TrainingRunPhase::Starting)
                && let Err(error) = port.transition_run(
                    &session.run_id,
                    RunStateChange {
                        phase: TrainingRunPhase::Running,
                        finished_ms: None,
                        message: Some(message),
                    },
                )
            {
                persistence_failure(port, session, error);
            }
        }
        TrainingRuntimeEvent::Metric(metric) => port.record_metric(&session.run_id, metric),
        TrainingRuntimeEvent::Log(message) => port.record_log(&session.run_id, message),
        TrainingRuntimeEvent::Checkpoint(checkpoint) => {
            if let Err(error) = port
                .publish_runtime_checkpoint(
                    &session.run_id,
                    &session.resolved_command,
                    &session.source,
                    checkpoint,
                )
                .await
            {
                port.signal_runtime_stop(&session.run_id);
                fail_run(port, &session.run_id, error.clone());
                session.publication_failure = Some(error);
            }
        }
    }
}

pub(crate) fn complete_runtime<P>(
    port: &P,
    session: TrainingRuntimeSession,
    terminal: Result<TrainingRuntimeOutcome, String>,
) where
    P: TrainingRunPort + ?Sized,
{
    port.cleanup_runtime(&session.run_id);
    if session.publication_failure.is_some() {
        return;
    }

    let change = match terminal {
        Err(error) => RunStateChange {
            phase: TrainingRunPhase::Failed,
            finished_ms: Some(port.now_ms()),
            message: Some(error),
        },
        Ok(outcome) if outcome.stopped && outcome.steps == 0 => RunStateChange {
            phase: TrainingRunPhase::Stopped,
            finished_ms: Some(port.now_ms()),
            message: Some(
                "Stopped before the first optimizer step; no checkpoint was produced.".to_owned(),
            ),
        },
        Ok(outcome) if outcome.stopped => RunStateChange {
            phase: TrainingRunPhase::Stopped,
            finished_ms: Some(port.now_ms()),
            message: Some(format!(
                "Stopped after step {}; the final checkpoint was published to Spiral.",
                outcome.steps
            )),
        },
        Ok(outcome) => RunStateChange {
            phase: TrainingRunPhase::Completed,
            finished_ms: Some(port.now_ms()),
            message: Some(format!(
                "Completed {} steps; the final checkpoint was published to Spiral.",
                outcome.steps
            )),
        },
    };
    if let Err(error) = port.transition_run(&session.run_id, change) {
        port.mark_persistence_failure(&session.run_id, &error);
    }
}

fn persistence_failure<P>(port: &P, session: &mut TrainingRuntimeSession, error: String)
where
    P: TrainingRunPort + ?Sized,
{
    port.signal_runtime_stop(&session.run_id);
    port.mark_persistence_failure(&session.run_id, &error);
    session.publication_failure = Some(error);
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
    use crate::backend::application::training::{ActTrainingSettings, TrainingPolicy};

    struct FakePort {
        events: Mutex<Vec<String>>,
        readiness: TrainingReadiness,
        phase: Mutex<Option<TrainingRunPhase>>,
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
                    summary: "ready".to_owned(),
                },
                phase: Mutex::new(Some(TrainingRunPhase::Starting)),
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
            *self.phase.lock().unwrap() = Some(change.phase);
            Ok(())
        }

        fn signal_runtime_stop(&self, run_id: &str) {
            self.push(format!("stop:{run_id}"));
        }

        fn mark_persistence_failure(&self, run_id: &str, error: &str) {
            self.push(format!("persistence:{run_id}:{error}"));
            *self.phase.lock().unwrap() = Some(TrainingRunPhase::Failed);
        }

        fn cleanup_runtime(&self, run_id: &str) {
            self.push(format!("cleanup:{run_id}"));
        }

        fn now_ms(&self) -> i64 {
            42
        }
    }

    impl TrainingCoordinatorPort for FakePort {
        type SourceHandle = String;

        async fn refresh_training_readiness(&self) -> TrainingReadiness {
            self.push("readiness");
            self.readiness.clone()
        }

        fn resolve_training_source(
            &self,
            dataset_id: &str,
        ) -> Result<ResolvedTrainingSource<Self::SourceHandle>, ApplicationError> {
            self.push(format!("resolve:{dataset_id}"));
            Ok(ResolvedTrainingSource {
                source: source(),
                handle: "credentials".to_owned(),
            })
        }

        async fn validate_training_query(
            &self,
            _handle: &Self::SourceHandle,
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

        fn register_training_source(
            &self,
            run_id: &str,
            _source: &TrainingSource,
        ) -> Result<(), ApplicationError> {
            self.push(format!("register:{run_id}"));
            Ok(())
        }

        fn unregister_training_source(&self, run_id: &str, _source: &TrainingSource) {
            self.push(format!("unregister:{run_id}"));
        }

        fn admit_training_run(&self, run: TrainingRun) -> Result<(), ApplicationError> {
            self.push(format!("admit:{}", run.id));
            if self.fail == Some("admit") {
                return Err(ApplicationError::Conflict("admission failed".to_owned()));
            }
            Ok(())
        }

        fn launch_training_runtime(
            &self,
            launch: TrainingLaunch<Self::SourceHandle>,
        ) -> Result<(), ApplicationError> {
            self.push(format!("launch:{}", launch.run_id));
            if self.fail == Some("launch") {
                return Err(ApplicationError::Internal("launch failed".to_owned()));
            }
            Ok(())
        }
    }

    impl TrainingRuntimePort for FakePort {
        type CheckpointEnvelope = String;

        fn run_phase(&self, run_id: &str) -> Option<TrainingRunPhase> {
            self.push(format!("phase:{run_id}"));
            *self.phase.lock().unwrap()
        }

        fn persist_resolved_configuration(
            &self,
            run_id: &str,
            _command: &StartTrainingCommand,
        ) -> Result<(), String> {
            self.push(format!("configured:{run_id}"));
            if self.fail == Some("configured") {
                return Err("configuration persistence failed".to_owned());
            }
            Ok(())
        }

        fn record_metric(&self, run_id: &str, metric: TrainingMetric) {
            self.push(format!("metric:{run_id}:{}", metric.step));
        }

        fn record_log(&self, run_id: &str, message: String) {
            self.push(format!("log:{run_id}:{message}"));
        }

        async fn publish_runtime_checkpoint(
            &self,
            run_id: &str,
            _command: &StartTrainingCommand,
            _source: &TrainingSource,
            checkpoint: Self::CheckpointEnvelope,
        ) -> Result<(), String> {
            self.push(format!("checkpoint:{run_id}:{checkpoint}"));
            if self.fail == Some("checkpoint") {
                return Err("checkpoint failed".to_owned());
            }
            Ok(())
        }

        fn reject_runtime_checkpoint(&self, checkpoint: Self::CheckpointEnvelope, error: String) {
            self.push(format!("reject:{checkpoint}:{error}"));
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
                video_decoder: "native".to_owned(),
                pretrained_backbone: false,
                hidden_dim: 32,
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

    fn source() -> TrainingSource {
        TrainingSource {
            project: "project".to_owned(),
            dataset: "namespace".to_owned(),
            dataset_id: "dataset".to_owned(),
        }
    }

    fn session() -> TrainingRuntimeSession {
        TrainingRuntimeSession::new("run-1".to_owned(), command(), source())
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
                "register:run-1",
                "admit:run-1",
                "launch:run-1",
            ]
        );
    }

    #[tokio::test]
    async fn failed_readiness_short_circuits_every_start_effect() {
        let mut port = FakePort::ready();
        port.readiness.ready = false;
        port.readiness.summary = "not ready".to_owned();
        let error = coordinate_training_start(&port, command())
            .await
            .unwrap_err();
        assert_eq!(error, ApplicationError::Conflict("not ready".to_owned()));
        assert_eq!(port.events(), ["readiness"]);
    }

    #[tokio::test]
    async fn query_failure_precedes_identity_registration_and_admission() {
        let port = FakePort::with_failure("validate");
        assert!(coordinate_training_start(&port, command()).await.is_err());
        assert_eq!(port.events(), ["readiness", "resolve:dataset", "validate"]);
    }

    #[tokio::test]
    async fn admission_failure_unregisters_the_immutable_source() {
        let port = FakePort::with_failure("admit");
        assert!(coordinate_training_start(&port, command()).await.is_err());
        assert_eq!(
            port.events(),
            [
                "readiness",
                "resolve:dataset",
                "validate",
                "identity",
                "register:run-1",
                "admit:run-1",
                "unregister:run-1",
            ]
        );
    }

    #[tokio::test]
    async fn launch_failure_terminalizes_the_admitted_run_before_cleanup() {
        let port = FakePort::with_failure("launch");
        assert!(coordinate_training_start(&port, command()).await.is_err());
        assert_eq!(
            port.events(),
            [
                "readiness",
                "resolve:dataset",
                "validate",
                "identity",
                "register:run-1",
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

    #[tokio::test]
    async fn runtime_events_are_accepted_in_policy_order() {
        let port = FakePort::ready();
        let mut session = session();
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Configured(Box::new(command())),
            RuntimeEventWindow::Live,
        )
        .await;
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Started("started".to_owned()),
            RuntimeEventWindow::Live,
        )
        .await;
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Metric(TrainingMetric {
                step: 3,
                ..TrainingMetric::default()
            }),
            RuntimeEventWindow::Live,
        )
        .await;
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Log("hello".to_owned()),
            RuntimeEventWindow::Live,
        )
        .await;
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Checkpoint("cp".to_owned()),
            RuntimeEventWindow::Live,
        )
        .await;
        complete_runtime(
            &port,
            session,
            Ok(TrainingRuntimeOutcome {
                steps: 10,
                stopped: false,
            }),
        );
        assert_eq!(
            port.events(),
            [
                "configured:run-1",
                "phase:run-1",
                "transition:run-1:Running",
                "metric:run-1:3",
                "log:run-1:hello",
                "checkpoint:run-1:cp",
                "cleanup:run-1",
                "transition:run-1:Completed",
            ]
        );
    }

    #[tokio::test]
    async fn checkpoint_failure_stops_and_terminalizes_once() {
        let port = FakePort::with_failure("checkpoint");
        let mut session = session();
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Checkpoint("cp".to_owned()),
            RuntimeEventWindow::Live,
        )
        .await;
        assert_eq!(
            session.publication_failure.as_deref(),
            Some("checkpoint failed")
        );
        complete_runtime(
            &port,
            session,
            Ok(TrainingRuntimeOutcome {
                steps: 1,
                stopped: true,
            }),
        );
        assert_eq!(
            port.events(),
            [
                "checkpoint:run-1:cp",
                "stop:run-1",
                "transition:run-1:Failed",
                "cleanup:run-1",
            ]
        );
    }

    #[tokio::test]
    async fn configuration_persistence_failure_stops_and_blocks_later_events() {
        let port = FakePort::with_failure("configured");
        let mut session = session();
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Configured(Box::new(command())),
            RuntimeEventWindow::Live,
        )
        .await;
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Metric(TrainingMetric {
                step: 3,
                ..TrainingMetric::default()
            }),
            RuntimeEventWindow::Live,
        )
        .await;
        assert_eq!(
            port.events(),
            [
                "configured:run-1",
                "stop:run-1",
                "persistence:run-1:configuration persistence failed",
            ]
        );
    }

    #[test]
    fn terminal_outcomes_are_classified_after_runtime_cleanup() {
        let cases = [
            (
                Ok(TrainingRuntimeOutcome {
                    steps: 0,
                    stopped: true,
                }),
                TrainingRunPhase::Stopped,
            ),
            (
                Ok(TrainingRuntimeOutcome {
                    steps: 4,
                    stopped: true,
                }),
                TrainingRunPhase::Stopped,
            ),
            (
                Ok(TrainingRuntimeOutcome {
                    steps: 10,
                    stopped: false,
                }),
                TrainingRunPhase::Completed,
            ),
            (Err("trainer failed".to_owned()), TrainingRunPhase::Failed),
        ];
        for (terminal, phase) in cases {
            let port = FakePort::ready();
            complete_runtime(&port, session(), terminal);
            assert_eq!(
                port.events(),
                [
                    "cleanup:run-1".to_owned(),
                    format!("transition:run-1:{phase:?}"),
                ]
            );
        }
    }

    #[tokio::test]
    async fn drained_checkpoint_is_rejected_and_cannot_leave_a_live_run() {
        let port = FakePort::ready();
        let mut session = session();
        accept_runtime_event(
            &port,
            &mut session,
            TrainingRuntimeEvent::Checkpoint("late".to_owned()),
            RuntimeEventWindow::Draining,
        )
        .await;
        assert_eq!(
            port.events(),
            [
                "reject:late:trainer completed before checkpoint publication",
                "transition:run-1:Failed",
            ]
        );
    }
}
