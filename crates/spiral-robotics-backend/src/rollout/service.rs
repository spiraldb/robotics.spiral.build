//! Pure lifecycle coordination for one remotely computed policy rollout.
//!
//! Runtime tasks, model formats, cameras, clocks, and motor actors remain
//! adapters. This module owns which session may exist, what its semantic state
//! is, how a stop is classified, and the mandatory cleanup barrier before a
//! session becomes terminal.

use std::sync::Arc;

use spiral_robotics_core::operation::RolloutDuration;

use super::policy::{ControlOwner, ExpectedPolicyOwner, PolicyOwnership};
use spiral_robotics_core::OperationError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RolloutRequest {
    run_id: String,
    checkpoint_step: u64,
    robot: String,
    duration_seconds: u32,
}

impl RolloutRequest {
    pub(crate) fn new(run_id: String, checkpoint_step: u64, robot: String) -> Self {
        Self {
            run_id,
            checkpoint_step,
            robot,
            duration_seconds: 0,
        }
    }

    pub(crate) fn with_duration(mut self, duration_seconds: u32) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn checkpoint_step(&self) -> u64 {
        self.checkpoint_step
    }

    pub(crate) fn robot(&self) -> &str {
        &self.robot
    }

    pub(crate) fn duration_seconds(&self) -> u32 {
        self.duration_seconds
    }
}

/// Publication seam. Delivery adapters map this semantic snapshot to their
/// own DTO or event protocol.
pub(crate) trait RolloutEventPort: Send + Sync {
    fn publish(&self, snapshot: &RolloutSnapshot);
}

/// Capability-facing execution seam implemented by the backend runtime.
/// The runtime may use Tokio, model libraries, cameras, and hardware drivers,
/// but only semantic rollout state crosses this boundary.
pub(crate) trait RolloutSessionPort: Send + Sync {
    fn snapshot(&self) -> RolloutSnapshot;

    async fn start(&self, request: RolloutRequest) -> Result<RolloutSnapshot, OperationError>;

    async fn stop(&self) -> Result<(), OperationError>;
}

pub(crate) fn snapshot<P: RolloutSessionPort + ?Sized>(port: &P) -> RolloutSnapshot {
    port.snapshot()
}

pub(crate) async fn start<P: RolloutSessionPort + ?Sized>(
    port: &P,
    request: RolloutRequest,
) -> Result<RolloutSnapshot, OperationError> {
    port.start(request).await
}

