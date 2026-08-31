//! Pure lifecycle coordination for one remotely computed policy rollout.
//!
//! Runtime tasks, model formats, cameras, clocks, and motor actors remain
//! adapters. This module owns which session may exist, what its semantic state
//! is, how a stop is classified, and the mandatory cleanup barrier before a
//! session becomes terminal.

use std::sync::Arc;

use spiral_robotics_api::rollout::{
    MAX_ROLLOUT_DURATION_SECONDS, RolloutConfig, RolloutMetrics, RolloutPhase, RolloutSnapshot,
};

use super::policy::{ControlOwner, ExpectedPolicyOwner, PolicyOwnership};

pub(crate) const MIN_ROLLOUT_DURATION_SECONDS: u32 = 1;

/// Admission marker for a duration inside the backend-owned safety window.
pub(crate) struct RolloutDuration;

impl RolloutDuration {
    pub(crate) const fn new(seconds: u32) -> Option<Self> {
        if seconds >= MIN_ROLLOUT_DURATION_SECONDS && seconds <= MAX_ROLLOUT_DURATION_SECONDS {
            Some(Self)
        } else {
            None
        }
    }
}

/// Publication seam between lifecycle decisions and state/persistence watchers.
pub(crate) trait RolloutEventPort: Send + Sync {
    fn publish(&self, snapshot: &RolloutSnapshot);
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

impl CompletionReason {
    fn message(self) -> String {
        match self {
            Self::StoppedBeforeModel => "Stopped before model loading.".to_owned(),
            Self::StoppedBeforeControl => "Stopped before the arm was enabled.".to_owned(),
            Self::StoppedSafely => {
                "Policy stopped safely; the follower is holding its current position.".to_owned()
            }
            Self::DurationReached { duration_seconds } => format!(
                "Policy completed its configured {} duration; the follower is holding its current position.",
                duration_seconds.rollout_duration_label()
            ),
            Self::ControlReplaced(message) => message,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ControlDecision {
    Continue,
    Finish(CompletionReason),
    Fail(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CleanupTicket {
    session: String,
}

pub(crate) struct RolloutAdmission {
    session: String,
    started_ms: i64,
    config: RolloutConfig,
}

impl RolloutAdmission {
    pub(crate) fn new(session: String, started_ms: i64, config: RolloutConfig) -> Self {
        Self {
            session,
            started_ms,
            config,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.session.trim().is_empty() || self.session.trim() != self.session {
            return Err("policy session id must be non-empty and trimmed".to_owned());
        }
        if self.config.run_id().trim().is_empty()
            || self.config.run_id().trim() != self.config.run_id()
            || self.config.robot().trim().is_empty()
            || self.config.robot().trim() != self.config.robot()
        {
            return Err("rollout run and robot ids must be non-empty and trimmed".to_owned());
        }
        if self.config.checkpoint_step() == 0 {
            return Err("rollout checkpoint step must be positive".to_owned());
        }
        if RolloutDuration::new(self.config.duration_seconds()).is_none() {
            return Err("rollout duration is outside the approved safety window".to_owned());
        }
        Ok(())
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

    fn finite_latency_ms(&self) -> Option<f64> {
        (self.latency_ms.is_finite() && self.latency_ms >= 0.0).then_some(self.latency_ms)
    }

    fn finite_model_ms(&self) -> Option<f64> {
        (self.model_ms.is_finite() && self.model_ms >= 0.0).then_some(self.model_ms)
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
    config: RolloutConfig,
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
        admission.validate()?;
        let RolloutAdmission {
            session,
            started_ms,
            config,
        } = admission;
        if let Some(active) = &self.active {
            return Err(format!(
                "policy session {:?} is already active",
                active.session
            ));
        }
        self.snapshot =
            RolloutSnapshot::new(Some(session.clone()), Some(config.run_id().to_owned()))
                .with_robot(Some(config.robot().to_owned()))
                .with_checkpoint(Some(config.checkpoint_step()))
                .with_duration(Some(config.duration_seconds()))
                .with_lifecycle(RolloutPhase::Loading, Some(started_ms))
                .with_completion(None, Some("Loading checkpoint…".to_owned()));
        self.active = Some(ActiveSession {
            session,
            config,
            phase: SessionPhase::Loading,
            stop_requested: false,
            stop_stage: None,
            cleanup_requested: false,
            pending_outcome: None,
        });
        self.publish();
        Ok(self.snapshot.clone())
    }

    pub(crate) fn request_stop(&mut self) -> Result<(), String> {
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| "no policy rollout is active".to_owned())?;
        active.stop_stage.get_or_insert(match active.phase {
            SessionPhase::Loading => StopStage::BeforeModel,
            SessionPhase::WarmingUp | SessionPhase::AwaitingControl => StopStage::BeforeControl,
            SessionPhase::Running | SessionPhase::Stopping => StopStage::Running,
        });
        active.stop_requested = true;
        active.phase = SessionPhase::Stopping;
        self.set_lifecycle(
            RolloutPhase::Stopping,
            "Stopping after the current control cycle…",
        );
        self.publish();
        Ok(())
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
        self.set_lifecycle(
            RolloutPhase::WarmingUp,
            "Preparing policy and waiting for cameras…",
        );
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
        let metrics = self.snapshot.metrics();
        self.snapshot = self.snapshot.clone().with_metrics(
            RolloutMetrics::new(
                metrics.inference_count(),
                metrics.command_count(),
                metrics.missed_deadlines(),
            )
            .with_latest_timings(timing.finite_latency_ms(), timing.finite_model_ms())
            .with_latency_summary(metrics.mean_latency_ms(), metrics.max_latency_ms()),
        );
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
        self.set_lifecycle(
            RolloutPhase::WarmingUp,
            "Waiting for exclusive policy control…",
        );
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
        let duration = active.config.duration_seconds();
        let PolicyCadence {
            sample_hz,
            action_steps,
        } = cadence;
        self.session_mut(session)?.phase = SessionPhase::Running;
        self.set_lifecycle(
            RolloutPhase::Running,
            format!(
            "Controlling the follower at {sample_hz:.1} Hz · {action_steps} actions per observation · {} maximum.",
            duration.rollout_duration_label()
            ),
        );
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
            ExpectedPolicyOwner::new(active.config.robot(), session),
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
        let metrics = self.snapshot.metrics();
        let previous = metrics.mean_latency_ms().unwrap_or(0.0) * metrics.inference_count() as f64;
        let inference_count = metrics.inference_count() + 1;
        let metrics = RolloutMetrics::new(
            inference_count,
            metrics.command_count(),
            metrics.missed_deadlines(),
        )
        .with_latest_timings(Some(latency_ms), Some(model_ms))
        .with_latency_summary(
            Some((previous + latency_ms) / inference_count as f64),
            Some(
                metrics
                    .max_latency_ms()
                    .map_or(latency_ms, |maximum| maximum.max(latency_ms)),
            ),
        );
        self.snapshot = self.snapshot.clone().with_metrics(metrics);
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
        let metrics = self.snapshot.metrics();
        self.snapshot = self.snapshot.clone().with_metrics(
            RolloutMetrics::new(
                metrics.inference_count(),
                metrics.command_count() + 1,
                metrics.missed_deadlines() + u64::from(missed_deadline),
            )
            .with_latest_timings(metrics.last_latency_ms(), metrics.last_model_ms())
            .with_latency_summary(metrics.mean_latency_ms(), metrics.max_latency_ms()),
        );
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
        if self.snapshot.warning() == Some(warning.as_str()) {
            return Ok(false);
        }
        self.snapshot = self.snapshot.clone().with_warning(Some(warning));
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
        self.set_lifecycle(
            RolloutPhase::Stopping,
            "Returning hardware control to a safe hold…",
        );
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
        let (phase, message) = outcome.terminal_state(result.cleanup);
        self.snapshot = self
            .snapshot
            .clone()
            .with_lifecycle(phase, self.snapshot.started_ms())
            .with_completion(Some(result.finished_ms), Some(message));
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

    fn set_lifecycle(&mut self, phase: RolloutPhase, message: impl Into<String>) {
        self.snapshot = self
            .snapshot
            .clone()
            .with_lifecycle(phase, self.snapshot.started_ms())
            .with_completion(self.snapshot.finished_ms(), Some(message.into()));
    }

    fn publish(&self) {
        self.events.publish(&self.snapshot);
    }
}

impl ExecutionOutcome {
    fn terminal_state(self, cleanup: Result<(), String>) -> (RolloutPhase, String) {
        let execution = match self {
            Self::Completed(reason) => Ok(reason.message()),
            Self::Failed(error) => Err(error),
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
}

trait RolloutDurationLabel {
    fn rollout_duration_label(self) -> String;
}

impl RolloutDurationLabel for u32 {
    fn rollout_duration_label(self) -> String {
        match self {
            1 => "1 second".to_owned(),
            value if value % 60 == 0 => {
                let minutes = value / 60;
                format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
            }
            value => format!("{value} seconds"),
        }
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

    fn config() -> RolloutConfig {
        RolloutConfig::new("run-1", 10, "follower").with_duration(30)
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
                    config(),
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
            .admit(RolloutAdmission::new("session-1".to_owned(), 100, config()))
            .unwrap();
        coordinator.warming_up("session-1").unwrap();
        coordinator.awaiting_control("session-1").unwrap();
        coordinator.request_stop().unwrap();
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
            .admit(RolloutAdmission::new("session-1".to_owned(), 100, config()))
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
            .admit(RolloutAdmission::new("session-1".to_owned(), 100, config()))
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
        assert_eq!(coordinator.snapshot.metrics().inference_count(), 2);
        assert_eq!(coordinator.snapshot.metrics().command_count(), 1);
        assert_eq!(coordinator.snapshot.metrics().missed_deadlines(), 1);
        assert_eq!(coordinator.snapshot.metrics().mean_latency_ms(), Some(15.0));
        assert_eq!(coordinator.snapshot.metrics().max_latency_ms(), Some(20.0));
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
            .admit(RolloutAdmission::new("session-1".to_owned(), 100, config()))
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
            .admit(RolloutAdmission::new("session-1".to_owned(), 100, config()))
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
            .admit(RolloutAdmission::new("session-1".to_owned(), 100, config()))
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
            .admit(RolloutAdmission::new("session-1".to_owned(), 100, config()))
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
            .admit(RolloutAdmission::new("session-1".to_owned(), 100, config()))
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
