//! What one remotely computed policy rollout reports while it runs: its
//! admission check, its live snapshot, how a stop or a control change ends it,
//! and its terminal state after the mandatory hardware cleanup.
//!
//! The rollout task owns the snapshot and publishes every change through one
//! `watch`. A stop request is the only other writer: once stop is asked for,
//! the task refuses every further phase change, so the stop is never
//! overwritten. Where the task was when it saw the stop is what the stop means.

use spiral_robotics_common::rollout::{
    MAX_ROLLOUT_DURATION_SECONDS, RolloutConfig, RolloutMetrics, RolloutPhase, RolloutSnapshot,
};
use tokio::sync::watch;

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

impl ExecutionOutcome {
    /// How the rollout ends once control is no longer its own; `None` while
    /// it still is. A healthy handover completes it, a hardware fault fails it.
    pub(super) fn after_control_change(
        owner: &ControlOwner,
        hardware_error: Option<&str>,
        expected: ExpectedPolicyOwner<'_>,
    ) -> Option<Self> {
        match owner.assess(hardware_error, expected) {
            Ok(PolicyOwnership::Retained) => None,
            Ok(PolicyOwnership::Replaced(message)) => {
                Some(Self::Completed(CompletionReason::ControlReplaced(message)))
            }
            Err(error) => Some(Self::Failed(error.to_string())),
        }
    }

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

pub(crate) struct RolloutAdmission {
    session: String,
    started_ms: i64,
    config: RolloutConfig,
}

impl RolloutAdmission {
    pub(super) fn new(session: String, started_ms: i64, config: RolloutConfig) -> Self {
        Self {
            session,
            started_ms,
            config,
        }
    }