pub(crate) async fn stop<P: RolloutSessionPort + ?Sized>(port: &P) -> Result<(), OperationError> {
    port.stop().await
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RolloutPhase {
    #[default]
    Idle,
    Loading,
    WarmingUp,
    Running,
    Stopping,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RolloutMetrics {
    inference_count: u64,
    command_count: u64,
    missed_deadlines: u64,
    last_latency_ms: Option<f64>,
    last_model_ms: Option<f64>,
    mean_latency_ms: Option<f64>,
    max_latency_ms: Option<f64>,
}

impl RolloutMetrics {
    pub(crate) fn new(inference_count: u64, command_count: u64, missed_deadlines: u64) -> Self {
        Self {
            inference_count,
            command_count,
            missed_deadlines,
            ..Self::default()
        }
    }

    pub(crate) fn with_latest_timings(
        mut self,
        last_latency_ms: Option<f64>,
        last_model_ms: Option<f64>,
    ) -> Self {
        self.last_latency_ms = last_latency_ms;
        self.last_model_ms = last_model_ms;
        self
    }

    pub(crate) fn with_latency_summary(
        mut self,
        mean_latency_ms: Option<f64>,
        max_latency_ms: Option<f64>,
    ) -> Self {
        self.mean_latency_ms = mean_latency_ms;
        self.max_latency_ms = max_latency_ms;
        self
    }

    pub(crate) fn inference_count(&self) -> u64 {
        self.inference_count
    }

    pub(crate) fn command_count(&self) -> u64 {
        self.command_count
    }

    pub(crate) fn missed_deadlines(&self) -> u64 {
        self.missed_deadlines
    }

    pub(crate) fn last_latency_ms(&self) -> Option<f64> {
        self.last_latency_ms
    }

    pub(crate) fn last_model_ms(&self) -> Option<f64> {
        self.last_model_ms
    }

    pub(crate) fn mean_latency_ms(&self) -> Option<f64> {
        self.mean_latency_ms
    }

    pub(crate) fn max_latency_ms(&self) -> Option<f64> {
        self.max_latency_ms
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RolloutSnapshot {
    identity: RolloutIdentity,
    lifecycle: RolloutLifecycle,
    metrics: RolloutMetrics,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RolloutIdentity {
    session_id: Option<String>,
    run_id: Option<String>,
    checkpoint_step: Option<u64>,
    robot: Option<String>,
    duration_seconds: Option<u32>,
}

impl RolloutIdentity {
    pub(crate) fn new(
        session_id: Option<String>,
        run_id: Option<String>,
        checkpoint_step: Option<u64>,
    ) -> Self {
        Self {
            session_id,
            run_id,
            checkpoint_step,
            robot: None,
            duration_seconds: None,
        }
    }

    pub(crate) fn with_target(
        mut self,
        robot: Option<String>,
        duration_seconds: Option<u32>,
    ) -> Self {
        self.robot = robot;
        self.duration_seconds = duration_seconds;
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RolloutLifecycle {
    phase: RolloutPhase,
    started_ms: Option<i64>,
    finished_ms: Option<i64>,
    message: Option<String>,
    warning: Option<String>,
}

impl RolloutLifecycle {
    pub(crate) fn new(
        phase: RolloutPhase,
        started_ms: Option<i64>,
        finished_ms: Option<i64>,
    ) -> Self {
        Self {
            phase,
            started_ms,
            finished_ms,
            message: None,
            warning: None,
        }
    }

    pub(crate) fn with_messages(
        mut self,
        message: Option<String>,
        warning: Option<String>,
    ) -> Self {
        self.message = message;
        self.warning = warning;
        self
    }
}

impl RolloutSnapshot {
    pub(crate) fn new(
        identity: RolloutIdentity,
        lifecycle: RolloutLifecycle,
        metrics: RolloutMetrics,
    ) -> Self {
        Self {
            identity,
            lifecycle,
            metrics,
        }
    }
    pub(crate) fn phase(&self) -> RolloutPhase {
        self.lifecycle.phase
    }
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.identity.session_id.as_deref()
    }
    pub(crate) fn run_id(&self) -> Option<&str> {
        self.identity.run_id.as_deref()
    }
    pub(crate) fn checkpoint_step(&self) -> Option<u64> {
        self.identity.checkpoint_step
    }
    pub(crate) fn robot(&self) -> Option<&str> {
        self.identity.robot.as_deref()
    }
    pub(crate) fn duration_seconds(&self) -> Option<u32> {
        self.identity.duration_seconds
    }
    pub(crate) fn started_ms(&self) -> Option<i64> {
        self.lifecycle.started_ms
    }
    pub(crate) fn finished_ms(&self) -> Option<i64> {
        self.lifecycle.finished_ms
    }
    pub(crate) fn message(&self) -> Option<&str> {
        self.lifecycle.message.as_deref()
    }
    pub(crate) fn warning(&self) -> Option<&str> {
        self.lifecycle.warning.as_deref()
    }
    pub(crate) fn metrics(&self) -> &RolloutMetrics {
        &self.metrics
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ExecutionOutcome {
    Completed(CompletionReason),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompletionReason {
    StoppedBeforeModel,
    StoppedBeforeControl,
    StoppedSafely,
    DurationReached { duration_seconds: u32 },
    ControlReplaced(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ControlDecision {
    Continue,
    Finish(CompletionReason),
    Fail(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StopSignal {
    session: String,
    newly_requested: bool,
}

impl StopSignal {
    pub(crate) fn session(&self) -> &str {
        &self.session
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CleanupTicket {
    session: String,
}

pub(crate) struct RolloutAdmission {
    session: String,
    started_ms: i64,
    request: RolloutRequest,
}

impl RolloutAdmission {
    pub(crate) fn new(session: String, started_ms: i64, request: RolloutRequest) -> Self {
        Self {
            session,
            started_ms,
            request,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InferenceTiming {
    latency_ms: f64,
    model_ms: f64,
}

impl InferenceTiming {
    pub(crate) fn new(latency_ms: f64, model_ms: f64) -> Self {
        Self {
            latency_ms,
            model_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PolicyCadence {
    sample_hz: f64,
    action_steps: usize,
}

impl PolicyCadence {
    pub(crate) fn new(sample_hz: f64, action_steps: usize) -> Self {
        Self {
            sample_hz,
            action_steps,
        }
    }
}

pub(crate) struct ControlObservation<'a> {
    owner: &'a ControlOwner,
    hardware_error: Option<&'a str>,
}

impl<'a> ControlObservation<'a> {
    pub(crate) fn new(owner: &'a ControlOwner, hardware_error: Option<&'a str>) -> Self {
        Self {
            owner,
            hardware_error,
        }
    }
}

pub(crate) struct CleanupResult {
    finished_ms: i64,
    cleanup: Result<(), String>,
}

impl CleanupResult {
    pub(crate) fn new(finished_ms: i64, cleanup: Result<(), String>) -> Self {
        Self {
            finished_ms,
            cleanup,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionPhase {
    Loading,
    WarmingUp,
    AwaitingControl,
    Running,
    Stopping,
}

struct ActiveSession {
    session: String,
    request: RolloutRequest,
    phase: SessionPhase,
    stop_requested: bool,
    stop_stage: Option<StopStage>,
    cleanup_requested: bool,
    pending_outcome: Option<ExecutionOutcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StopStage {
    BeforeModel,
    BeforeControl,
    Running,
}

pub(crate) struct RolloutCoordinator {
    active: Option<ActiveSession>,
    snapshot: RolloutSnapshot,
    events: Arc<dyn RolloutEventPort>,
}

impl RolloutCoordinator {
    pub(crate) fn new(events: Arc<dyn RolloutEventPort>) -> Self {
        Self {
            active: None,
            snapshot: RolloutSnapshot::default(),
            events,
        }
    }

    pub(crate) fn admit(&mut self, admission: RolloutAdmission) -> Result<RolloutSnapshot, String> {
        let RolloutAdmission {
            session,
            started_ms,
            request,
        } = admission;
        validate_request(&session, &request)?;
        if let Some(active) = &self.active {
            return Err(format!(
                "policy session {:?} is already active",
                active.session
            ));
        }
        self.snapshot = RolloutSnapshot {
            identity: RolloutIdentity {
                session_id: Some(session.clone()),
                run_id: Some(request.run_id.clone()),
                checkpoint_step: Some(request.checkpoint_step),
                robot: Some(request.robot.clone()),
                duration_seconds: Some(request.duration_seconds),
            },
            lifecycle: RolloutLifecycle {
                phase: RolloutPhase::Loading,
                started_ms: Some(started_ms),
                message: Some("Loading checkpoint…".to_owned()),
                ..RolloutLifecycle::default()
            },
            metrics: RolloutMetrics::default(),
        };
        self.active = Some(ActiveSession {
            session,
            request,
            phase: SessionPhase::Loading,
            stop_requested: false,
            stop_stage: None,
            cleanup_requested: false,
            pending_outcome: None,
        });
        self.publish();
        Ok(self.snapshot.clone())
    }

    pub(crate) fn request_stop(&mut self) -> Result<StopSignal, String> {
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| "no policy rollout is active".to_owned())?;
        let newly_requested = !active.stop_requested;
        active.stop_stage.get_or_insert(match active.phase {
            SessionPhase::Loading => StopStage::BeforeModel,
            SessionPhase::WarmingUp | SessionPhase::AwaitingControl => StopStage::BeforeControl,
            SessionPhase::Running | SessionPhase::Stopping => StopStage::Running,
        });
        active.stop_requested = true;
        active.phase = SessionPhase::Stopping;
        self.snapshot.lifecycle.phase = RolloutPhase::Stopping;
        self.snapshot.lifecycle.message =
            Some("Stopping after the current control cycle…".to_owned());
        let session = active.session.clone();
        self.publish();
        Ok(StopSignal {
            session,
            newly_requested,
        })
    }

    pub(crate) fn stopped_outcome(&self, session: &str) -> Result<ExecutionOutcome, String> {
        let active = self.session(session)?;
        if !active.stop_requested {
            return Err(format!("policy session {session:?} was not asked to stop"));
        }
        let reason = match active.stop_stage {
            Some(StopStage::BeforeModel) => CompletionReason::StoppedBeforeModel,
            Some(StopStage::BeforeControl) => CompletionReason::StoppedBeforeControl,
            Some(StopStage::Running) => CompletionReason::StoppedSafely,
            None => {
                return Err(format!(
                    "policy session {session:?} has no stop classification"
                ));
            }
        };
        Ok(ExecutionOutcome::Completed(reason))
    }

    pub(crate) fn warming_up(&mut self, session: &str) -> Result<bool, String> {
        let active = self.session(session)?;
        if active.stop_requested {
            return Ok(false);
        }
        if active.phase != SessionPhase::Loading {
            return Err(format!(
                "policy session {session:?} cannot warm up from {:?}",
                active.phase
            ));
        }
        self.session_mut(session)?.phase = SessionPhase::WarmingUp;
        self.snapshot.lifecycle.phase = RolloutPhase::WarmingUp;
        self.snapshot.lifecycle.message =
            Some("Preparing policy and waiting for cameras…".to_owned());
        self.publish();
        Ok(true)
    }

    pub(crate) fn record_warmup(
        &mut self,
        session: &str,
        timing: InferenceTiming,
    ) -> Result<(), String> {
        let active = self.session(session)?;
        if active.stop_requested {
            return Ok(());
        }
        if active.phase != SessionPhase::WarmingUp {
            return Err(format!(
                "policy session {session:?} cannot publish warmup metrics outside warmup"
            ));
        }
        self.snapshot.metrics.last_latency_ms = finite_metric(timing.latency_ms);
        self.snapshot.metrics.last_model_ms = finite_metric(timing.model_ms);
        self.publish();
        Ok(())
    }

    pub(crate) fn awaiting_control(&mut self, session: &str) -> Result<bool, String> {
        let active = self.session(session)?;
        if active.stop_requested {
            return Ok(false);
        }
        if active.phase != SessionPhase::WarmingUp {
            return Err(format!(
                "policy session {session:?} cannot request control before warmup"
            ));
        }
        self.session_mut(session)?.phase = SessionPhase::AwaitingControl;
        self.snapshot.lifecycle.phase = RolloutPhase::WarmingUp;
        self.snapshot.lifecycle.message = Some("Waiting for exclusive policy control…".to_owned());
        self.publish();
        Ok(true)
    }

    pub(crate) fn running(
        &mut self,
        session: &str,
        cadence: PolicyCadence,
    ) -> Result<bool, String> {
        let active = self.session(session)?;
        if active.stop_requested {
            return Ok(false);
        }
        if active.phase != SessionPhase::AwaitingControl {
            return Err(format!(
                "policy session {session:?} cannot run before control admission"
            ));
        }
        let duration = active.request.duration_seconds;
        let PolicyCadence {
            sample_hz,
            action_steps,
        } = cadence;
        self.session_mut(session)?.phase = SessionPhase::Running;
        self.snapshot.lifecycle.phase = RolloutPhase::Running;
        self.snapshot.lifecycle.message = Some(format!(
            "Controlling the follower at {sample_hz:.1} Hz · {action_steps} actions per observation · {} maximum.",
            format_duration(duration)
        ));
        self.publish();
        Ok(true)
    }

    pub(crate) fn observe_control(
        &self,
        session: &str,
        observation: ControlObservation<'_>,
    ) -> Result<ControlDecision, String> {
        let active = self.session(session)?;
        match observation.owner.assess(
            observation.hardware_error,
            ExpectedPolicyOwner::new(&active.request.robot, session),
        ) {
            Ok(PolicyOwnership::Retained) => Ok(ControlDecision::Continue),
            Ok(PolicyOwnership::Replaced(message)) => Ok(ControlDecision::Finish(
                CompletionReason::ControlReplaced(message),
            )),
            Err(error) => Ok(ControlDecision::Fail(error.to_string())),
        }
    }

    pub(crate) fn record_inference(
        &mut self,
        session: &str,
        timing: InferenceTiming,
    ) -> Result<(), String> {
        let active = self.session(session)?;
        if active.phase != SessionPhase::Running
            && !(active.phase == SessionPhase::Stopping
                && active.stop_stage == Some(StopStage::Running))
        {
            return Err(format!(
                "policy session {session:?} cannot publish inference metrics while not running"
            ));
        }
        let InferenceTiming {
            latency_ms,
            model_ms,
        } = timing;
        if !latency_ms.is_finite() || latency_ms < 0.0 || !model_ms.is_finite() || model_ms < 0.0 {
            return Err("rollout latency metrics must be finite and non-negative".to_owned());
        }
        let metrics = &mut self.snapshot.metrics;
        let previous = metrics.mean_latency_ms.unwrap_or(0.0) * metrics.inference_count as f64;
        metrics.inference_count += 1;
        metrics.last_latency_ms = Some(latency_ms);
        metrics.last_model_ms = Some(model_ms);
        metrics.mean_latency_ms = Some((previous + latency_ms) / metrics.inference_count as f64);
        metrics.max_latency_ms = Some(
            metrics
                .max_latency_ms
                .map_or(latency_ms, |maximum| maximum.max(latency_ms)),
        );
        self.publish();
        Ok(())
    }

    pub(crate) fn record_command(
        &mut self,
        session: &str,
        missed_deadline: bool,
    ) -> Result<(), String> {
        let active = self.session(session)?;
        if active.phase != SessionPhase::Running
            && !(active.phase == SessionPhase::Stopping
                && active.stop_stage == Some(StopStage::Running))
        {
            return Err(format!(
                "policy session {session:?} cannot publish commands while not running"
            ));
        }
        self.snapshot.metrics.command_count += 1;
        self.snapshot.metrics.missed_deadlines += u64::from(missed_deadline);
        self.publish();
        Ok(())
    }

    pub(crate) fn record_action_clamp_warning(
        &mut self,
        session: &str,
        warning: String,
    ) -> Result<bool, String> {
        let active = self.session(session)?;
        if active.phase != SessionPhase::Running
            && !(active.phase == SessionPhase::Stopping
                && active.stop_stage == Some(StopStage::Running))
        {
            return Err(format!(
                "policy session {session:?} cannot publish action warnings while not running"
            ));
        }
        if self.snapshot.lifecycle.warning.as_deref() == Some(warning.as_str()) {
            return Ok(false);
        }
        self.snapshot.lifecycle.warning = Some(warning);
        self.publish();
        Ok(true)
    }

    pub(crate) fn begin_cleanup(
        &mut self,
        session: &str,
        outcome: ExecutionOutcome,
    ) -> Result<CleanupTicket, String> {
        let active = self.session_mut(session)?;
        if active.cleanup_requested {
            return Err(format!(
                "cleanup for policy session {session:?} was already requested"
            ));
        }
        active.cleanup_requested = true;
        active.pending_outcome = Some(outcome);
        active.phase = SessionPhase::Stopping;
        self.snapshot.lifecycle.phase = RolloutPhase::Stopping;
        self.snapshot.lifecycle.message =
            Some("Returning hardware control to a safe hold…".to_owned());
        self.publish();
        Ok(CleanupTicket {
            session: session.to_owned(),
        })
    }

    pub(crate) fn finish_cleanup(
        &mut self,
        ticket: CleanupTicket,
        result: CleanupResult,
    ) -> Result<RolloutSnapshot, String> {
        let active = self.session_mut(&ticket.session)?;
        if !active.cleanup_requested {
            return Err(format!(
                "cleanup for policy session {:?} was never requested",
                ticket.session
            ));
        }
        let outcome = active.pending_outcome.take().ok_or_else(|| {
            format!(
                "cleanup ticket for {:?} was already consumed",
                ticket.session
            )
        })?;
        let (phase, message) = terminal_state(outcome, result.cleanup);
        self.snapshot.lifecycle.phase = phase;
        self.snapshot.lifecycle.finished_ms = Some(result.finished_ms);
        self.snapshot.lifecycle.message = Some(message);
        self.publish();
        self.active = None;
        Ok(self.snapshot.clone())
    }

    fn session(&self, session: &str) -> Result<&ActiveSession, String> {
        self.active
            .as_ref()
            .filter(|active| active.session == session)
            .ok_or_else(|| format!("policy session {session:?} is not active"))
    }

    fn session_mut(&mut self, session: &str) -> Result<&mut ActiveSession, String> {
        self.active
            .as_mut()
            .filter(|active| active.session == session)
            .ok_or_else(|| format!("policy session {session:?} is not active"))
    }

    fn publish(&self) {
        self.events.publish(&self.snapshot);
    }
}

fn validate_request(session: &str, request: &RolloutRequest) -> Result<(), String> {
    if session.trim().is_empty() || session.trim() != session {
        return Err("policy session id must be non-empty and trimmed".to_owned());
    }
    if request.run_id.trim().is_empty()
        || request.run_id.trim() != request.run_id
        || request.robot.trim().is_empty()
        || request.robot.trim() != request.robot
    {
        return Err("rollout run and robot ids must be non-empty and trimmed".to_owned());
    }
    if request.checkpoint_step == 0 {
        return Err("rollout checkpoint step must be positive".to_owned());
    }
    if RolloutDuration::new(request.duration_seconds).is_none() {
        return Err("rollout duration is outside the approved safety window".to_owned());
    }
    Ok(())
}

fn finite_metric(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn terminal_state(
    outcome: ExecutionOutcome,
    cleanup: Result<(), String>,
) -> (RolloutPhase, String) {
    let execution = match outcome {
        ExecutionOutcome::Completed(reason) => Ok(completion_message(reason)),
        ExecutionOutcome::Failed(error) => Err(error),
    };
    match (execution, cleanup) {
        (Ok(message), Ok(())) => (RolloutPhase::Idle, message),
        (Err(error), Ok(())) => (RolloutPhase::Failed, error),
        (Ok(_), Err(cleanup)) => (
            RolloutPhase::Failed,
            format!("policy execution ended, but safe control cleanup failed: {cleanup}"),
        ),
        (Err(error), Err(cleanup)) => (
            RolloutPhase::Failed,
            format!("{error}; safe control cleanup also failed: {cleanup}"),
        ),
    }
}

fn completion_message(reason: CompletionReason) -> String {
    match reason {
        CompletionReason::StoppedBeforeModel => "Stopped before model loading.".to_owned(),
        CompletionReason::StoppedBeforeControl => "Stopped before the arm was enabled.".to_owned(),
        CompletionReason::StoppedSafely => {
            "Policy stopped safely; the follower is holding its current position.".to_owned()
        }
        CompletionReason::DurationReached { duration_seconds } => format!(
            "Policy completed its configured {} duration; the follower is holding its current position.",
            format_duration(duration_seconds)
        ),
        CompletionReason::ControlReplaced(message) => message,
    }
}

fn format_duration(seconds: u32) -> String {
    match seconds {
        1 => "1 second".to_owned(),
        value if value % 60 == 0 => {
            let minutes = value / 60;
            format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
        }
        value => format!("{value} seconds"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    use super::*;

    #[derive(Default)]
    struct Events(Mutex<Vec<RolloutSnapshot>>);

    impl RolloutEventPort for Events {
        fn publish(&self, snapshot: &RolloutSnapshot) {
            self.0.lock().unwrap().push(snapshot.clone());
        }
    }

    fn request() -> RolloutRequest {
        RolloutRequest::new("run-1".to_owned(), 10, "follower".to_owned()).with_duration(30)
    }

    fn coordinator() -> (Arc<Events>, RolloutCoordinator) {
        let events = Arc::new(Events::default());
        let coordinator = RolloutCoordinator::new(events.clone());
        (events, coordinator)
    }

    #[test]
    fn concurrent_starts_admit_exactly_one_session() {
        let events = Arc::new(Events::default());
        let coordinator = Arc::new(Mutex::new(RolloutCoordinator::new(events)));
        let barrier = Arc::new(Barrier::new(3));
        let attempts = ["session-a", "session-b"].map(|session| {
            let coordinator = coordinator.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                coordinator.lock().unwrap().admit(RolloutAdmission::new(
                    session.to_owned(),
                    100,
                    request(),
                ))
            })
        });
        barrier.wait();
        let results = attempts.map(|attempt| attempt.join().unwrap());

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert!(coordinator.lock().unwrap().active.is_some());
    }

    #[test]
    fn stop_before_control_admission_never_becomes_running() {
        let (_events, mut coordinator) = coordinator();
        coordinator
            .admit(RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                request(),
            ))
            .unwrap();
        coordinator.warming_up("session-1").unwrap();
        coordinator.awaiting_control("session-1").unwrap();
        let signal = coordinator.request_stop().unwrap();

        assert!(signal.newly_requested);
        assert!(
            !coordinator
                .running("session-1", PolicyCadence::new(60.0, 10))
                .unwrap()
        );
        assert_eq!(
            coordinator.stopped_outcome("session-1").unwrap(),
            ExecutionOutcome::Completed(CompletionReason::StoppedBeforeControl)
        );
    }

    #[test]
    fn stop_while_loading_is_classified_before_model_execution() {
        let (_events, mut coordinator) = coordinator();
        coordinator
            .admit(RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                request(),
            ))
            .unwrap();

        coordinator.request_stop().unwrap();

        assert_eq!(
            coordinator.stopped_outcome("session-1").unwrap(),
            ExecutionOutcome::Completed(CompletionReason::StoppedBeforeModel)
        );
        assert!(!coordinator.warming_up("session-1").unwrap());
    }

    #[test]
    fn snapshot_tracks_semantic_lifecycle_and_metrics() {
        let (events, mut coordinator) = coordinator();
        coordinator
            .admit(RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                request(),
            ))
            .unwrap();
        coordinator.warming_up("session-1").unwrap();
        coordinator
            .record_warmup("session-1", InferenceTiming::new(12.0, 8.0))
            .unwrap();
        coordinator.awaiting_control("session-1").unwrap();
        coordinator
            .running("session-1", PolicyCadence::new(60.0, 10))
            .unwrap();
        coordinator
            .record_inference("session-1", InferenceTiming::new(20.0, 15.0))
            .unwrap();
        coordinator
            .record_inference("session-1", InferenceTiming::new(10.0, 7.0))
            .unwrap();
        coordinator.record_command("session-1", true).unwrap();

        assert_eq!(coordinator.snapshot.phase(), RolloutPhase::Running);
        assert_eq!(coordinator.snapshot.metrics.inference_count, 2);
        assert_eq!(coordinator.snapshot.metrics.command_count, 1);
        assert_eq!(coordinator.snapshot.metrics.missed_deadlines, 1);
        assert_eq!(coordinator.snapshot.metrics.mean_latency_ms, Some(15.0));
        assert_eq!(coordinator.snapshot.metrics.max_latency_ms, Some(20.0));
        assert!(
            events
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|snapshot| snapshot.phase() == RolloutPhase::WarmingUp)
        );
    }

    #[test]
    fn action_clamp_warning_is_visible_without_stopping_the_rollout() {
        let (events, mut coordinator) = coordinator();
        coordinator
            .admit(RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                request(),
            ))
            .unwrap();
        coordinator.warming_up("session-1").unwrap();
        coordinator.awaiting_control("session-1").unwrap();
        coordinator
            .running("session-1", PolicyCadence::new(60.0, 10))
            .unwrap();

        let warning =
            "Policy output for joint \"wrist_flex\" exceeded normalized [-1, 1]; the command was clamped to the limit."
                .to_owned();
        assert!(
            coordinator
                .record_action_clamp_warning("session-1", warning.clone())
                .unwrap()
        );
        let published = events.0.lock().unwrap().len();
        assert!(
            !coordinator
                .record_action_clamp_warning("session-1", warning.clone())
                .unwrap()
        );

        assert_eq!(coordinator.snapshot.phase(), RolloutPhase::Running);
        assert_eq!(coordinator.snapshot.warning(), Some(warning.as_str()));
        assert_eq!(events.0.lock().unwrap().len(), published);
    }

    #[test]
    fn healthy_external_control_loss_is_completion_but_hardware_loss_is_failure() {
        let (_events, mut coordinator) = coordinator();
        coordinator
            .admit(RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                request(),
            ))
            .unwrap();
        coordinator.warming_up("session-1").unwrap();
        coordinator.awaiting_control("session-1").unwrap();
        coordinator
            .running("session-1", PolicyCadence::new(60.0, 10))
            .unwrap();

        assert!(matches!(
            coordinator
                .observe_control(
                    "session-1",
                    ControlObservation::new(
                        &ControlOwner::Other("Manual on follower".to_owned()),
                        None,
                    ),
                )
                .unwrap(),
            ControlDecision::Finish(CompletionReason::ControlReplaced(message))
                if message.contains("Manual on follower")
        ));
        assert!(matches!(
            coordinator
                .observe_control(
                    "session-1",
                    ControlObservation::new(
                        &ControlOwner::Other("Idle".to_owned()),
                        Some("motor bus stopped responding"),
                    ),
                )
                .unwrap(),
            ControlDecision::Fail(message)
                if message.contains("motor bus stopped responding")
        ));
    }

    #[test]
    fn executor_failure_is_terminal_only_after_cleanup() {
        let (events, mut coordinator) = coordinator();
        coordinator
            .admit(RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                request(),
            ))
            .unwrap();
        let ticket = coordinator
            .begin_cleanup(
                "session-1",
                ExecutionOutcome::Failed("native executor failed".to_owned()),
            )
            .unwrap();

        assert_eq!(coordinator.snapshot.phase(), RolloutPhase::Stopping);
        assert_eq!(
            coordinator
                .active
                .as_ref()
                .map(|active| active.session.as_str()),
            Some("session-1")
        );
        let terminal = coordinator
            .finish_cleanup(ticket, CleanupResult::new(200, Ok(())))
            .unwrap();
        assert_eq!(terminal.phase(), RolloutPhase::Failed);
        assert_eq!(terminal.finished_ms(), Some(200));
        assert!(
            terminal
                .message()
                .unwrap()
                .contains("native executor failed")
        );
        assert!(coordinator.active.is_none());
        assert_eq!(
            events.0.lock().unwrap().last().unwrap().phase(),
            RolloutPhase::Failed
        );
    }

    #[test]
    fn cleanup_is_issued_and_consumed_exactly_once() {
        let (_events, mut coordinator) = coordinator();
        coordinator
            .admit(RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                request(),
            ))
            .unwrap();
        let ticket = coordinator
            .begin_cleanup(
                "session-1",
                ExecutionOutcome::Completed(CompletionReason::StoppedBeforeModel),
            )
            .unwrap();

        assert!(
            coordinator
                .begin_cleanup(
                    "session-1",
                    ExecutionOutcome::Completed(CompletionReason::StoppedBeforeModel),
                )
                .unwrap_err()
                .contains("already requested")
        );
        coordinator
            .finish_cleanup(ticket, CleanupResult::new(200, Ok(())))
            .unwrap();
        assert!(
            coordinator
                .begin_cleanup(
                    "session-1",
                    ExecutionOutcome::Completed(CompletionReason::StoppedBeforeModel),
                )
                .unwrap_err()
                .contains("not active")
        );
    }

    #[test]
    fn cleanup_failure_cannot_report_a_successful_rollout() {
        let (_events, mut coordinator) = coordinator();
        coordinator
            .admit(RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                request(),
            ))
            .unwrap();
        let ticket = coordinator
            .begin_cleanup(
                "session-1",
                ExecutionOutcome::Completed(CompletionReason::DurationReached {
                    duration_seconds: 30,
                }),
            )
            .unwrap();
        let terminal = coordinator
            .finish_cleanup(
                ticket,
                CleanupResult::new(200, Err("torque hold failed".to_owned())),
            )
            .unwrap();

        assert_eq!(terminal.phase(), RolloutPhase::Failed);
        assert!(terminal.message().unwrap().contains("torque hold failed"));
    }
}
