//! Native policy-rollout contracts.

use serde::{Deserialize, Serialize};
use spiral_robotics_core::operation::OperationLifecycle;

/// Operator-approved launch parameters for one backend-owned policy session.
pub use spiral_robotics_core::operation::{
    DEFAULT_ROLLOUT_DURATION_SECONDS, MAX_ROLLOUT_ACTION_STEPS, MAX_ROLLOUT_DURATION_SECONDS,
    MIN_ROLLOUT_DURATION_SECONDS,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartRolloutRequest {
    run_id: String,
    checkpoint_step: u64,
    robot: String,
    /// Time spent actively applying policy actions, excluding load and warm-up.
    #[serde(default = "StartRolloutRequest::default_duration_seconds")]
    duration_seconds: u32,
}

impl StartRolloutRequest {
    const fn default_duration_seconds() -> u32 {
        DEFAULT_ROLLOUT_DURATION_SECONDS
    }

    pub fn new(run_id: impl Into<String>, checkpoint_step: u64, robot: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            checkpoint_step,
            robot: robot.into(),
            duration_seconds: DEFAULT_ROLLOUT_DURATION_SECONDS,
        }
    }
    pub const fn with_duration(mut self, duration_seconds: u32) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub const fn checkpoint_step(&self) -> u64 {
        self.checkpoint_step
    }
    pub fn robot(&self) -> &str {
        &self.robot
    }
    pub const fn duration_seconds(&self) -> u32 {
        self.duration_seconds
    }
}

/// Authoritative lifecycle of the native inference and motor-control task.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutPhase {
    #[default]
    Idle,
    Loading,
    WarmingUp,
    Running,
    Stopping,
    Failed,
}

impl RolloutPhase {
    pub const fn active(self) -> bool {
        match self {
            Self::Loading => OperationLifecycle::Pending,
            Self::WarmingUp | Self::Running => OperationLifecycle::Running,
            Self::Stopping => OperationLifecycle::Stopping,
            Self::Idle | Self::Failed => OperationLifecycle::Finished,
        }
        .active()
    }
}

/// Measurements accumulated by the active native rollout session.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutMetrics {
    inference_count: u64,
    command_count: u64,
    missed_deadlines: u64,
    last_latency_ms: Option<f64>,
    last_model_ms: Option<f64>,
    mean_latency_ms: Option<f64>,
    max_latency_ms: Option<f64>,
}

impl RolloutMetrics {
    pub const fn new(inference_count: u64, command_count: u64, missed_deadlines: u64) -> Self {
        Self {
            inference_count,
            command_count,
            missed_deadlines,
            last_latency_ms: None,
            last_model_ms: None,
            mean_latency_ms: None,
            max_latency_ms: None,
        }
    }
    pub const fn with_latest_timings(
        mut self,
        last_latency_ms: Option<f64>,
        last_model_ms: Option<f64>,
    ) -> Self {
        self.last_latency_ms = last_latency_ms;
        self.last_model_ms = last_model_ms;
        self
    }
    pub const fn with_latency_summary(
        mut self,
        mean_latency_ms: Option<f64>,
        max_latency_ms: Option<f64>,
    ) -> Self {
        self.mean_latency_ms = mean_latency_ms;
        self.max_latency_ms = max_latency_ms;
        self
    }
    pub const fn inference_count(&self) -> u64 {
        self.inference_count
    }
    pub const fn command_count(&self) -> u64 {
        self.command_count
    }
    pub const fn missed_deadlines(&self) -> u64 {
        self.missed_deadlines
    }
    pub const fn last_latency_ms(&self) -> Option<f64> {
        self.last_latency_ms
    }
    pub const fn last_model_ms(&self) -> Option<f64> {
        self.last_model_ms
    }
    pub const fn mean_latency_ms(&self) -> Option<f64> {
        self.mean_latency_ms
    }
    pub const fn max_latency_ms(&self) -> Option<f64> {
        self.max_latency_ms
    }
}

/// Browser-safe state for rollout. Model bytes and device handles stay server-side.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutSnapshot {
    #[serde(flatten)]
    identity: RolloutIdentity,
    #[serde(flatten)]
    lifecycle: RolloutLifecycle,
    metrics: RolloutMetrics,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutIdentity {
    session_id: Option<String>,
    run_id: Option<String>,
    checkpoint_step: Option<u64>,
    robot: Option<String>,
    #[serde(default)]
    duration_seconds: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RolloutLifecycle {
    phase: RolloutPhase,
    started_ms: Option<i64>,
    finished_ms: Option<i64>,
    message: Option<String>,
    #[serde(default)]
    warning: Option<String>,
}

impl RolloutIdentity {
    pub fn new(session_id: Option<String>, run_id: Option<String>, robot: Option<String>) -> Self {
        Self {
            session_id,
            run_id,
            checkpoint_step: None,
            robot,
            duration_seconds: None,
        }
    }
    pub const fn with_checkpoint(mut self, checkpoint_step: Option<u64>) -> Self {
        self.checkpoint_step = checkpoint_step;
        self
    }
    pub const fn with_duration(mut self, duration_seconds: Option<u32>) -> Self {
        self.duration_seconds = duration_seconds;
        self
    }
}

impl RolloutLifecycle {
    pub const fn new(phase: RolloutPhase, started_ms: Option<i64>) -> Self {
        Self {
            phase,
            started_ms,
            finished_ms: None,
            message: None,
            warning: None,
        }
    }
    pub const fn with_finished_ms(mut self, finished_ms: Option<i64>) -> Self {
        self.finished_ms = finished_ms;
        self
    }
    pub fn with_messages(mut self, message: Option<String>, warning: Option<String>) -> Self {
        self.message = message;
        self.warning = warning;
        self
    }
}

impl RolloutSnapshot {
    pub fn new(
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
    pub const fn phase(&self) -> RolloutPhase {
        self.lifecycle.phase
    }
    pub fn session_id(&self) -> Option<&str> {
        self.identity.session_id.as_deref()
    }
    pub fn run_id(&self) -> Option<&str> {
        self.identity.run_id.as_deref()
    }
    pub const fn checkpoint_step(&self) -> Option<u64> {
        self.identity.checkpoint_step
    }
    pub fn robot(&self) -> Option<&str> {
        self.identity.robot.as_deref()
    }
    pub const fn duration_seconds(&self) -> Option<u32> {
        self.identity.duration_seconds
    }
    pub const fn started_ms(&self) -> Option<i64> {
        self.lifecycle.started_ms
    }
    pub const fn finished_ms(&self) -> Option<i64> {
        self.lifecycle.finished_ms
    }
    pub fn message(&self) -> Option<&str> {
        self.lifecycle.message.as_deref()
    }
    pub fn warning(&self) -> Option<&str> {
        self.lifecycle.warning.as_deref()
    }
    pub const fn metrics(&self) -> &RolloutMetrics {
        &self.metrics
    }
}