    /// The snapshot an admitted rollout starts from.
    pub(super) fn into_snapshot(self) -> Result<RolloutSnapshot, String> {
        if self.session.trim().is_empty() || self.session.trim() != self.session {
            return Err("policy session id must be non-empty and trimmed".to_owned());
        }
        let config = &self.config;
        if config.run_id().trim().is_empty()
            || config.run_id().trim() != config.run_id()
            || config.robot().trim().is_empty()
            || config.robot().trim() != config.robot()
        {
            return Err("rollout run and robot ids must be non-empty and trimmed".to_owned());
        }
        if config.checkpoint_step() == 0 {
            return Err("rollout checkpoint step must be positive".to_owned());
        }
        if RolloutDuration::new(config.duration_seconds()).is_none() {
            return Err("rollout duration is outside the approved safety window".to_owned());
        }
        Ok(
            RolloutSnapshot::new(Some(self.session), Some(config.run_id().to_owned()))
                .with_robot(Some(config.robot().to_owned()))
                .with_checkpoint(Some(config.checkpoint_step()))
                .with_duration(Some(config.duration_seconds()))
                .with_lifecycle(RolloutPhase::Loading, Some(self.started_ms))
                .with_completion(None, Some("Loading checkpoint…".to_owned())),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct InferenceTiming {
    latency_ms: f64,
    model_ms: f64,
}

impl InferenceTiming {
    pub(super) fn new(latency_ms: f64, model_ms: f64) -> Self {
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
    pub(super) fn new(sample_hz: f64, action_steps: usize) -> Self {
        Self {
            sample_hz,
            action_steps,
        }
    }
}

pub(crate) struct CleanupResult {
    finished_ms: i64,
    cleanup: Result<(), String>,
}

impl CleanupResult {
    pub(super) fn new(finished_ms: i64, cleanup: Result<(), String>) -> Self {
        Self {
            finished_ms,
            cleanup,
        }
    }
}

/// A snapshot entering a phase keeps its timing and says what it is doing.
pub(super) trait RolloutPhaseChange {
    fn enter(&mut self, phase: RolloutPhase, message: String);
}

impl RolloutPhaseChange for RolloutSnapshot {
    fn enter(&mut self, phase: RolloutPhase, message: String) {
        let (started_ms, finished_ms) = (self.started_ms(), self.finished_ms());
        *self = std::mem::take(self)
            .with_lifecycle(phase, started_ms)
            .with_completion(finished_ms, Some(message));
    }
}

/// The running rollout's view of its own published snapshot and stop signal.
pub(super) struct RolloutProgress<'a> {
    published: &'a watch::Sender<RolloutSnapshot>,
    stop: watch::Receiver<bool>,
}

impl<'a> RolloutProgress<'a> {
    pub(super) fn new(
        published: &'a watch::Sender<RolloutSnapshot>,
        stop: watch::Receiver<bool>,
    ) -> Self {
        Self { published, stop }
    }

    pub(super) fn stop_requested(&self) -> bool {
        *self.stop.borrow()
    }

    /// Resolves once stop is asked for, or once nobody can ask any more.
    pub(super) async fn stop_asked(&mut self) {
        let _ = self.stop.wait_for(|stop| *stop).await;
    }

    pub(super) fn stop_signal(&mut self) -> &mut watch::Receiver<bool> {
        &mut self.stop
    }

    /// `false` once stop was asked for: the rollout must not warm up.
    pub(super) fn warming_up(&self) -> bool {
        self.advance(
            RolloutPhase::WarmingUp,
            "Preparing policy and waiting for cameras…".to_owned(),
        )
    }

    /// `false` once stop was asked for: the rollout must not request control.
    pub(super) fn awaiting_control(&self) -> bool {
        self.advance(
            RolloutPhase::WarmingUp,
            "Waiting for exclusive policy control…".to_owned(),
        )
    }

    /// `false` once stop was asked for: the rollout must not drive the arm.
    pub(super) fn running(&self, cadence: PolicyCadence, duration_seconds: u32) -> bool {
        let PolicyCadence {
            sample_hz,
            action_steps,
        } = cadence;
        self.advance(
            RolloutPhase::Running,
            format!(
                "Controlling the follower at {sample_hz:.1} Hz · {action_steps} actions per observation · {} maximum.",
                duration_seconds.rollout_duration_label()
            ),
        )
    }

    fn advance(&self, phase: RolloutPhase, message: String) -> bool {
        self.published.send_if_modified(|snapshot| {
            if *self.stop.borrow() {
                return false;
            }
            snapshot.enter(phase, message);
            true
        })
    }

    pub(super) fn record_warmup(&self, timing: InferenceTiming) {
        self.published.send_modify(|snapshot| {
            let metrics = snapshot.metrics().clone();
            *snapshot = std::mem::take(snapshot).with_metrics(
                RolloutMetrics::new(
                    metrics.inference_count(),
                    metrics.command_count(),
                    metrics.missed_deadlines(),
                )
                .with_latest_timings(timing.finite_latency_ms(), timing.finite_model_ms())
                .with_latency_summary(metrics.mean_latency_ms(), metrics.max_latency_ms()),
            );
        });
    }

    pub(super) fn record_inference(&self, timing: InferenceTiming) -> Result<(), String> {
        let (Some(latency_ms), Some(model_ms)) =
            (timing.finite_latency_ms(), timing.finite_model_ms())
        else {
            return Err("rollout latency metrics must be finite and non-negative".to_owned());
        };
        self.published.send_modify(|snapshot| {
            let metrics = snapshot.metrics().clone();
            let previous =
                metrics.mean_latency_ms().unwrap_or(0.0) * metrics.inference_count() as f64;
            let inference_count = metrics.inference_count() + 1;
            *snapshot = std::mem::take(snapshot).with_metrics(
                RolloutMetrics::new(
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
                ),
            );
        });
        Ok(())
    }

    pub(super) fn record_command(&self, missed_deadline: bool) {
        self.published.send_modify(|snapshot| {
            let metrics = snapshot.metrics().clone();
            *snapshot = std::mem::take(snapshot).with_metrics(
                RolloutMetrics::new(
                    metrics.inference_count(),
                    metrics.command_count() + 1,
                    metrics.missed_deadlines() + u64::from(missed_deadline),
                )
                .with_latest_timings(metrics.last_latency_ms(), metrics.last_model_ms())
                .with_latency_summary(metrics.mean_latency_ms(), metrics.max_latency_ms()),
            );
        });
    }

    /// Show `warning` unless it is already shown; `true` when it is new.
    pub(super) fn record_action_clamp_warning(&self, warning: String) -> bool {
        self.published.send_if_modified(|snapshot| {
            if snapshot.warning() == Some(warning.as_str()) {
                return false;
            }
            *snapshot = std::mem::take(snapshot).with_warning(Some(warning));
            true
        })
    }

    /// The rollout is terminal only after the hardware returns to a safe hold.
    pub(super) fn cleaning_up(&self) {
        self.published.send_modify(|snapshot| {
            snapshot.enter(
                RolloutPhase::Stopping,
                "Returning hardware control to a safe hold…".to_owned(),
            );
        });
    }

    pub(super) fn finish(
        &self,
        outcome: ExecutionOutcome,
        result: CleanupResult,
    ) -> RolloutSnapshot {
        let (phase, message) = outcome.terminal_state(result.cleanup);
        self.published.send_modify(|snapshot| {
            let started_ms = snapshot.started_ms();
            *snapshot = std::mem::take(snapshot)
                .with_lifecycle(phase, started_ms)
                .with_completion(Some(result.finished_ms), Some(message));
        });
        self.published.borrow().clone()
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
    use super::*;

    struct ProgressHarness {
        published: watch::Sender<RolloutSnapshot>,
        stop: watch::Sender<bool>,
    }

    impl ProgressHarness {
        fn new() -> Self {
            let admitted = RolloutAdmission::new(
                "session-1".to_owned(),
                100,
                RolloutConfig::new("run-1", 10, "follower").with_duration(30),
            )
            .into_snapshot()
            .unwrap();
            Self {
                published: watch::channel(admitted).0,
                stop: watch::channel(false).0,
            }
        }

        fn progress(&self) -> RolloutProgress<'_> {
            RolloutProgress::new(&self.published, self.stop.subscribe())
        }

        fn request_stop(&self) {
            self.stop.send_replace(true);
            self.published.send_modify(|snapshot| {
                snapshot.enter(RolloutPhase::Stopping, "Stopping…".to_owned());
            });
        }

        fn phase(&self) -> RolloutPhase {
            self.published.borrow().phase()
        }
    }

    #[test]
    fn admission_rejects_untrimmed_identities_and_unsafe_durations() {
        let admit = |session: &str, config| {
            RolloutAdmission::new(session.to_owned(), 100, config).into_snapshot()
        };
        let config = RolloutConfig::new("run-1", 10, "follower").with_duration(30);
        assert!(admit(" session", config.clone()).is_err());
        assert!(admit("session", RolloutConfig::new("run-1", 0, "follower")).is_err());
        assert!(admit("session", config.clone().with_duration(0)).is_err());
        let admitted = admit("session", config).unwrap();
        assert_eq!(admitted.phase(), RolloutPhase::Loading);
        assert_eq!(admitted.started_ms(), Some(100));
    }

    #[test]
    fn a_stop_is_never_overwritten_by_a_later_phase() {
        let harness = ProgressHarness::new();
        let progress = harness.progress();
        assert!(progress.warming_up());
        harness.request_stop();
        assert!(progress.stop_requested());
        assert!(!progress.awaiting_control());
        assert!(!progress.running(PolicyCadence::new(60.0, 10), 30));
        assert_eq!(harness.phase(), RolloutPhase::Stopping);
    }

    #[test]
    fn snapshot_tracks_semantic_lifecycle_and_metrics() {
        let harness = ProgressHarness::new();
        let progress = harness.progress();
        assert!(progress.warming_up());
        progress.record_warmup(InferenceTiming::new(12.0, 8.0));
        assert!(progress.awaiting_control());
        assert!(progress.running(PolicyCadence::new(60.0, 10), 30));
        progress
            .record_inference(InferenceTiming::new(20.0, 15.0))
            .unwrap();
        progress
            .record_inference(InferenceTiming::new(10.0, 7.0))
            .unwrap();
        assert!(
            progress
                .record_inference(InferenceTiming::new(f64::NAN, 1.0))
                .is_err()
        );
        progress.record_command(true);

        let snapshot = harness.published.borrow().clone();
        assert_eq!(snapshot.phase(), RolloutPhase::Running);
        assert_eq!(snapshot.metrics().inference_count(), 2);
        assert_eq!(snapshot.metrics().command_count(), 1);
        assert_eq!(snapshot.metrics().missed_deadlines(), 1);
        assert_eq!(snapshot.metrics().mean_latency_ms(), Some(15.0));
        assert_eq!(snapshot.metrics().max_latency_ms(), Some(20.0));
    }

    #[test]
    fn action_clamp_warning_is_visible_without_stopping_the_rollout() {
        let harness = ProgressHarness::new();
        let progress = harness.progress();
        assert!(progress.running(PolicyCadence::new(60.0, 10), 30));
        let warning =
            "Policy output for joint \"wrist_flex\" exceeded normalized [-1, 1]; the command was clamped to the limit."
                .to_owned();
        assert!(progress.record_action_clamp_warning(warning.clone()));
        assert!(!progress.record_action_clamp_warning(warning.clone()));
        assert_eq!(harness.phase(), RolloutPhase::Running);
        assert_eq!(harness.published.borrow().warning(), Some(warning.as_str()));
    }

    #[test]
    fn healthy_external_control_loss_is_completion_but_hardware_loss_is_failure() {
        let expected = || ExpectedPolicyOwner::new("follower", "session-1");
        assert_eq!(
            ExecutionOutcome::after_control_change(
                &ControlOwner::Policy {
                    robot: "follower".to_owned(),
                    session: "session-1".to_owned(),
                },
                None,
                expected(),
            ),
            None
        );
        assert!(matches!(
            ExecutionOutcome::after_control_change(
                &ControlOwner::Other("Manual on follower".to_owned()),
                None,
                expected(),
            ),
            Some(ExecutionOutcome::Completed(CompletionReason::ControlReplaced(message)))
                if message.contains("Manual on follower")
        ));
        assert!(matches!(
            ExecutionOutcome::after_control_change(
                &ControlOwner::Other("Idle".to_owned()),
                Some("motor bus stopped responding"),
                expected(),
            ),
            Some(ExecutionOutcome::Failed(message)) if message.contains("motor bus stopped responding")
        ));
    }

    #[test]
    fn executor_failure_is_terminal_only_after_cleanup() {
        let harness = ProgressHarness::new();
        let progress = harness.progress();
        progress.cleaning_up();
        assert_eq!(harness.phase(), RolloutPhase::Stopping);
        let terminal = progress.finish(
            ExecutionOutcome::Failed("native executor failed".to_owned()),
            CleanupResult::new(200, Ok(())),
        );
        assert_eq!(terminal.phase(), RolloutPhase::Failed);
        assert_eq!(terminal.started_ms(), Some(100));
        assert_eq!(terminal.finished_ms(), Some(200));
        assert!(
            terminal
                .message()
                .unwrap()
                .contains("native executor failed")
        );
        assert_eq!(*harness.published.borrow(), terminal);
    }

    #[test]
    fn cleanup_failure_cannot_report_a_successful_rollout() {
        let harness = ProgressHarness::new();
        let terminal = harness.progress().finish(
            ExecutionOutcome::Completed(CompletionReason::DurationReached {
                duration_seconds: 30,
            }),
            CleanupResult::new(200, Err("torque hold failed".to_owned())),
        );
        assert_eq!(terminal.phase(), RolloutPhase::Failed);
        assert!(terminal.message().unwrap().contains("torque hold failed"));
    }
}
